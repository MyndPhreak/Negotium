use crate::{ActionError, OwnedHandle, open_verified};
use std::ffi::c_void;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, SID_NAME_USE, TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER,
    TokenElevation, TokenUser, TokenVirtualizationEnabled,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::SystemInformation::{
    IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_I386,
    IMAGE_FILE_MACHINE_UNKNOWN,
};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass, GetProcessInformation,
    HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, IsWow64Process2, NORMAL_PRIORITY_CLASS, OpenProcessToken,
    PROCESS_MACHINE_INFORMATION, PROCESS_NAME_WIN32, ProcessMachineTypeInfo, QueryFullProcessImageNameW,
    REALTIME_PRIORITY_CLASS,
};
use windows::core::PWSTR;

#[derive(Clone, Debug, PartialEq)]
pub struct Details {
    pub pid: u32,
    pub image_path: Option<String>,
    pub command_line: Option<String>,
    pub user: Option<String>,
    pub priority: Option<String>,
    /// The process token is elevated (runs as admin).
    pub elevated: Option<bool>,
    /// UAC file and registry virtualization is on for the process token.
    pub virtualized: Option<bool>,
    pub architecture: Option<Architecture>,
    pub efficiency: Option<bool>,
}

/// The machine a process's code is built for, and whether it runs emulated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Architecture {
    X86,
    X64,
    Arm64,
    X64OnArm64,
    X86OnArm64,
    Other,
}

