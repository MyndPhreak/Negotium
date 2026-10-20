use negotium_actions::PriorityClass;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestraintMode {
    /// Both Windows 11 EcoQoS (Execution Speed throttling) and BelowNormal priority class.
    #[default]
    Both,
    /// Only EcoQoS (Power Throttling execution speed), leaves base priority untouched.
    EcoQosOnly,
    /// Only BelowNormal priority class, leaves power throttling untouched.
    BelowNormalOnly,
    /// Idle priority class and EcoQoS.
    Idle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PersistentRule {
    /// Target image name, e.g. "game.exe" (case-insensitive).
    pub name: String,
    #[serde(default)]
    pub priority: Option<PriorityClass>,
    #[serde(default)]
    pub affinity_mask: Option<u64>,
    #[serde(default)]
    pub cpu_sets: Option<Vec<u32>>,
    #[serde(default)]
    pub efficiency_mode: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GovernorConfig {
    /// Whether automatic runaway CPU restraint (ProBalance) is active.
    pub probalance_enabled: bool,
    /// Minimum total system CPU % before ProBalance engages restraint (default: 70%).
    pub system_cpu_threshold_pct: f32,
    /// Minimum individual process CPU % to qualify for restraint (default: 20%).
    pub process_cpu_threshold_pct: f32,
    /// Process CPU % below which cooldown starts towards restoration (default: 10%).
    pub restore_cpu_threshold_pct: f32,
    /// Consecutive time a process must remain above threshold before restraint begins (ms, default: 1000).
    pub time_over_quota_ms: u64,
    /// Minimum duration a process remains restrained before restoration can occur (ms, default: 4000).
    pub min_restraint_duration_ms: u64,
    /// Restraint technique applied to restrained processes.
    pub restraint_mode: RestraintMode,
    /// User-defined process exclusions from ProBalance (e.g. "obs64.exe").
    pub exclusions: Vec<String>,
    /// Persistent process configuration rules (priority, affinity, cpu sets, efficiency mode).
    pub rules: Vec<PersistentRule>,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            probalance_enabled: true,
            system_cpu_threshold_pct: 70.0,
            process_cpu_threshold_pct: 20.0,
            restore_cpu_threshold_pct: 10.0,
            time_over_quota_ms: 1000,
            min_restraint_duration_ms: 4000,
            restraint_mode: RestraintMode::Both,
            exclusions: Vec::new(),
            rules: Vec::new(),
        }
    }
}
