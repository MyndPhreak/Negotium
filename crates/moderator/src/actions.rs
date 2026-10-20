use crate::config::RestraintMode;
use negotium_actions::PriorityClass;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreReason {
    CpuDropped,
    BecameForeground,
    ProcessExited,
    /// Negotium is exiting or its worker is restarting: nothing will be left to restore it later.
    Shutdown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModeratorAction {
    Restrain {
        pid: u32,
        create_time: i64,
        name: String,
        mode: RestraintMode,
    },
    Restore {
        pid: u32,
        create_time: i64,
        name: String,
        reason: RestoreReason,
        original_priority: Option<PriorityClass>,
        original_efficiency: Option<bool>,
    },
    ApplyRule {
        pid: u32,
        create_time: i64,
        name: String,
        priority: Option<PriorityClass>,
        affinity_mask: Option<u64>,
        cpu_sets: Option<Vec<u32>>,
        efficiency_mode: Option<bool>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModeratorEvent {
    RestraintStarted {
        pid: u32,
        name: String,
        cpu_pct: f32,
        mode: RestraintMode,
    },
    RestraintRestored {
        pid: u32,
        name: String,
        reason: RestoreReason,
        duration_ms: u64,
    },
    RuleApplied {
        pid: u32,
        name: String,
        summary: String,
    },
}
