//! Negotium GUI: pure view building (format, view), the supervised worker, and Slint glue.
pub mod controls;
pub mod format;
pub mod log;
pub mod view;
pub mod worker;

slint::include_modules!();
