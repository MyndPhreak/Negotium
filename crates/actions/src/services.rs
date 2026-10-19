//! Windows Services enumeration and control (`EnumServicesStatusExW`, `QueryServiceConfigW`,
//! `StartServiceW`, `ControlService`). Call off the UI thread.
use crate::ActionError;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::ERROR_MORE_DATA;
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW, OpenSCManagerW,
    OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfigW, QueryServiceStatusEx, SC_ENUM_PROCESS_INFO, SC_HANDLE,
    SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE, SC_STATUS_PROCESS_INFO, SERVICE_AUTO_START, SERVICE_BOOT_START,
    SERVICE_CONTINUE_PENDING, SERVICE_CONTROL_STOP, SERVICE_DEMAND_START, SERVICE_DISABLED,
    SERVICE_PAUSE_PENDING, SERVICE_PAUSED, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START,
    SERVICE_START_PENDING, SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STATUS_PROCESS, SERVICE_STOP,
    SERVICE_STOP_PENDING, SERVICE_STOPPED, SERVICE_SYSTEM_START, SERVICE_WIN32, StartServiceW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceEntry {
    pub name: String,
    pub display_name: String,
    pub status: String,
    pub running: bool,
    pub pid: Option<u32>,
    pub start_type: String,
    pub group: String,
}

struct ScHandle(SC_HANDLE);
impl Drop for ScHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: valid SC_HANDLE closed once.
            unsafe {
                let _ = CloseServiceHandle(self.0);
            }
        }
    }
}

pub fn state_label(state: u32) -> &'static str {
    match state {
        x if x == SERVICE_RUNNING.0 => "Running",
        x if x == SERVICE_STOPPED.0 => "Stopped",
        x if x == SERVICE_START_PENDING.0 => "Starting",
        x if x == SERVICE_STOP_PENDING.0 => "Stopping",
        x if x == SERVICE_PAUSED.0 => "Paused",
        x if x == SERVICE_PAUSE_PENDING.0 => "Pausing",
        x if x == SERVICE_CONTINUE_PENDING.0 => "Resuming",
        _ => "Unknown",
    }
}

pub fn start_type_label(start_type: u32) -> &'static str {
    match windows::Win32::System::Services::SERVICE_START_TYPE(start_type) {
        SERVICE_AUTO_START => "Automatic",
        SERVICE_DEMAND_START => "Manual",
        SERVICE_DISABLED => "Disabled",
        SERVICE_BOOT_START => "Boot",
        SERVICE_SYSTEM_START => "System",
        _ => "—",
    }
}

/// Extracts the `svchost.exe -k <group>` service group from a binary path line, if present.
pub fn extract_svchost_group(bin_path: &str) -> Option<String> {
    let lower = bin_path.to_ascii_lowercase();
    if !lower.contains("svchost") {
        return None;
    }
    let mut tokens = bin_path.split_whitespace();
    while let Some(tok) = tokens.next() {
        if tok.eq_ignore_ascii_case("-k") {
            return tokens.next().map(|g| g.trim_matches('"').to_string()).filter(|g| !g.is_empty());
        }
    }
    None
}

unsafe fn pwstr_to_string(ptr: PWSTR) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: NUL-terminated wide string inside service buffer.
    unsafe { ptr.to_string().unwrap_or_default() }
}

/// Enumerates all Win32 services on the local machine.
pub fn list_services() -> Result<Vec<ServiceEntry>, ActionError> {
    // SAFETY: connects to local SCM for enumeration.
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ENUMERATE_SERVICE) }
        .map(ScHandle)
        .map_err(|e| ActionError::from_win(&e))?;

    let mut needed = 0u32;
    let mut count = 0u32;
    let mut resume = 0u32;
    // SAFETY: size probe with None buffer; expects ERROR_MORE_DATA.
    let first = unsafe {
        EnumServicesStatusExW(
            scm.0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            None,
            &mut needed,
            &mut count,
            Some(&mut resume),
            PCWSTR::null(),
        )
    };
    if let Err(e) = first
        && e.code() != ERROR_MORE_DATA.to_hresult()
    {
        return Err(ActionError::from_win(&e));
    }
    if needed == 0 {
        return Ok(Vec::new());
    }

    let mut buf = vec![0u8; needed as usize + 1024];
    resume = 0;
    // SAFETY: `buf` is writable of length `buf.len()`.
    unsafe {
        EnumServicesStatusExW(
            scm.0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            Some(&mut buf),
            &mut needed,
            &mut count,
            Some(&mut resume),
            PCWSTR::null(),
        )
    }
    .map_err(|e| ActionError::from_win(&e))?;

    // SAFETY: `buf` begins with `count` contiguous `ENUM_SERVICE_STATUS_PROCESSW` structs.
    let raw = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize) };
    let mut config_buf = vec![0u8; 8192];
    let mut out = Vec::with_capacity(count as usize);

    for item in raw {
        // SAFETY: pointers in `item` point into `buf`.
        let name = unsafe { pwstr_to_string(item.lpServiceName) };
        // SAFETY: pointers in `item` point into `buf`.
        let display_name = unsafe { pwstr_to_string(item.lpDisplayName) };
        let st = item.ServiceStatusProcess;
        let state = st.dwCurrentState.0;
        let running = st.dwCurrentState == SERVICE_RUNNING;
        let pid = (st.dwProcessId != 0).then_some(st.dwProcessId);
        let (start_type, group) = query_config(scm.0, PCWSTR(item.lpServiceName.0), &mut config_buf);
        out.push(ServiceEntry {
            name,
            display_name,
            status: state_label(state).to_string(),
            running,
            pid,
            start_type,
            group,
        });
    }
    Ok(out)
}

