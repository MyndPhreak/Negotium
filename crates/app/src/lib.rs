//! Negotium GUI: pure view building (format, view), the supervised worker, and Slint glue.
pub mod bridge;
pub mod controls;
pub mod dispatch;
pub mod format;
pub mod graph;
pub mod framestats;
pub mod log;
pub mod perf;
pub mod platform;
pub mod scrub;
pub mod view;
pub mod worker;

slint::include_modules!();
