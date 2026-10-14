//! Windows data sources. The only crate besides `negotium-bench` that calls Windows APIs.
pub mod identity;
pub mod processes;

pub use identity::{Identity, IdentityCache};
pub use processes::{ProcessReader, RawProcess};
pub mod windows_scan;

pub use windows_scan::scan_windows;
