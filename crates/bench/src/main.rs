//! Measures an app the way spec §8 defines: launch → first window, CPU %, private
//! working set, and UI responsiveness (WM_NULL round-trip). Run from an elevated terminal.
mod childenv;
mod memmap;
mod stats;

use anyhow::{Context, Result, bail};
use negotium_collector::{ProcessReader, logical_cpus};
use serde::Serialize;
use stats::{Summary, summary};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, ERROR_TIMEOUT, GetLastError, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Threading::{GetProcessId, TerminateProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{IsUserAnAdmin, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetClassNameW, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SMTO_NORMAL,
    SW_SHOWNORMAL, SendMessageTimeoutW, WM_CLOSE, WM_NULL,
};
use windows::core::{BOOL, HSTRING, PCWSTR};

const WINDOW_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSIVENESS_TIMEOUT_MS: u32 = 1000;
/// Minimum time a window stays open before WM_CLOSE is posted: a GL app can still be starting up
/// right after its window appears, and an early WM_CLOSE may not be honoured.
const CLOSE_SETTLE: Duration = Duration::from_secs(2);

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
    /// How a Negotium run was started (renderer, switches, page, hidden).
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<NegotiumRun>,
    /// The renderer the run really drew with: the requested one, or "software (fell back from gpu)"
    /// if a GPU run's watchdog fell back (marker or log line in the scratch directory).
    #[serde(skip_serializing_if = "Option::is_none")]
    renderer_used: Option<String>,
    /// `cpu_pct` x logical processors: the share of one core, spec section 8's unit.
    cpu_one_core_pct: Summary,
    /// The busiest GPU engine's utilisation attributed to the process, %.
    gpu_pct: Summary,
    /// Where Negotium's memory went at the end of sampling (crate::memmap).
    #[serde(skip_serializing_if = "Option::is_none")]
    memory: Option<memmap::MemoryReport>,
}

/// What a Negotium run starts with (ignored for other targets): its renderer, its own settings
/// file (never the user's), its page, and whether it sits hidden in the tray while measured.
#[derive(Clone, Debug, PartialEq, Serialize)]
struct NegotiumRun {
    /// "gpu" (FemtoVG, Negotium's own choice) or "software"; None means gpu.
    renderer: Option<String>,
    smooth_graphs: bool,
    tray_cpu: bool,
    /// `--page=<name>` (overview, performance, ...).
    page: Option<String>,
    /// Close the window to the tray after it appears and measure the hidden process (no
    /// responsiveness probe: waking a hidden window would add to the cost being measured).
    hidden: bool,
}

impl Default for NegotiumRun {
    fn default() -> Self {
        Self { renderer: None, smooth_graphs: true, tray_cpu: true, page: None, hidden: false }
    }
}

impl NegotiumRun {
    /// SLINT_BACKEND for the chosen renderer. GPU is left to Negotium itself (None), which keeps
    /// its watchdog; software overrides it.
    fn backend(&self) -> Result<Option<&'static str>> {
        match self.renderer.as_deref() {
            None | Some("gpu") => Ok(None),
            Some("software") => Ok(Some("winit-software")),
            Some(other) => bail!("--renderer gpu|software, got {other}"),
        }
    }

    /// The run's settings file: defaults but for the switches.
    fn settings_json(&self) -> String {
        let mut v = serde_json::json!({ "smooth_graphs": self.smooth_graphs, "tray_cpu": self.tray_cpu });
        if self.hidden {
            // Closing must hide to the tray (the default, stated so the run does not depend on it).
            v["keep_in_tray"] = true.into();
        }
        v.to_string()
    }

    /// `--exit-on-close` makes closing exit, which is how a visible run ends; a hidden run needs
    /// closing to hide instead and is ended by the bench.
    fn args(&self) -> String {
        let mut args = if self.hidden { vec![] } else { vec!["--exit-on-close".to_string()] };
        args.extend(self.page.as_ref().map(|p| format!("--page={p}")));
        args.join(" ")
    }
}