fn query_config(scm: SC_HANDLE, service_name: PCWSTR, buf: &mut Vec<u8>) -> (String, String) {
    // SAFETY: opens service for config query.
    let Ok(svc) = (unsafe { OpenServiceW(scm, service_name, SERVICE_QUERY_CONFIG) }).map(ScHandle) else {
        return ("—".into(), String::new());
    };
    let mut needed = 0u32;
    // SAFETY: queries config into `buf`; resizes once if needed.
    let mut ok = unsafe {
        QueryServiceConfigW(svc.0, Some(buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW), buf.len() as u32, &mut needed)
    };
    if ok.is_err() && needed as usize > buf.len() {
        buf.resize(needed as usize, 0);
        // SAFETY: `buf` resized to `needed`.
        ok = unsafe {
            QueryServiceConfigW(svc.0, Some(buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW), buf.len() as u32, &mut needed)
        };
    }
    if ok.is_err() {
        return ("—".into(), String::new());
    }
    // SAFETY: `buf` holds a valid `QUERY_SERVICE_CONFIGW`.
    let cfg = unsafe { &*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW) };
    let start = start_type_label(cfg.dwStartType.0).to_string();
    // SAFETY: `lpBinaryPathName` and `lpLoadOrderGroup` point into `buf`.
    let bin = unsafe { pwstr_to_string(cfg.lpBinaryPathName) };
    let group = extract_svchost_group(&bin).unwrap_or_else(|| unsafe { pwstr_to_string(cfg.lpLoadOrderGroup) });
    (start, group)
}

/// Starts a stopped service.
pub fn start_service(name: &str) -> Result<(), ActionError> {
    // SAFETY: connects to local SCM.
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .map(ScHandle)
        .map_err(|e| ActionError::from_win(&e))?;
    let name_h = HSTRING::from(name);
    // SAFETY: opens service with SERVICE_START.
    let svc = unsafe { OpenServiceW(scm.0, &name_h, SERVICE_START) }
        .map(ScHandle)
        .map_err(|e| ActionError::from_win(&e))?;
    // SAFETY: starts the service with no arguments.
    unsafe { StartServiceW(svc.0, None) }.map_err(|e| ActionError::from_win(&e))
}

/// Stops a running service.
pub fn stop_service(name: &str) -> Result<(), ActionError> {
    // SAFETY: connects to local SCM.
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .map(ScHandle)
        .map_err(|e| ActionError::from_win(&e))?;
    let name_h = HSTRING::from(name);
    // SAFETY: opens service with SERVICE_STOP.
    let svc = unsafe { OpenServiceW(scm.0, &name_h, SERVICE_STOP) }
        .map(ScHandle)
        .map_err(|e| ActionError::from_win(&e))?;
    let mut status = SERVICE_STATUS::default();
    // SAFETY: sends SERVICE_CONTROL_STOP.
    unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut status) }.map_err(|e| ActionError::from_win(&e))
}

/// Restarts a service (stops it if running, waits up to 10s for it to reach Stopped, then starts it).
pub fn restart_service(name: &str) -> Result<(), ActionError> {
    // SAFETY: connects to local SCM.
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .map(ScHandle)
        .map_err(|e| ActionError::from_win(&e))?;
    let name_h = HSTRING::from(name);
    // SAFETY: opens service with start, stop, and query status rights.
    let svc = unsafe { OpenServiceW(scm.0, &name_h, SERVICE_STOP | SERVICE_START | SERVICE_QUERY_STATUS) }
        .map(ScHandle)
        .map_err(|e| ActionError::from_win(&e))?;
    let mut status = SERVICE_STATUS::default();
    // Ignore error if already stopped.
    // SAFETY: sends SERVICE_CONTROL_STOP.
    let _ = unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut status) };

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let mut ssp = SERVICE_STATUS_PROCESS::default();
        let mut needed = 0u32;
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                &mut ssp as *mut SERVICE_STATUS_PROCESS as *mut u8,
                std::mem::size_of::<SERVICE_STATUS_PROCESS>(),
            )
        };
        // SAFETY: queries current process status into `ssp`.
        if unsafe { QueryServiceStatusEx(svc.0, SC_STATUS_PROCESS_INFO, Some(bytes), &mut needed) }.is_ok()
            && ssp.dwCurrentState == SERVICE_STOPPED
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    // SAFETY: starts the service.
    unsafe { StartServiceW(svc.0, None) }.map_err(|e| ActionError::from_win(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_svchost_group_from_binary_path() {
        assert_eq!(
            extract_svchost_group(r"C:\WINDOWS\system32\svchost.exe -k LocalServiceNetworkRestricted -p"),
            Some("LocalServiceNetworkRestricted".into())
        );
        assert_eq!(
            extract_svchost_group(r"C:\WINDOWS\System32\svchost.exe -k netsvcs"),
            Some("netsvcs".into())
        );
        assert_eq!(extract_svchost_group(r"C:\Program Files\App\service.exe -k foo"), None);
    }

    #[test]
    fn live_services_enumeration_returns_known_windows_services() {
        let services = list_services().expect("list_services");
        assert!(!services.is_empty(), "Windows always has services");
        assert!(services.iter().any(|s| s.name.eq_ignore_ascii_case("EventLog") || s.name.eq_ignore_ascii_case("RpcSs")));
    }
}
