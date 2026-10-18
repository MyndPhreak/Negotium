//! Negotium GUI: pure view building (format, view), the supervised worker, and Slint glue.
pub mod appdata;
pub mod bridge;
pub mod controls;
pub mod dispatch;
pub mod format;
pub mod graph;
pub mod icons;
pub mod install;
pub mod integrate;
pub mod framestats;
pub mod log;
pub mod nav;
pub mod perf;
pub mod platform;
pub mod renderer;
pub mod scrub;
pub mod settings;
pub mod settingspage;
pub mod smooth;
pub mod startup;
pub mod tray;
pub mod trayicon;
pub mod update;
pub mod view;
pub mod worker;

slint::include_modules!();
