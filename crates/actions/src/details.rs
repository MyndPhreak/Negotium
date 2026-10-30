use crate::{ActionError, OwnedHandle, open_verified};
use std::ffi::c_void;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, PSID, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, OpenProcessToken, PROCESS_NAME_WIN32, QueryFullProcessImageNameW,
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
    Ok(Details { pid, image_path: image_path(&h), command_line: command_line(&h), user: user(&h), priority: priority(&h) })
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
    // SAFETY: `sid` points into `buf`, which is alive.
    let (domain, name) = unsafe { sid_account(sid) }?;
    Some(format!("{domain}\\{name}"))
}

/// (domain, name) of the account `sid` names.
///
/// # Safety
/// `sid` must point at a valid SID.
pub(crate) unsafe fn sid_account(sid: PSID) -> Option<(String, String)> {
    let (mut name, mut domain) = ([0u16; 256], [0u16; 256]);
    let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
    let mut use_ = SID_NAME_USE::default();
    // SAFETY: buffers and lengths are valid; `sid` is valid (the caller's contract).
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
    Some((
        String::from_utf16_lossy(domain.get(..domain_len as usize)?),
        String::from_utf16_lossy(name.get(..name_len as usize)?),
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
