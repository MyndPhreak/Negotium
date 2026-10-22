//! The built `negotium.exe` carries an application manifest (RT_MANIFEST, id 1). It binds
//! Common Controls 6, which has `TaskDialogIndirect`, and says the exe supports Windows 10/11,
//! so Windows does not apply Vista-era compatibility behaviour to it.
use windows::Win32::Foundation::FreeLibrary;
use windows::Win32::System::LibraryLoader::{
    FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW, LoadResource,
    LockResource, SizeofResource,
};
use windows::Win32::UI::WindowsAndMessaging::RT_MANIFEST;
use windows::core::{HSTRING, PCWSTR};

/// The exe's manifest resource as text, or `None` when it has none.
fn exe_manifest() -> Option<String> {
    let exe = HSTRING::from(env!("CARGO_BIN_EXE_negotium"));
    // SAFETY: the exe is mapped as data only (none of its code runs) and freed before returning;
    // the resource bytes are copied out while the module is loaded.
    unsafe {
        let module =
            LoadLibraryExW(&exe, None, LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE).expect("map negotium.exe");
        let found = FindResourceW(Some(module), PCWSTR(1 as *const u16), RT_MANIFEST);
        let text = (!found.is_invalid()).then(|| {
            let data = LoadResource(Some(module), found).expect("load the manifest");
            let bytes = std::slice::from_raw_parts(LockResource(data) as *const u8, SizeofResource(Some(module), found) as usize);
            String::from_utf8_lossy(bytes).into_owned()
        });
        let _ = FreeLibrary(module);
        text
    }
}

#[test]
fn exe_manifest_asks_for_common_controls_6_on_windows_10() {
    let manifest = exe_manifest().expect("negotium.exe has no RT_MANIFEST resource with id 1");
    for want in ["Microsoft.Windows.Common-Controls", "6.0.0.0", "{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"] {
        assert!(manifest.contains(want), "the manifest has no {want}:\n{manifest}");
    }
}
