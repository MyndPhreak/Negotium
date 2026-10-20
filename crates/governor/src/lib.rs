//! Process Governor engine for Negotium.
//!
//! Provides intelligent, automatic process restraint (ProBalance) to preserve system
//! responsiveness during high load, as well as persistent per-process affinity, CPU sets,
//! priority class, and efficiency mode enforcement.

mod actions;
mod config;
mod engine;
mod exclusions;

pub use actions::{GovernorAction, GovernorEvent, RestoreReason};
pub use config::{GovernorConfig, PersistentRule, RestraintMode};
pub use engine::{Governor, RestrainedProcess};
pub use exclusions::{CRITICAL_SYSTEM_PROCESSES, is_safety_excluded, is_user_excluded};
