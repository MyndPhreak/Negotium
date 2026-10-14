//! Measures an app the way spec §8 defines: launch → first window, CPU %, private
//! working set, and UI responsiveness (WM_NULL round-trip). Run from an elevated terminal.
mod stats;

use anyhow::{Context, Result, bail};
use negotium_collector::{ProcessReader, logical_cpus};
use serde::Serialize;
use stats::{Summary, summary};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, ERROR_TIMEOUT, GetLastError, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Threading::{GetProcessId, TerminateProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{IsUserAnAdmin, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SMTO_NORMAL,
    SW_SHOWNORMAL, SendMessageTimeoutW, WM_CLOSE, WM_NULL,
};
use windows::core::{BOOL, HSTRING, PCWSTR};

const WINDOW_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSIVENESS_TIMEOUT_MS: u32 = 1000;

#[derive(Serialize)]
struct Report {
    target: String,
    logical_cpus: u32,
    warmup_secs: u64,
    startup_ms: Vec<f64>,
    startup: Summary,
    cpu_pct: Summary,
    private_working_set_mb: Summary,
    responsiveness_ms: Summary,
    responsiveness_timeouts: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_stats: Option<serde_json::Value>,
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let target = args.next().context("usage: negotium-bench <taskmgr|negotium|PATH> [--runs N] [--sample-secs S] [--warmup-secs S] [--out FILE]")?;
    let (mut runs, mut sample_secs, mut warmup_secs, mut out) = (5u32, 60u64, 5u64, None::<String>);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--runs" => runs = args.next().context("--runs N")?.parse()?,
            "--sample-secs" => sample_secs = args.next().context("--sample-secs S")?.parse()?,
            "--warmup-secs" => warmup_secs = args.next().context("--warmup-secs S")?.parse()?,
            "--out" => out = Some(args.next().context("--out FILE")?),
            other => bail!("unknown argument {other}"),
        }
    }
    // SAFETY: no arguments; reads the current token.
    if !unsafe { IsUserAnAdmin() }.as_bool() {
        bail!("run from an elevated terminal: Task Manager auto-elevates and UIPI blocks probing higher-integrity windows");
    }
    let is_negotium = target.eq_ignore_ascii_case("negotium");
    let path = if target.eq_ignore_ascii_case("taskmgr") {
        PathBuf::from(r"C:\Windows\System32\Taskmgr.exe")
    } else if is_negotium {
        std::env::current_exe()?.with_file_name("negotium.exe")
    } else {
        PathBuf::from(&target)
    };
    let exe_name = path.file_name().context("target has no file name")?.to_string_lossy().to_lowercase();

    let stats_path = std::env::temp_dir().join("negotium-frame-stats.json");
    if is_negotium {
        let _ = std::fs::remove_file(&stats_path);
        // SAFETY: single-threaded at this point; no other thread reads the environment.
        unsafe { std::env::set_var("NEGOTIUM_FRAME_STATS", &stats_path) };
    }
    let launch_args = is_negotium.then_some("--exit-on-close");

    let mut startup_ms = Vec::new();
    let mut last: Option<Launched> = None;
    for run in 0..runs {
        if let Some(prev) = last.take() {
            prev.close()?;
        }
        if running_pid(&exe_name)?.is_some() {
            bail!("{exe_name} is already running; close it first (Task Manager is single-instance)");
        }
        let t0 = Instant::now();
        // An early return below drops `target`, which cleans the launched process up.
        let mut target = launch(&path, launch_args)?;
        let hwnd = wait_for_window(target.pid)?;
        target.hwnd = Some(hwnd.0 as usize);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("run {}: first window after {ms:.0} ms", run + 1);
        startup_ms.push(ms);
        last = Some(target);
    }
    let target = last.context("--runs must be at least 1")?;
    println!("warming up pid {} for {warmup_secs} s, then sampling for {sample_secs} s …", target.pid);
    let (cpu, ws, resp, timeouts) =
        sample(&target, Duration::from_secs(warmup_secs), Duration::from_secs(sample_secs))?;
    target.close()?;

    let frame_stats = if is_negotium {
        std::fs::read_to_string(&stats_path).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    } else {
        None
    };
    let report = Report {
        target: path.display().to_string(),
        logical_cpus: logical_cpus(),
        warmup_secs,
        startup: summary(&startup_ms),
        startup_ms,
        cpu_pct: summary(&cpu),
        private_working_set_mb: summary(&ws),
        responsiveness_ms: summary(&resp),
        responsiveness_timeouts: timeouts,
        frame_stats,
    };
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = out {
        std::fs::write(&path, json).with_context(|| format!("write {path}"))?;
    }
    Ok(())
}

