//! The user the admin task is for, by SID. Read from this process's token: after a UAC prompt an
//! administrator's elevated token is the same user (the same SID) as their filtered one, so the
//! elevated `--integrate admin-on` names the task after the user who asked for it. (A standard
//! user who elevates with another account's credentials runs as that other account; the task is
//! then that account's, as Windows would see it too.)
use crate::{ActionError, OwnedHandle};
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PWSTR;

/// A string SID's shape: `S-1-<authority>-<sub>…`, digits between dashes only (so it is safe as a
/// task name: no separators or wildcards).
pub fn is_string_sid(sid: &str) -> bool {
    sid.strip_prefix("S-1-").is_some_and(|rest| {
        rest.split('-').count() >= 2
            && rest.split('-').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// This process's user SID as a string (`S-1-5-21-…-1001`).
pub fn current_user_sid() -> Result<String, ActionError> {
    let win = |e: windows::core::Error| ActionError::from_win(&e);
    let mut token = HANDLE::default();
    // SAFETY: opens our own token for query; wrapped in OwnedHandle below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(win)?;
    let token = OwnedHandle(token);
    let mut needed = 0u32;
    // SAFETY: size query with a null buffer; fails with ERROR_INSUFFICIENT_BUFFER and sets `needed`.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
    if (needed as usize) < size_of::<TOKEN_USER>() {
        return Err(ActionError::Other("the process token has no user".into()));
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    let capacity = (buf.len() * 8) as u32;
    // SAFETY: `buf` is 8-byte aligned and `capacity` bytes long.
    unsafe { GetTokenInformation(token.0, TokenUser, Some(buf.as_mut_ptr().cast()), capacity, &mut needed) }
        .map_err(win)?;
    // SAFETY: the buffer now holds a TOKEN_USER (at least that many bytes, checked above); its
    // SID pointer refers into the same buffer, which outlives its use below.
    let sid = unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid };
    let mut text = PWSTR::null();
    // SAFETY: `sid` is a valid SID in `buf`; `text` receives a LocalAlloc'd string freed below.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(win)?;
    // SAFETY: `text` is a valid NUL-terminated string until freed.
    let out = unsafe { text.to_string() };
    // SAFETY: allocated by ConvertSidToStringSidW with LocalAlloc; freed exactly once.
    unsafe { LocalFree(Some(HLOCAL(text.0 as *mut c_void))) };
    out.map_err(|_| ActionError::Other("the SID is not valid text".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sids_have_their_shape() {
        for good in ["S-1-5-21-1004336348-1177238915-682003330-1001", "S-1-12-1-1-2-3-4", "S-1-5-18"] {
            assert!(is_string_sid(good), "{good}");
        }
        for bad in ["", "S-1-", "S-1-5", "S-1-5-", "S-1-5--1", "s-1-5-21", "S-1-5-21-x", "S-1-5-21\\..", "S-1-5-21 1", "S-1-5-*"] {
            assert!(!is_string_sid(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_current_user_has_a_user_sid() {
        // A read of this process's own token: nothing is changed.
        let sid = current_user_sid().unwrap();
        assert!(is_string_sid(&sid), "{sid}");
        assert!(sid.starts_with("S-1-5-") || sid.starts_with("S-1-12-"), "{sid}");
        assert_eq!(current_user_sid().unwrap(), sid, "stable");
    }
}
