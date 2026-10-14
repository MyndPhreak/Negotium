use negotium_collector::scan_windows;
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WINDOW_EX_STYLE,
    WS_POPUP, WS_VISIBLE,
};
use windows::core::{HSTRING, w};

#[test]
fn detects_a_window_that_stops_pumping_messages() {
    let title = format!("negotium-hang-test-{}", std::process::id());
    let thread_title = title.clone();
    std::thread::spawn(move || {
        // SAFETY: creates a plain STATIC window owned by this thread.
        let _hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0), w!("STATIC"), &HSTRING::from(thread_title.as_str()),
                WS_POPUP | WS_VISIBLE, -32000, -32000, 100, 100, None, None, None, None,
            )
        }
        .expect("create window");
        let pump_until = Instant::now() + Duration::from_millis(500);
        let mut msg = MSG::default();
        while Instant::now() < pump_until {
            // SAFETY: standard message pump on the owning thread.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_secs(12)); // stop pumping: the window goes "Not responding"
    });

    let deadline = Instant::now() + Duration::from_secs(11);
    let mut seen_responsive = false;
    loop {
        let started = Instant::now();
        let windows = scan_windows();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "scan_windows() must never block on a hung window; one call took {elapsed:?}"
        );
        let found = windows.into_iter().find(|w| w.title == title);
        if let Some(w) = found {
            assert_eq!(w.pid, std::process::id());
            if !w.hung {
                seen_responsive = true;
            } else {
                assert!(seen_responsive, "window should be responsive before it hangs");
                return;
            }
        }
        assert!(Instant::now() < deadline, "window never reported hung");
        std::thread::sleep(Duration::from_millis(100));
    }
}