fn read_processes(reader: &mut ProcessReader) -> Result<Vec<negotium_collector::RawProcess>> {
    reader.read().map_err(|status| anyhow::anyhow!("could not read the process list: NTSTATUS {status:#010x}"))
}

fn running_pid(exe_name: &str) -> Result<Option<u32>> {
    let list = read_processes(&mut ProcessReader::new())?;
    Ok(list.into_iter().find(|p| p.name.to_lowercase() == exe_name).map(|p| p.pid))
}

/// A launched target process. Owns the process handle from `ShellExecuteExW`, so the process
/// cannot be confused with an unrelated one that reuses its id. If dropped while still armed
/// (any early return or error after launch), it closes the target, falling back to terminating
/// it, so no process is left behind. The handle is closed exactly once, in `Drop`.
struct Launched {
    pid: u32,
    handle: HANDLE,
    hwnd: Option<usize>,
    armed: bool,
}

impl Launched {
    fn hwnd(&self) -> Option<HWND> {
        self.hwnd.map(|bits| HWND(bits as *mut c_void))
    }

    fn post_close(&self) -> windows::core::Result<()> {
        let hwnd = self.hwnd().ok_or_else(windows::core::Error::empty)?;
        // SAFETY: asks the window to close; no pointers involved.
        unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
    }

    /// Waits up to `timeout_ms` for the process to exit; true if it has exited.
    fn wait_exited(&self, timeout_ms: u32) -> bool {
        // SAFETY: `handle` is a valid process handle owned by `self` until Drop.
        unsafe { WaitForSingleObject(self.handle, timeout_ms) == WAIT_OBJECT_0 }
    }

    /// Asks the window to close and waits up to 10 s for the process to exit.
    fn close(mut self) -> Result<()> {
        self.post_close().context("PostMessageW WM_CLOSE")?;
        if !self.wait_exited(10_000) {
            bail!("pid {} did not exit after WM_CLOSE", self.pid);
        }
        self.armed = false;
        Ok(())
    }
}

impl Drop for Launched {
    fn drop(&mut self) {
        if self.armed && !self.wait_exited(0) {
            if self.hwnd.is_some() {
                let _ = self.post_close();
            }
            if !self.wait_exited(5_000) {
                // SAFETY: `handle` is the valid process handle owned by `self`; PROCESS_TERMINATE
                // access was granted by ShellExecuteExW's process handle.
                unsafe {
                    let _ = TerminateProcess(self.handle, 1);
                }
            }
        }
        // SAFETY: `handle` is owned by `self` (SEE_MASK_NOCLOSEPROCESS) and closed exactly once, here.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

fn launch(path: &PathBuf, args: Option<&str>) -> Result<Launched> {
    let file = HSTRING::from(path.as_os_str());
    let params = args.map(HSTRING::from);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: params.as_ref().map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is fully initialized and `file` and `params` outlive the call.
    unsafe { ShellExecuteExW(&mut info) }.context("ShellExecuteExW")?;
    let handle = info.hProcess;
    if handle.is_invalid() {
        bail!("ShellExecuteExW returned no process handle");
    }
    // SAFETY: `handle` is a valid process handle requested via SEE_MASK_NOCLOSEPROCESS.
    let pid = unsafe { GetProcessId(handle) };
    if pid == 0 {
        // SAFETY: `handle` is valid and owned by us; terminate the process we cannot identify, then close once.
        unsafe {
            let _ = TerminateProcess(handle, 1);
        }
        // SAFETY: `handle` is valid, owned by us, and closed exactly once on this path.
        unsafe {
            let _ = CloseHandle(handle);
        }
        bail!("could not read launched process id");
    }
    Ok(Launched { pid, handle, hwnd: None, armed: true })
}

struct Search {
    pid: u32,
    found: Option<HWND>,
}

unsafe extern "system" fn find_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the `&mut Search` passed by first_window.
    let search = unsafe { &mut *(lparam.0 as *mut Search) };
    let mut pid = 0u32;
    // SAFETY: read-only queries on a handle supplied by EnumWindows.
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == search.pid
            && IsWindowVisible(hwnd).as_bool()
            && !GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid())
        {
            search.found = Some(hwnd);
            return false.into();
        }
    }
    true.into()
}

