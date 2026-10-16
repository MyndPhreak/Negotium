//! Windows data sources. The only crate besides `negotium-bench` that calls Windows APIs.
pub mod gpu;
pub mod identity;
pub mod memory;
pub mod net;
pub mod pdh;
pub mod processes;
pub mod sysinfo;

pub use identity::{Identity, IdentityCache, VersionInfo, version_string};
pub use processes::{ProcessReader, RawProcess};
pub mod windows_scan;

pub use windows_scan::scan_windows;

pub use memory::{local_time_ms, logical_cpus, memory_totals, processor_groups, uptime_s};
pub use sysinfo::{SystemInfo, read_system_info};
pub use pdh::{PdhReadings, PdhSources};

mod collector;

pub use collector::Collector;
