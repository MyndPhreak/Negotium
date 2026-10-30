//! Facts about an executable file: its version and who signed it.
use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use windows::Win32::Foundation::{
    HANDLE, HWND, INVALID_HANDLE_VALUE, TRUST_E_NOSIGNATURE, TRUST_E_PROVIDER_UNKNOWN, TRUST_E_SUBJECT_FORM_UNKNOWN,
};
use windows::Win32::Security::Cryptography::Catalog::{
    CATALOG_INFO, CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash,
    CryptCATAdminReleaseCatalogContext, CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext,
};
use windows::Win32::Security::Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
use windows::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_DATA_UNION_CHOICE,
    WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE,
    WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain,
    WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW};
use windows::core::{HSTRING, PCWSTR, w};

/// Who signed a file, as Windows checks it (no revocation check: offline-safe and fast).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signature {
    /// The leaf signer's display name, e.g. "Microsoft Windows".
    Signed(String),
    NotSigned,
    /// The file could not be read, or the check failed for another reason.
    Unknown,
}

/// The file version from the version resource, e.g. "10.0.26100.1".
pub fn file_version(path: &str) -> Option<String> {
    let path = HSTRING::from(path);
    // SAFETY: plain Win32 size query.
    let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` is `size` writable bytes.
    unsafe { GetFileVersionInfoW(&path, None, size, buf.as_mut_ptr().cast()) }.ok()?;
    let (mut info, mut len) = (std::ptr::null_mut::<c_void>(), 0u32);
    // SAFETY: `buf` holds the version block; the result points into it.
    let found = unsafe { VerQueryValueW(buf.as_ptr().cast(), w!("\\"), &mut info, &mut len) }.as_bool();
    if !found || info.is_null() || (len as usize) < size_of::<VS_FIXEDFILEINFO>() {
        return None;
    }
    // SAFETY: checked non-null and at least a VS_FIXEDFILEINFO long, inside `buf`, which is alive.
    let f = unsafe { std::ptr::read_unaligned(info as *const VS_FIXEDFILEINFO) };
    Some(format!(
        "{}.{}.{}.{}",
        f.dwFileVersionMS >> 16,
        f.dwFileVersionMS & 0xffff,
        f.dwFileVersionLS >> 16,
        f.dwFileVersionLS & 0xffff
    ))
}

/// Checks the embedded Authenticode signature, then the system catalogs. Blocking (it can read
/// the whole file); call off the UI thread.
pub fn signature(path: &str) -> Signature {
    let Ok(file) = std::fs::File::open(path) else { return Signature::Unknown };
    let wide = HSTRING::from(path);
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        ..Default::default()
    };
    let mut data = trust_data(WTD_CHOICE_FILE);
    data.Anonymous.pFile = &mut info;
    match verify(&mut data) {
        Verified::Signed(name) => Signature::Signed(name),
        Verified::NoSignature => catalog_signature(&file, &wide),
        Verified::Failed => Signature::NotSigned,
    }
}

enum Verified {
    Signed(String),
    /// No embedded signature (the file may still be catalog-signed).
    NoSignature,
    /// A signature that does not verify (untrusted root, changed file).
    Failed,
}

fn trust_data(choice: WINTRUST_DATA_UNION_CHOICE) -> WINTRUST_DATA {
    WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: choice,
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..Default::default()
    }
}

/// Runs WinVerifyTrust on `data`, reads the signer on success, and always closes the state.
fn verify(data: &mut WINTRUST_DATA) -> Verified {
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let no_ui = HWND(INVALID_HANDLE_VALUE.0);
    // SAFETY: `data` is a fully initialised WINTRUST_DATA whose union points at live structs.
    let status = unsafe { WinVerifyTrust(no_ui, &mut action, (data as *mut WINTRUST_DATA).cast()) };
    let unsigned = [TRUST_E_NOSIGNATURE, TRUST_E_SUBJECT_FORM_UNKNOWN, TRUST_E_PROVIDER_UNKNOWN];
    let result = if status == 0 {
        Verified::Signed(signer_name(data.hWVTStateData).unwrap_or_default())
    } else if unsigned.iter().any(|e| e.0 == status) {
        Verified::NoSignature
    } else {
        Verified::Failed
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: closes the state the call above opened; same structs, still alive.
    unsafe { WinVerifyTrust(no_ui, &mut action, (data as *mut WINTRUST_DATA).cast()) };
    result
}

fn signer_name(state: HANDLE) -> Option<String> {
    // SAFETY: `state` comes from a successful verify that is not closed yet.
    let prov = unsafe { WTHelperProvDataFromStateData(state) };
    if prov.is_null() {
        return None;
    }
    // SAFETY: `prov` is valid while the state is open.
    let sgnr = unsafe { WTHelperGetProvSignerFromChain(prov, 0, false, 0) };
    // SAFETY: null or a signer inside the open state.
    let sgnr = unsafe { sgnr.as_ref() }?;
    if sgnr.csCertChain == 0 || sgnr.pasCertChain.is_null() {
        return None;
    }
    // SAFETY: the chain has at least one entry (checked above); entry 0 is the leaf.
    let cert = unsafe { (*sgnr.pasCertChain).pCert };
    if cert.is_null() {
        return None;
    }
    let mut buf = [0u16; 256];
    // SAFETY: `cert` is a live CERT_CONTEXT; `buf` is writable.
    let len = unsafe { CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut buf)) } as usize;
    // The length counts the terminating null.
    let name = String::from_utf16_lossy(buf.get(..len.saturating_sub(1))?);
    (!name.is_empty()).then_some(name)
}

struct CatAdmin(isize);

impl Drop for CatAdmin {
    fn drop(&mut self) {
        // SAFETY: a context acquired once and released once here.
        unsafe {
            let _ = CryptCATAdminReleaseContext(self.0, 0);
        }
    }
}

/// Looks the file's hash up in the system catalogs (SHA-256, then SHA-1) and verifies the member.
fn catalog_signature(file: &std::fs::File, path: &HSTRING) -> Signature {
    let handle = HANDLE(file.as_raw_handle());
    for algorithm in [w!("SHA256"), PCWSTR::null()] {
        let mut admin = 0isize;
        // SAFETY: `admin` is writable; released by CatAdmin.
        if unsafe { CryptCATAdminAcquireContext2(&mut admin, None, algorithm, None, None) }.is_err() {
            continue;
        }
        let admin = CatAdmin(admin);
        let mut len = 0u32;
        // SAFETY: size query with no buffer.
        let _ = unsafe { CryptCATAdminCalcHashFromFileHandle2(admin.0, handle, &mut len, None, None) };
        if len == 0 {
            continue;
        }
        let mut hash = vec![0u8; len as usize];
        // SAFETY: `hash` is `len` writable bytes.
        let hashed = unsafe { CryptCATAdminCalcHashFromFileHandle2(admin.0, handle, &mut len, Some(hash.as_mut_ptr()), None) };
        if hashed.is_err() {
            continue;
        }
        // SAFETY: valid context and hash.
        let cat = unsafe { CryptCATAdminEnumCatalogFromHash(admin.0, &hash, None, None) };
        if cat == 0 {
            continue;
        }
        let mut info = CATALOG_INFO { cbStruct: size_of::<CATALOG_INFO>() as u32, ..Default::default() };
        // SAFETY: `cat` is a live catalog context; `info` is writable.
        let result = if unsafe { CryptCATCatalogInfoFromContext(cat, &mut info, 0) }.is_ok() {
            let tag = HSTRING::from(hash.iter().map(|b| format!("{b:02X}")).collect::<String>());
            let mut member = WINTRUST_CATALOG_INFO {
                cbStruct: size_of::<WINTRUST_CATALOG_INFO>() as u32,
                pcwszCatalogFilePath: PCWSTR(info.wszCatalogFile.as_ptr()),
                pcwszMemberTag: PCWSTR(tag.as_ptr()),
                pcwszMemberFilePath: PCWSTR(path.as_ptr()),
                hMemberFile: handle,
                pbCalculatedFileHash: hash.as_mut_ptr(),
                cbCalculatedFileHash: len,
                hCatAdmin: admin.0,
                ..Default::default()
            };
            let mut data = trust_data(WTD_CHOICE_CATALOG);
            data.Anonymous.pCatalog = &mut member;
            match verify(&mut data) {
                Verified::Signed(name) => Signature::Signed(name),
                _ => Signature::NotSigned,
            }
        } else {
            Signature::NotSigned
        };
        // SAFETY: releases the context EnumCatalogFromHash returned, once.
        unsafe {
            let _ = CryptCATAdminReleaseCatalogContext(admin.0, cat, 0);
        }
        return result;
    }
    Signature::NotSigned
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notepad() -> String {
        format!(r"{}\System32\notepad.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
    }

    #[test]
    fn notepad_has_a_version() {
        assert!(file_version(&notepad()).is_some_and(|v| v.split('.').count() == 4));
    }

    #[test]
    fn a_catalog_signed_windows_file_is_signed_by_microsoft() {
        assert!(matches!(signature(&notepad()), Signature::Signed(n) if n.contains("Microsoft")));
    }

    #[test]
    fn the_test_binary_is_not_signed() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(signature(exe.to_str().unwrap()), Signature::NotSigned);
    }

    #[test]
    fn a_missing_file_is_unknown() {
        assert_eq!(signature(r"C:\nope\nope.exe"), Signature::Unknown);
        assert_eq!(file_version(r"C:\nope\nope.exe"), None);
    }
}