/// `PROCESSINFOCLASS::ProcessCommandLineInformation`.
const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(process: HANDLE, class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

/// Reads display details for a process after verifying its identity. Any field that cannot
/// be read (protected process, access denied) is `None`.
pub fn process_details(pid: u32, create_time: i64) -> Result<Details, ActionError> {
    let h = open_verified(pid, create_time, Default::default())?;
    Ok(Details {
        pid,
        image_path: image_path(&h),
        command_line: command_line(&h),
        user: user(&h),
        priority: priority(&h),
        elevated: token_flag(&h, TokenElevation),
        virtualized: token_flag(&h, TokenVirtualizationEnabled),
        architecture: architecture(&h),
        efficiency: crate::is_efficiency_mode(pid, create_time).ok(),
    })
}

/// The most memory (working set) the process has used since it started, in bytes.
pub fn peak_working_set(pid: u32, create_time: i64) -> Result<u64, ActionError> {
    let h = open_verified(pid, create_time, Default::default())?;
    let mut counters = PROCESS_MEMORY_COUNTERS { cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
    // SAFETY: `counters` is writable and its size is passed in `cb`.
    unsafe { GetProcessMemoryInfo(h.0, &mut counters, counters.cb) }.map_err(|e| ActionError::from_win(&e))?;
    Ok(counters.PeakWorkingSetSize as u64)
}

/// A 32-bit token value read as a flag (TokenElevation, TokenVirtualizationEnabled).
fn token_flag(h: &OwnedHandle, class: TOKEN_INFORMATION_CLASS) -> Option<bool> {
    let mut token = HANDLE::default();
    // SAFETY: opens the process token for query; wrapped in OwnedHandle below.
    unsafe { OpenProcessToken(h.0, TOKEN_QUERY, &mut token) }.ok()?;
    let token = OwnedHandle(token);
    let mut value = 0u32;
    let mut needed = 0u32;
    // SAFETY: both classes return a single DWORD; `value` is 4 writable bytes.
    unsafe { GetTokenInformation(token.0, class, Some((&mut value as *mut u32).cast()), 4, &mut needed) }.ok()?;
    Some(value != 0)
}

fn architecture(h: &OwnedHandle) -> Option<Architecture> {
    let (mut process, mut native) = (IMAGE_FILE_MACHINE::default(), IMAGE_FILE_MACHINE::default());
    // SAFETY: valid handle with query rights; both outputs are writable.
    unsafe { IsWow64Process2(h.0, &mut process, Some(&mut native)) }.ok()?;
    // Windows 11 names the machine of an x64 process emulated on ARM64, which IsWow64Process2
    // reports as native (it is not WOW64).
    let mut info = PROCESS_MACHINE_INFORMATION::default();
    // SAFETY: `info` is a writable PROCESS_MACHINE_INFORMATION of the size passed.
    let machine = unsafe {
        GetProcessInformation(h.0, ProcessMachineTypeInfo, (&mut info as *mut PROCESS_MACHINE_INFORMATION).cast(), size_of::<PROCESS_MACHINE_INFORMATION>() as u32)
    }
    .map(|()| info.ProcessMachine)
    .unwrap_or(if process == IMAGE_FILE_MACHINE_UNKNOWN { native } else { process });
    Some(classify(machine, native))
}

fn classify(machine: IMAGE_FILE_MACHINE, native: IMAGE_FILE_MACHINE) -> Architecture {
    let on_arm = native == IMAGE_FILE_MACHINE_ARM64;
    match machine {
        m if m == IMAGE_FILE_MACHINE_ARM64 => Architecture::Arm64,
        m if m == IMAGE_FILE_MACHINE_AMD64 && on_arm => Architecture::X64OnArm64,
        m if m == IMAGE_FILE_MACHINE_AMD64 => Architecture::X64,
        m if m == IMAGE_FILE_MACHINE_I386 && on_arm => Architecture::X86OnArm64,
        m if m == IMAGE_FILE_MACHINE_I386 => Architecture::X86,
        _ => Architecture::Other,
    }
}

fn image_path(h: &OwnedHandle) -> Option<String> {
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` has `len` writable UTF-16 units and the handle has query rights.
    unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
    Some(String::from_utf16_lossy(buf.get(..len as usize)?))
}

fn command_line(h: &OwnedHandle) -> Option<String> {
    let mut needed = 0u32;
    // SAFETY: size query with a null buffer; the kernel reports the required size in `needed`.
    unsafe { NtQueryInformationProcess(h.0, PROCESS_COMMAND_LINE_INFORMATION, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    let capacity = (buf.len() * 8) as u32;
    // SAFETY: `buf` is 8-byte aligned and `capacity` bytes long.
    let status = unsafe {
        NtQueryInformationProcess(h.0, PROCESS_COMMAND_LINE_INFORMATION, buf.as_mut_ptr().cast(), capacity, &mut needed)
    };
    if status < 0 || (needed as usize) < 16 || needed > capacity {
        return None;
    }
    // The buffer starts with a UNICODE_STRING { u16 len_bytes, u16 max, (4 bytes padding),
    // *const u16 } whose buffer points into the same allocation (just past the header).
    let base = buf.as_ptr() as *const u8;
    // SAFETY: the kernel wrote at least a 16-byte UNICODE_STRING header at the start of `buf`
    // (`needed >= 16` checked), which is 8-byte aligned.
    let (len_bytes, ptr) = unsafe { (*(base as *const u16), *(base.add(8) as *const *const u16)) };
    if ptr.is_null() || len_bytes == 0 {
        return None;
    }
    let start = ptr as usize;
    let end = start.checked_add(len_bytes as usize)?;
    // Require the string to lie wholly inside the bytes the kernel wrote, after the header,
    // and be u16-aligned.
    if start < base as usize + 16 || end > base as usize + needed as usize || !start.is_multiple_of(2) {
        return None;
    }
    // SAFETY: `[start, end)` is within `buf` (checked above), aligned for u16, and initialised.
    let units = unsafe { std::slice::from_raw_parts(ptr, len_bytes as usize / 2) };
    Some(String::from_utf16_lossy(units))
}

fn user(h: &OwnedHandle) -> Option<String> {
    let mut token = HANDLE::default();
    // SAFETY: opens the process token for query; wrapped in OwnedHandle below.
    unsafe { OpenProcessToken(h.0, TOKEN_QUERY, &mut token) }.ok()?;
    let token = OwnedHandle(token);
    let mut needed = 0u32;
    // SAFETY: size query with a null buffer; fails with ERROR_INSUFFICIENT_BUFFER and sets `needed`.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
    if (needed as usize) < size_of::<TOKEN_USER>() {
        return None;
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    let capacity = (buf.len() * 8) as u32;
    // SAFETY: `buf` is 8-byte aligned and `capacity` bytes long.
    unsafe { GetTokenInformation(token.0, TokenUser, Some(buf.as_mut_ptr().cast()), capacity, &mut needed) }.ok()?;
    // SAFETY: the buffer now holds a TOKEN_USER (at least that many bytes, checked above);
    // its SID pointer refers into the same buffer.
    let sid = unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid };
    let (mut name, mut domain) = ([0u16; 256], [0u16; 256]);
    let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
    let mut use_ = SID_NAME_USE::default();
    // SAFETY: buffers and lengths are valid; `sid` points into `buf`, which is alive.
    unsafe {
        LookupAccountSidW(
            None,
            sid,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut use_,
        )
    }
    .ok()?;
    Some(format!(
        "{}\\{}",
        String::from_utf16_lossy(domain.get(..domain_len as usize)?),
        String::from_utf16_lossy(name.get(..name_len as usize)?)
    ))
}

fn priority(h: &OwnedHandle) -> Option<String> {
    // SAFETY: valid handle with query rights.
    let class = unsafe { GetPriorityClass(h.0) };
    let name = match class {
        c if c == IDLE_PRIORITY_CLASS.0 => "Low",
        c if c == BELOW_NORMAL_PRIORITY_CLASS.0 => "Below normal",
        c if c == NORMAL_PRIORITY_CLASS.0 => "Normal",
        c if c == ABOVE_NORMAL_PRIORITY_CLASS.0 => "Above normal",
        c if c == HIGH_PRIORITY_CLASS.0 => "High",
        c if c == REALTIME_PRIORITY_CLASS.0 => "Realtime",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Threading::GetCurrentProcessId;

    fn own() -> (u32, i64) {
        // SAFETY: plain Win32 call.
        let pid = unsafe { GetCurrentProcessId() };
        let h = open_verified(pid, 0, Default::default()).err();
        assert_eq!(h, Some(ActionError::AlreadyExited), "create time 0 never matches");
        // SAFETY: plain Win32 call; the handle is owned by OwnedHandle.
        let raw = unsafe {
            windows::Win32::System::Threading::OpenProcess(
                windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
        }
        .unwrap();
        let (ct, _) = crate::read_state(&OwnedHandle(raw)).unwrap();
        (pid, ct)
    }

    #[test]
    fn own_process_has_its_architecture() {
        let (pid, ct) = own();
        let want = if cfg!(target_arch = "aarch64") { Architecture::Arm64 } else { Architecture::X64 };
        assert_eq!(process_details(pid, ct).unwrap().architecture, Some(want));
    }

    #[test]
    fn own_process_reports_elevation_as_the_token_says() {
        let (pid, ct) = own();
        assert_eq!(process_details(pid, ct).unwrap().elevated, Some(crate::is_elevated()));
    }

    #[test]
    fn own_process_is_not_virtualized() {
        let (pid, ct) = own();
        assert_eq!(process_details(pid, ct).unwrap().virtualized, Some(false));
    }

    #[test]
    fn own_process_has_a_peak_at_least_its_working_set() {
        let (pid, ct) = own();
        assert!(peak_working_set(pid, ct).unwrap() >= 1 << 20);
    }

    #[test]
    fn a_wrong_create_time_is_already_exited() {
        let (pid, ct) = own();
        assert_eq!(peak_working_set(pid, ct + 1), Err(ActionError::AlreadyExited));
    }
}