fn first_window(pid: u32) -> Option<HWND> {
    let mut search = Search { pid, found: None };
    // SAFETY: `search` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(Some(find_window), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

fn wait_for_window(pid: u32) -> Result<HWND> {
    let deadline = Instant::now() + WINDOW_TIMEOUT;
    loop {
        if let Some(h) = first_window(pid) {
            return Ok(h);
        }
        if Instant::now() > deadline {
            bail!("no visible window for pid {pid} within {WINDOW_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Waits `warmup` (excluded from the samples, so the launch burst does not skew them), then
/// samples for `duration`.
fn sample(target: &Launched, warmup: Duration, duration: Duration) -> Result<(Vec<f64>, Vec<f64>, Vec<f64>, usize)> {
    let pid = target.pid;
    let hwnd_bits = target.hwnd.context("target window not found")?;
    std::thread::sleep(warmup);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_probe = stop.clone();
    let prober = std::thread::spawn(move || -> std::result::Result<(Vec<f64>, usize), String> {
        let hwnd = HWND(hwnd_bits as *mut c_void);
        let (mut times, mut timeouts) = (Vec::new(), 0usize);
        while !stop_probe.load(Ordering::Relaxed) {
            let t = Instant::now();
            // SAFETY: WM_NULL has no side effects; bounded by the timeout.
            let r = unsafe {
                SendMessageTimeoutW(hwnd, WM_NULL, WPARAM(0), LPARAM(0), SMTO_NORMAL, RESPONSIVENESS_TIMEOUT_MS, None)
            };
            if r.0 == 0 {
                // SAFETY: reads this thread's last-error value, set by the call above.
                let err = unsafe { GetLastError() };
                if err != ERROR_TIMEOUT {
                    return Err(windows::core::Error::from(err.to_hresult()).to_string());
                }
                timeouts += 1;
                times.push(RESPONSIVENESS_TIMEOUT_MS as f64);
            } else {
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok((times, timeouts))
    });

    let cpus = logical_cpus() as f64;
    let mut reader = ProcessReader::new();
    let (mut cpu, mut ws) = (Vec::new(), Vec::new());
    let mut prev: Option<(Instant, u64)> = None;
    let end = Instant::now() + duration;
    let sampled: Result<()> = (|| {
        while Instant::now() < end && !prober.is_finished() {
            let now = Instant::now();
            let p = read_processes(&mut reader)?
                .into_iter()
                .find(|p| p.pid == pid)
                .context("target exited during sampling")?;
            if let Some((t0, c0)) = prev {
                let capacity = now.duration_since(t0).as_secs_f64() * 10_000_000.0 * cpus;
                cpu.push((p.cpu_time_100ns.saturating_sub(c0) as f64 / capacity * 100.0).clamp(0.0, 100.0));
            }
            ws.push(p.private_working_set as f64 / (1024.0 * 1024.0));
            prev = Some((now, p.cpu_time_100ns));
            std::thread::sleep(Duration::from_secs(1));
        }
        Ok(())
    })();
    stop.store(true, Ordering::Relaxed);
    let probed = prober.join().expect("prober thread");
    // A probe failure is the root cause when the target vanished or was unreachable, so report it first.
    let (resp, timeouts) = probed.map_err(|e| anyhow::anyhow!("responsiveness probe failed: {e}"))?;
    sampled?;
    Ok((cpu, ws, resp, timeouts))
}
