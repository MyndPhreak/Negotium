//! Moderator engine for Negotium.
//!
//! Provides intelligent, automatic process restraint (Aequilibrium) to preserve system
//! responsiveness during high load, as well as persistent per-process affinity, CPU sets,
//! priority class, and efficiency mode enforcement.

mod actions;
mod config;
mod engine;
mod exclusions;

pub use actions::{ModeratorAction, ModeratorEvent, RestoreReason};
pub use config::{ModeratorConfig, PersistentRule, RestraintMode};
pub use engine::{Moderator, RestrainedProcess};
pub use exclusions::{image_stem, is_safety_excluded, is_user_excluded, same_image, CRITICAL_SYSTEM_PROCESSES};
