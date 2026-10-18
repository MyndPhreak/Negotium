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
use windows::Win32::Security::{
    GetTokenInformation, PSID, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER, TokenGroups, TokenUser,
};
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

/// A user account's SID: a local or domain account (`S-1-5-21-<domain>-<domain>-<domain>-<rid>`)
/// or a Microsoft Entra ID account (`S-1-12-1-<four parts>`), in `is_string_sid`'s shape. Service
/// and well-known SIDs (SYSTEM `S-1-5-18`, LOCAL SERVICE `-19`, NETWORK SERVICE `-20`, groups,
/// `S-1-5-21-…` without a RID) are refused: an installer step running as SYSTEM must fail, not
/// register a task or a Run value for SYSTEM.
pub fn is_account_sid(sid: &str) -> bool {
    let parts = |prefix: &str| sid.strip_prefix(prefix).map(|rest| rest.split('-').count());
    is_string_sid(sid) && (parts("S-1-5-21-").is_some_and(|n| n >= 4) || parts("S-1-12-1-").is_some_and(|n| n >= 4))
}

/// The Administrators group's SID.
pub const ADMINISTRATORS: &str = "S-1-5-32-544";

/// This process's token information of `class`, in an 8-byte aligned buffer at least `min` bytes
/// long.
fn token_info(class: TOKEN_INFORMATION_CLASS, min: usize) -> Result<Vec<u64>, ActionError> {
    let win = |e: windows::core::Error| ActionError::from_win(&e);
    let mut token = HANDLE::default();
    // SAFETY: opens our own token for query; wrapped in OwnedHandle below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(win)?;
    let token = OwnedHandle(token);
    let mut needed = 0u32;
    // SAFETY: size query with a null buffer; fails with ERROR_INSUFFICIENT_BUFFER and sets `needed`.
    let _ = unsafe { GetTokenInformation(token.0, class, None, 0, &mut needed) };
    if (needed as usize) < min {
        return Err(ActionError::Other("the process token could not be read".into()));
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    let capacity = (buf.len() * 8) as u32;
    // SAFETY: `buf` is 8-byte aligned and `capacity` bytes long.
    unsafe { GetTokenInformation(token.0, class, Some(buf.as_mut_ptr().cast()), capacity, &mut needed) }
        .map_err(win)?;
    Ok(buf)
}

/// `sid` as a string (`S-1-5-21-…`).
///
/// # Safety
/// `sid` must point at a valid SID.
unsafe fn sid_string(sid: PSID) -> Result<String, ActionError> {
    let mut text = PWSTR::null();
    // SAFETY: `sid` is valid (the caller's contract); `text` receives a LocalAlloc'd string freed below.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(|e| ActionError::from_win(&e))?;
    // SAFETY: `text` is a valid NUL-terminated string until freed.
    let out = unsafe { text.to_string() };
    // SAFETY: allocated by ConvertSidToStringSidW with LocalAlloc; freed exactly once.
    unsafe { LocalFree(Some(HLOCAL(text.0 as *mut c_void))) };
    out.map_err(|_| ActionError::Other("the SID is not valid text".into()))
}

/// This process's user SID as a string (`S-1-5-21-…-1001`).
pub fn current_user_sid() -> Result<String, ActionError> {
    let buf = token_info(TokenUser, size_of::<TOKEN_USER>())?;
    // SAFETY: the buffer holds a TOKEN_USER (at least that many bytes, checked by `token_info`);
    // its SID pointer refers into the same buffer, which outlives its use below.
    let sid = unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid };
    // SAFETY: `sid` is the token's valid user SID in `buf`.
    unsafe { sid_string(sid) }
}

/// The groups in this process's token, as SID strings, whatever their attributes: an
/// administrator's filtered (non-elevated) token still lists Administrators, as deny-only.
fn token_groups() -> Result<Vec<String>, ActionError> {
    let buf = token_info(TokenGroups, size_of::<TOKEN_GROUPS>())?;
    let groups = buf.as_ptr() as *const TOKEN_GROUPS;
    // SAFETY: the buffer holds a TOKEN_GROUPS whose `GroupCount` entries follow in the same
    // buffer (Windows sized it so); the SID pointers refer into it, and it outlives their use.
    let entries = unsafe { std::slice::from_raw_parts((*groups).Groups.as_ptr(), (*groups).GroupCount as usize) };
    // SAFETY: each entry's SID is valid (see above).
    entries.iter().map(|g| unsafe { sid_string(g.Sid) }).collect()
}

/// Whether this process's user is in the Administrators group, elevated or not (admin mode is
/// offered only then). An unreadable token counts as not.
pub fn is_admin_member() -> bool {
    token_groups().is_ok_and(|groups| groups.iter().any(|g| g == ADMINISTRATORS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sids_have_their_shape() {
        // The shape only: well-known SIDs have it too (`is_account_sid` refuses them).
        for good in ["S-1-5-21-1004336348-1177238915-682003330-1001", "S-1-12-1-1-2-3-4", "S-1-5-18"] {
            assert!(is_string_sid(good), "{good}");
        }
        for bad in ["", "S-1-", "S-1-5", "S-1-5-", "S-1-5--1", "s-1-5-21", "S-1-5-21-x", "S-1-5-21\\..", "S-1-5-21 1", "S-1-5-*"] {
            assert!(!is_string_sid(bad), "{bad:?}");
            assert!(!is_account_sid(bad), "{bad:?}");
        }
    }

    #[test]
    fn only_user_accounts_sids_are_account_sids() {
        for good in [
            "S-1-5-21-1004336348-1177238915-682003330-1001",
            "S-1-5-21-1004336348-1177238915-682003330-500",
            "S-1-12-1-1885673120-1143434361-2093374368-2546219385",
        ] {
            assert!(is_account_sid(good), "{good}");
        }
        for bad in [
            "S-1-5-18",                                // SYSTEM
            "S-1-5-19",                                // LOCAL SERVICE
            "S-1-5-20",                                // NETWORK SERVICE
            "S-1-5-32-544",                            // Administrators
            "S-1-5-11",                                // Authenticated Users
            "S-1-1-0",                                 // Everyone
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464", // a service
            "S-1-5-21-1004336348-1177238915-682003330", // a domain, no RID
            "S-1-12-1-1-2-3",
            "S-1-5-21",
            "S-1-5-21-1-2-3-4-x",
        ] {
            assert!(!is_account_sid(bad), "{bad}");
        }
    }

    #[test]
    fn the_current_user_has_a_user_sid() {
        // A read of this process's own token: nothing is changed. Tests run as a signed-in user.
        let sid = current_user_sid().unwrap();
        assert!(is_account_sid(&sid), "{sid}");
        assert_eq!(current_user_sid().unwrap(), sid, "stable");
    }

    /// Whatever this account is, an elevated token is an administrator's, and every token lists
    /// Everyone among its groups.
    #[test]
    fn this_tokens_groups_are_read() {
        let groups = token_groups().unwrap();
        assert!(groups.iter().any(|g| g == "S-1-1-0"), "{groups:?}");
        assert!(groups.iter().all(|g| is_string_sid(g)), "{groups:?}");
        if crate::is_elevated() {
            assert!(is_admin_member());
        }
    }
}
