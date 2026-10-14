//! Manual-test helper: shows a captioned window that responds for N seconds (default 3),
//! then stops processing messages for 60 s. Click it once it says "Not Responding" to make
//! Windows ghost it; `negotium-probe` should then list `negotium-hang` as NOT RESPONDING.
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::w;

fn main() {
    let responsive: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(3);
    // SAFETY: creates a plain STATIC window owned by this thread.
    let _hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0), w!("STATIC"), w!("negotium-hang (click me once frozen)"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, 480, 200, None, None, None, None,
        )
    }
    .expect("create window");
    let until = Instant::now() + Duration::from_secs(responsive);
    let mut msg = MSG::default();
    while Instant::now() < until {
        // SAFETY: standard message pump on the owning thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_secs(60));
}
