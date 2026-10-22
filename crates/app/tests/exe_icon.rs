//! The built `negotium.exe` carries its own icon. Task Manager, Explorer and shortcuts read the
//! icon from the file's resources, not from the window (which Slint sets from the SVG at runtime).
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::core::HSTRING;

#[test]
fn exe_embeds_an_icon() {
    let exe = HSTRING::from(env!("CARGO_BIN_EXE_negotium"));
    // A negative index with no output arrays returns how many icons the file holds.
    let count = unsafe { ExtractIconExW(&exe, -1, None, None, 0) };
    assert!(count > 0, "negotium.exe has no icon resource");
}