/// The pages `--page=` knows (the app's `Page::name`s). A typo would silently open the Overview
/// while the report named the typed page, so anything else is an error. Returns the lower-case name.
fn valid_page(name: &str) -> Result<String> {
    const PAGES: [&str; 4] = ["overview", "performance", "startup", "settings"];
    let lower = name.to_ascii_lowercase();
    if name.chars().any(char::is_whitespace) || !PAGES.contains(&lower.as_str()) {
        bail!("--page takes one of {}, not {name:?}", PAGES.join(", "));
    }
    Ok(lower)
}

/// What a finished run left in its scratch directory (`<scratch>\settings.json` and the app's
/// `<scratch>\Negotium\...`): whether the GPU renderer fell back to software.
#[derive(Debug, Default, PartialEq)]
struct RunFiles {
    renderer_fell_back: bool,
}

/// Fails loudly if the app complained about the run's settings file (its loader is per-field
/// tolerant, so an invalid value would otherwise fall back to a default and the run would measure
/// something other than its report says), and notes a renderer fallback.
fn check_run_files(scratch: &Path) -> Result<RunFiles> {
    let bad = scratch.join("settings.json.bad");
    if bad.exists() {
        bail!("the app set the run's settings aside ({}): the bench's settings do not match the app's", bad.display());
    }
    let app_dir = scratch.join("Negotium");
    let log = std::fs::read_to_string(app_dir.join("logs").join("negotium.log")).unwrap_or_default();
    if let Some(line) = log.lines().find(|l| l.contains("settings.json")) {
        bail!("the app complained about the run's settings: {line}");
    }
    let fell_back = app_dir.join("renderer-fallback").exists() || log.contains("relaunching with the software renderer");
    Ok(RunFiles { renderer_fell_back: fell_back })
}

fn on_off(v: &str) -> Result<bool> {
    match v {
        "on" => Ok(true),
        "off" => Ok(false),
        other => bail!("expected on or off, got {other}"),
    }
}

/// Negotium itself, by keyword or by the file name of an explicit path: either way it gets the
/// isolated environment, never the user's real settings, marker and log.
fn is_negotium_target(target: &str) -> bool {
    target.eq_ignore_ascii_case("negotium")
        || Path::new(target).file_name().is_some_and(|n| n.eq_ignore_ascii_case("negotium.exe"))
}

/// A process name as given on the command line, with or without `.exe`, lower-cased.
fn exe_file_name(name: &str) -> String {
    let n = name.to_lowercase();
    if n.ends_with(".exe") { n } else { format!("{n}.exe") }
}

/// Why the bench will not start a child: it never closes or kills an instance it did not launch.
fn already_running_message(exe_name: &str, is_negotium: bool) -> String {
    if is_negotium {
        format!("{exe_name} is already running. The benchmark will not touch it: exit Negotium from its tray icon (right-click, Exit), then run the benchmark again.")
    } else {
        format!("{exe_name} is already running; close it first (Task Manager is single-instance)")
    }
}

/// `negotium-bench memory (--pid N | --name EXE) [--top N] [--out FILE]`: where a running
/// process's memory goes (crate::memmap). Needs no elevation for the user's own processes.
fn memory_command(mut args: impl Iterator<Item = String>) -> Result<()> {
    let (mut pid, mut top, mut out) = (None::<u32>, 12usize, None::<String>);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pid" => pid = Some(args.next().context("--pid N")?.parse()?),
            "--name" => {
                let name = exe_file_name(&args.next().context("--name EXE")?);
                pid = Some(running_pid(&name)?.with_context(|| format!("{name} is not running"))?);
            }
            "--top" => top = args.next().context("--top N")?.parse()?,
            "--out" => out = Some(args.next().context("--out FILE")?),
            other => bail!("unknown argument {other}"),
        }
    }
    let report = memmap::measure(pid.context("--pid N or --name EXE")?, top)?;
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = out {
        std::fs::write(&path, json).with_context(|| format!("write {path}"))?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let target = args.next().context("usage: negotium-bench <taskmgr|negotium|PATH> [--runs N] [--sample-secs S] [--warmup-secs S] [--renderer gpu|software] [--smooth on|off] [--tray-cpu on|off] [--page NAME] [--hidden] [--out FILE]
       negotium-bench memory (--pid N | --name EXE) [--top N] [--out FILE]")?;
    if target == "memory" {
        return memory_command(args);
    }
    let (mut runs, mut sample_secs, mut warmup_secs, mut out) = (5u32, 60u64, 5u64, None::<String>);
    let mut run = NegotiumRun::default();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--runs" => runs = args.next().context("--runs N")?.parse()?,
            "--sample-secs" => sample_secs = args.next().context("--sample-secs S")?.parse()?,
            "--warmup-secs" => warmup_secs = args.next().context("--warmup-secs S")?.parse()?,
            "--renderer" => run.renderer = Some(args.next().context("--renderer gpu|software")?),
            "--smooth" => run.smooth_graphs = on_off(&args.next().context("--smooth on|off")?)?,
            "--tray-cpu" => run.tray_cpu = on_off(&args.next().context("--tray-cpu on|off")?)?,
            "--page" => run.page = Some(valid_page(&args.next().context("--page NAME")?)?),
            "--hidden" => run.hidden = true,
            "--out" => out = Some(args.next().context("--out FILE")?),
            other => bail!("unknown argument {other}"),
        }
    }
    // SAFETY: no arguments; reads the current token.
    if !unsafe { IsUserAnAdmin() }.as_bool() {
        bail!("run from an elevated terminal: Task Manager auto-elevates and UIPI blocks probing higher-integrity windows");
    }
    let is_negotium = is_negotium_target(&target);
    if run != NegotiumRun::default() && !is_negotium {
        eprintln!("warning: --renderer, --smooth, --tray-cpu, --page and --hidden only apply to Negotium; ignored for {target}");
    }
    let backend = run.backend()?;
    run.renderer.get_or_insert_with(|| "gpu".to_string());
    let path = if target.eq_ignore_ascii_case("taskmgr") {
        PathBuf::from(r"C:\Windows\System32\Taskmgr.exe")
    } else if target.eq_ignore_ascii_case("negotium") {
        std::env::current_exe()?.with_file_name("negotium.exe")
    } else {
        PathBuf::from(&target)
    };
    let exe_name = path.file_name().context("target has no file name")?.to_string_lossy().to_lowercase();

    let stats_path = std::env::temp_dir().join("negotium-frame-stats.json");
    let real_local_app_data = std::env::var_os("LOCALAPPDATA");
    // Declared before `last`, so a launched child is cleaned up before its scratch directory goes.
    let mut scratch: Option<childenv::ScratchDir> = None;
    let launch_args = is_negotium.then(|| run.args());

    let mut startup_ms = Vec::new();
    let mut last: Option<Launched> = None;
    for run_index in 0..runs {
        if let Some(prev) = last.take() {
            prev.finish(run.hidden)?;
        }
        if running_pid(&exe_name)?.is_some() {
            bail!("{}", already_running_message(&exe_name, is_negotium));
        }
        if is_negotium {
            // Per-run reset: a run that fails to write its stats must not leave an earlier run's behind.
            let _ = std::fs::remove_file(&stats_path);
            // A fresh LOCALAPPDATA per run: its own settings.json, renderer marker and log, so the
            // user's real files are never read or written (and a fallback marker cannot leak into
            // the next run).
            let dir = childenv::scratch_local_appdata(&std::env::temp_dir(), real_local_app_data.as_deref(), std::process::id(), run_index)?;
            let dir = childenv::ScratchDir::create(dir)?;
            let settings_path = dir.path().join("settings.json");
            std::fs::write(&settings_path, run.settings_json()).context("write the run's settings")?;
            childenv::negotium_child_env(dir.path(), &settings_path, &stats_path, backend).apply_to_process();
            // Assigning drops the previous run's directory; its child was closed above.
            scratch = Some(dir);
        }
        let t0 = Instant::now();
        // An early return below drops `target`, which cleans the launched process up.
        let mut target = launch(&path, launch_args.as_deref())?;
        let hwnd = wait_for_window(target.pid)?;
        target.hwnd = Some(hwnd.0 as usize);
        target.appeared = Some(Instant::now());
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("run {}: first window after {ms:.0} ms", run_index + 1);
        startup_ms.push(ms);
        last = Some(target);
    }
    let target = last.context("--runs must be at least 1")?;
    println!("warming up pid {} for {warmup_secs} s, then sampling for {sample_secs} s …", target.pid);
    let mut target = target;
    if run.hidden {
        target.hide_to_tray()?;
        println!("the window is hidden in the tray; measuring the hidden process");
    }
    let (cpu, ws, gpu, resp, timeouts) =
        sample(&target, Duration::from_secs(warmup_secs), Duration::from_secs(sample_secs), !run.hidden)?;
    let memory = if is_negotium {
        memmap::measure(target.pid, 12).map_err(|e| eprintln!("memory breakdown unavailable: {e:#}")).ok()
    } else {
        None
    };
    // Before the child ends and the scratch directory goes: what the app made of the run's settings.
    let files = scratch.as_ref().map(|d| check_run_files(d.path())).transpose();
    target.finish(run.hidden)?;
    drop(scratch);
    let files = files?;
    let renderer_used = files.map(|f| match (run.renderer.as_deref(), f.renderer_fell_back) {
        (Some("gpu"), true) => {
            eprintln!("warning: the GPU renderer fell back to software during this run; its numbers are software numbers");
            "software (fell back from gpu)".to_string()
        }
        (r, _) => r.unwrap_or("gpu").to_string(),
    });

    // A hidden run is ended by the bench, so Negotium writes no frame stats.
    let frame_stats = if is_negotium && !run.hidden {
        let read = std::fs::read_to_string(&stats_path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).map_err(|e| e.to_string()));
        match read {
            Ok(v) => Some(v),
            Err(reason) => {
                eprintln!("frame stats unavailable: {reason}");
                None
            }
        }
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
        run: is_negotium.then(|| run.clone()),
        renderer_used,
        cpu_one_core_pct: summary(&cpu.iter().map(|c| c * logical_cpus() as f64).collect::<Vec<_>>()),
        gpu_pct: summary(&gpu),
        memory,
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
    /// When the window was first seen; `close` waits `CLOSE_SETTLE` after this.
    appeared: Option<Instant>,
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

    /// Waits (if needed) until the window has been open for `CLOSE_SETTLE`, then asks it to close.
    fn settle_and_post_close(&self) -> Result<()> {
        if let Some(appeared) = self.appeared {
            std::thread::sleep(CLOSE_SETTLE.saturating_sub(appeared.elapsed()));
        }
        self.post_close().context("PostMessageW WM_CLOSE")
    }

    /// Closes the window to the tray (Negotium hides instead of exiting when `--exit-on-close` is
    /// absent) and checks that it did: the window is gone and the process still runs.
    fn hide_to_tray(&mut self) -> Result<()> {
        self.settle_and_post_close()?;
        let hwnd = self.hwnd().context("target window not found")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        // SAFETY: a read-only query on a window handle; a destroyed window just reports not visible.
        while unsafe { IsWindowVisible(hwnd) }.as_bool() {
            if Instant::now() > deadline {
                bail!("the window did not hide after WM_CLOSE");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if self.wait_exited(500) {
            bail!("pid {} exited instead of hiding to the tray (is the tray unavailable?)", self.pid);
        }
        Ok(())
    }

    /// Ends the run: a visible one is closed (it exits on close); a hidden one is terminated, as
    /// closing only hides it. Either way this is a process the bench itself launched.
    fn finish(mut self, hidden: bool) -> Result<()> {
        if !hidden {
            return self.close();
        }
        // SAFETY: `handle` is the valid process handle owned by `self`, opened with terminate access.
        unsafe { TerminateProcess(self.handle, 0) }.context("TerminateProcess")?;
        if !self.wait_exited(5_000) {
            bail!("pid {} did not exit after TerminateProcess", self.pid);
        }
        self.armed = false;
        Ok(())
    }

    /// Waits (if needed) until the window has been open for `CLOSE_SETTLE`, asks it to close and
    /// waits up to 10 s for the process to exit.
    fn close(mut self) -> Result<()> {
        self.settle_and_post_close()?;
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

fn launch(path: &Path, args: Option<&str>) -> Result<Launched> {
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
    Ok(Launched { pid, handle, hwnd: None, appeared: None, armed: true })
}

/// winit (Slint's backend) creates a small visible, unowned message window ("Winit Thread Event
/// Target", 16x16) about 0.5 s before the real window exists. It is not the app's window: timing
/// it would under-report startup and probing/closing it would not exercise the UI.
fn is_helper_window(hwnd: HWND) -> bool {
    let mut buf = [0u16; 64];
    // SAFETY: `buf` is a valid writable buffer; the call only fills it.
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    len > 0 && String::from_utf16_lossy(&buf[..len as usize]) == "Winit Thread Event Target"
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
            && !is_helper_window(hwnd)
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
/// CPU %, private working set MB, GPU %, responsiveness ms, and the probe's timeouts (see `sample`).
type Samples = (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, usize);

/// With `probe` false no responsiveness series is taken: waking a hidden window would add to the
/// cost being measured.
fn sample(target: &Launched, warmup: Duration, duration: Duration, probe: bool) -> Result<Samples> {
    let pid = target.pid;
    let hwnd_bits = target.hwnd.context("target window not found")?;
    std::thread::sleep(warmup);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_probe = stop.clone();
    let prober = probe.then(|| std::thread::spawn(move || -> std::result::Result<(Vec<f64>, usize), String> {
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
    }));

    let cpus = logical_cpus() as f64;
    let mut reader = ProcessReader::new();
    // The busiest GPU engine the process uses (PDH "GPU Engine", as Task Manager shows it).
    let mut pdh = negotium_collector::PdhSources::open().ok();
    let (mut cpu, mut ws, mut gpu) = (Vec::new(), Vec::new(), Vec::new());
    let mut prev: Option<(Instant, u64)> = None;
    let end = Instant::now() + duration;
    let sampled: Result<()> = (|| {
        while Instant::now() < end && !prober.as_ref().is_some_and(|p| p.is_finished()) {
            let now = Instant::now();
            let p = read_processes(&mut reader)?
                .into_iter()
                .find(|p| p.pid == pid)
                .context("target exited during sampling")?;
            if let Some((t0, c0)) = prev {
                let capacity = now.duration_since(t0).as_secs_f64() * 10_000_000.0 * cpus;
                cpu.push((p.cpu_time_100ns.saturating_sub(c0) as f64 / capacity * 100.0).clamp(0.0, 100.0));
                if let Some(by_pid) = pdh.as_mut().and_then(|p| p.read().gpu_pct_by_pid) {
                    gpu.push(by_pid.get(&pid).copied().unwrap_or(0.0) as f64);
                }
            }
            ws.push(p.private_working_set as f64 / (1024.0 * 1024.0));
            prev = Some((now, p.cpu_time_100ns));
            std::thread::sleep(Duration::from_secs(1));
        }
        Ok(())
    })();
    stop.store(true, Ordering::Relaxed);
    let probed = prober.map(|p| p.join().expect("prober thread")).unwrap_or(Ok((Vec::new(), 0)));
    // A probe failure is the root cause when the target vanished or was unreachable, so report it first.
    let (resp, timeouts) = probed.map_err(|e| anyhow::anyhow!("responsiveness probe failed: {e}"))?;
    sampled?;
    Ok((cpu, ws, gpu, resp, timeouts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_negotium_is_never_closed_and_the_user_is_told_to_exit_it_from_the_tray() {
        let m = already_running_message("negotium.exe", true);
        assert!(m.contains("tray") && m.contains("will not touch"), "{m}");
        assert!(!already_running_message("taskmgr.exe", false).contains("tray"));
    }

    #[test]
    fn negotium_is_recognised_by_keyword_or_by_the_exe_name_of_a_path() {
        for t in ["negotium", "Negotium", r"C:\x\target\release\negotium.exe", r"C:\x\NEGOTIUM.EXE", "negotium.exe"] {
            assert!(is_negotium_target(t), "{t}");
        }
        for t in ["taskmgr", r"C:\Windows\System32\Taskmgr.exe", r"C:\negotium\other.exe", "negotium-bench.exe"] {
            assert!(!is_negotium_target(t), "{t}");
        }
    }

    #[test]
    fn a_negotium_run_starts_with_its_own_settings_renderer_and_page() {
        let mut run = NegotiumRun::default();
        assert_eq!(run.args(), "--exit-on-close");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&run.settings_json()).unwrap(), serde_json::json!({ "smooth_graphs": true, "tray_cpu": true }));
        assert_eq!(run.backend().unwrap(), None);
        run.renderer = Some("software".into());
        run.smooth_graphs = false;
        run.page = Some("performance".into());
        assert_eq!(run.backend().unwrap(), Some("winit-software"));
        assert_eq!(run.args(), "--exit-on-close --page=performance");
        assert!(run.settings_json().contains("\"smooth_graphs\":false"));
        run.renderer = Some("vulkan".into());
        assert!(run.backend().is_err());
        assert!(on_off("on").unwrap() && !on_off("off").unwrap() && on_off("yes").is_err());
    }

    #[test]
    fn a_hidden_run_keeps_the_window_hidden_in_the_tray_and_does_not_exit_on_close() {
        let run = NegotiumRun { hidden: true, page: Some("performance".into()), ..NegotiumRun::default() };
        assert_eq!(run.args(), "--page=performance", "closing must hide to the tray, not exit");
        let v: serde_json::Value = serde_json::from_str(&run.settings_json()).unwrap();
        assert_eq!(v["keep_in_tray"], true);
        assert_eq!(v["tray_cpu"], true);
    }

    #[test]
    fn page_names_are_checked_not_trusted() {
        assert_eq!(valid_page("Performance").unwrap(), "performance");
        for ok in ["overview", "startup", "settings"] {
            assert_eq!(valid_page(ok).unwrap(), ok);
        }
        for bad in ["perfomance", "", "performance ", " settings", "start up", "over\tview", "performance --smooth"] {
            assert!(valid_page(bad).is_err(), "{bad:?}");
        }
    }

    /// Literal copy of the field names in the app's `settings::load` (`field!(...)`). The app ignores
    /// a key it does not know, so a rename there must fail here; the app has the twin test
    /// `the_bench_settings_keys_are_fields_the_app_loads`.
    const APP_FIELDS: [&str; 7] = ["refresh_ms", "theme", "always_on_top", "keep_in_tray", "gpu_rendering", "smooth_graphs", "tray_cpu"];

    #[test]
    fn the_settings_keys_are_fields_the_app_knows() {
        for hidden in [false, true] {
            let run = NegotiumRun { hidden, ..NegotiumRun::default() };
            let v: serde_json::Value = serde_json::from_str(&run.settings_json()).unwrap();
            for key in v.as_object().unwrap().keys() {
                assert!(APP_FIELDS.contains(&key.as_str()), "{key} is not a settings field of the app");
            }
        }
    }

    #[test]
    fn a_run_whose_settings_the_app_complained_about_fails_and_a_fallback_is_noted() {
        let root = std::env::temp_dir().join(format!("negotium-bench-runfiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let logs = root.join("Negotium").join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        assert_eq!(check_run_files(&root).unwrap(), RunFiles::default(), "nothing logged is fine");
        std::fs::write(logs.join("negotium.log"), "1 WARN worker restarted\n").unwrap();
        assert!(!check_run_files(&root).unwrap().renderer_fell_back);
        std::fs::write(root.join("Negotium").join("renderer-fallback"), "x").unwrap();
        assert!(check_run_files(&root).unwrap().renderer_fell_back);
        std::fs::write(logs.join("negotium.log"), "1 WARN settings.json had invalid fields (tray_cpu); they use their defaults\n").unwrap();
        assert!(check_run_files(&root).is_err());
        std::fs::write(logs.join("negotium.log"), "").unwrap();
        std::fs::write(root.join("settings.json.bad"), "{}").unwrap();
        assert!(check_run_files(&root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn memory_name_works_with_or_without_exe() {
        assert_eq!(exe_file_name("Negotium"), "negotium.exe");
        assert_eq!(exe_file_name("negotium.EXE"), "negotium.exe");
    }
}
