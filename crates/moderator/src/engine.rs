use crate::actions::{ModeratorAction, ModeratorEvent, RestoreReason};
use crate::config::{ModeratorConfig, PersistentRule};
use crate::exclusions::{is_safety_excluded, is_user_excluded, same_image};
use negotium_actions::PriorityClass;
use negotium_snapshot::Snapshot;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct RestrainedProcess {
    pub name: String,
    pub restrained_at_ms: u64,
    pub last_high_cpu_ms: u64,
    pub original_priority: Option<PriorityClass>,
    pub original_efficiency: Option<bool>,
}

#[derive(Default)]
pub struct Moderator {
    /// Active restraints: (pid, create_time) -> RestrainedProcess
    restraints: HashMap<(u32, i64), RestrainedProcess>,
    /// Candidates over quota: (pid, create_time) -> first_time_over_quota_ms
    high_cpu_since: HashMap<(u32, i64), u64>,
    /// The persistent rule last applied to each process instance. A rule added or edited while
    /// its process runs differs from the one recorded here, so it is applied to that instance too.
    applied_rules: HashMap<(u32, i64), PersistentRule>,
    /// The latest tick's snapshot time, for restraint durations reported outside a tick.
    now_ms: u64,
}

impl Moderator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluates the current system state against moderator configuration and returns actions and events.
    /// `rates`: per-process CPU usage percentage indexed by (pid, create_time).
    /// `foreground_pid`: PID of the current foreground window (or None).
    pub fn tick(
        &mut self,
        snap: &Snapshot,
        rates: &HashMap<(u32, i64), f32>,
        foreground_pid: Option<u32>,
        config: &ModeratorConfig,
    ) -> (Vec<ModeratorAction>, Vec<ModeratorEvent>) {
        let mut actions = Vec::new();
        let mut events = Vec::new();
        let now_ms = snap.taken_at_ms;
        self.now_ms = now_ms;

        // 1. Build live process lookup and foreground tree
        let live_keys: HashSet<(u32, i64)> = snap.processes.iter().map(|p| (p.pid, p.create_time)).collect();
        let foreground_tree = Self::find_foreground_tree(snap, foreground_pid);

        // 2. Clean up exited processes
        self.high_cpu_since.retain(|k, _| live_keys.contains(k));
        self.applied_rules.retain(|k, _| live_keys.contains(k));

        let mut exited_restraints = Vec::new();
        self.restraints.retain(|k, v| {
            if !live_keys.contains(k) {
                exited_restraints.push((*k, v.clone()));
                false
            } else {
                true
            }
        });

        for ((pid, _create_time), r) in exited_restraints {
            let duration_ms = now_ms.saturating_sub(r.restrained_at_ms);
            events.push(ModeratorEvent::RestraintRestored {
                pid,
                name: r.name,
                reason: RestoreReason::ProcessExited,
                duration_ms,
            });
        }

        // 3. Apply persistent rules to processes that don't yet have their current rule
        for p in &snap.processes {
            let key = (p.pid, p.create_time);
            let Some(rule) = config.rules.iter().find(|r| same_image(&p.name, &r.name)) else {
                self.applied_rules.remove(&key);
                continue;
            };
            if self.applied_rules.get(&key) != Some(rule) {
                let mut summary_parts = Vec::new();
                if let Some(prio) = rule.priority {
                    summary_parts.push(format!("Priority: {}", prio.display_name()));
                }
                if let Some(mask) = rule.affinity_mask {
                    summary_parts.push(format!("Affinity: {mask:#x}"));
                }
                if let Some(sets) = &rule.cpu_sets {
                    summary_parts.push(format!("CpuSets: {sets:?}"));
                }
                if let Some(eff) = rule.efficiency_mode {
                    summary_parts.push(format!("Efficiency: {eff}"));
                }

                actions.push(ModeratorAction::ApplyRule {
                    pid: p.pid,
                    create_time: p.create_time,
                    name: p.name.clone(),
                    priority: rule.priority,
                    affinity_mask: rule.affinity_mask,
                    cpu_sets: rule.cpu_sets.clone(),
                    efficiency_mode: rule.efficiency_mode,
                });

                events.push(ModeratorEvent::RuleApplied {
                    pid: p.pid,
                    name: p.name.clone(),
                    summary: summary_parts.join(", "),
                });
                self.applied_rules.insert(key, rule.clone());
            }
        }

        // 4. If Aequilibrium is disabled, restore any actively restrained processes and return
        if !config.aequilibrium_enabled {
            let (restores, restored) = self.restore_all(RestoreReason::CpuDropped);
            actions.extend(restores);
            events.extend(restored);
            return (actions, events);
        }

        // 5. Evaluate existing restraints for restoration
        let mut to_restore = Vec::new();
        for (key, r) in &mut self.restraints {
            let &(pid, _create_time) = key;
            // Check if process became foreground or child of foreground
            if foreground_tree.contains(&pid) {
                to_restore.push((*key, RestoreReason::BecameForeground));
                continue;
            }

            let cpu = rates.get(key).copied().unwrap_or(0.0);
            if cpu >= config.restore_cpu_threshold_pct {
                r.last_high_cpu_ms = now_ms;
            } else {
                // Process is below restore threshold; check cooldown duration
                let restrained_for = now_ms.saturating_sub(r.restrained_at_ms);
                let cool_for = now_ms.saturating_sub(r.last_high_cpu_ms);
                if restrained_for >= config.min_restraint_duration_ms && cool_for >= config.min_restraint_duration_ms {
                    to_restore.push((*key, RestoreReason::CpuDropped));
                }
            }
        }

        for (key, reason) in to_restore {
            if let Some(r) = self.restraints.remove(&key) {
                let duration_ms = now_ms.saturating_sub(r.restrained_at_ms);
                actions.push(ModeratorAction::Restore {
                    pid: key.0,
                    create_time: key.1,
                    name: r.name.clone(),
                    reason,
                    original_priority: r.original_priority,
                    original_efficiency: r.original_efficiency,
                });
                events.push(ModeratorEvent::RestraintRestored {
                    pid: key.0,
                    name: r.name,
                    reason,
                    duration_ms,
                });
            }
        }

        // 6. Evaluate potential candidates for new restraint
        // Only trigger new restraints if total system CPU is at or above threshold
        let system_under_pressure = snap.totals.cpu_pct.unwrap_or(0.0) >= config.system_cpu_threshold_pct;

        for p in &snap.processes {
            let key = (p.pid, p.create_time);
            if self.restraints.contains_key(&key) {
                continue;
            }
            if foreground_tree.contains(&p.pid) {
                self.high_cpu_since.remove(&key);
                continue;
            }
            if is_safety_excluded(&p.name) || is_user_excluded(&p.name, &config.exclusions) {
                self.high_cpu_since.remove(&key);
                continue;
            }

            let cpu = rates.get(&key).copied().unwrap_or(0.0);
            if system_under_pressure && cpu >= config.process_cpu_threshold_pct {
                let first_seen = *self.high_cpu_since.entry(key).or_insert(now_ms);
                if now_ms.saturating_sub(first_seen) >= config.time_over_quota_ms {
                    // Ready to restrain!
                    actions.push(ModeratorAction::Restrain {
                        pid: p.pid,
                        create_time: p.create_time,
                        name: p.name.clone(),
                        mode: config.restraint_mode,
                    });
                    events.push(ModeratorEvent::RestraintStarted {
                        pid: p.pid,
                        name: p.name.clone(),
                        cpu_pct: cpu,
                        mode: config.restraint_mode,
                    });
                    self.restraints.insert(
                        key,
                        RestrainedProcess {
                            name: p.name.clone(),
                            restrained_at_ms: now_ms,
                            last_high_cpu_ms: now_ms,
                            original_priority: None,
                            original_efficiency: None,
                        },
                    );
                    self.high_cpu_since.remove(&key);
                }
            } else {
                self.high_cpu_since.remove(&key);
            }
        }

        (actions, events)
    }

    /// Ends every active restraint, returning the Restore actions to carry out. Used when
    /// Aequilibrium is turned off and when the worker shuts down or restarts, since a restraint
    /// left behind then would outlive Negotium.
    pub fn restore_all(&mut self, reason: RestoreReason) -> (Vec<ModeratorAction>, Vec<ModeratorEvent>) {
        let mut actions = Vec::new();
        let mut events = Vec::new();
        for ((pid, create_time), r) in self.restraints.drain() {
            let duration_ms = self.now_ms.saturating_sub(r.restrained_at_ms);
            actions.push(ModeratorAction::Restore {
                pid,
                create_time,
                name: r.name.clone(),
                reason,
                original_priority: r.original_priority,
                original_efficiency: r.original_efficiency,
            });
            events.push(ModeratorEvent::RestraintRestored { pid, name: r.name, reason, duration_ms });
        }
        self.high_cpu_since.clear();
        (actions, events)
    }

    /// Records the original priority and efficiency state read before applying restraint,
    /// so restoration can reliably restore the exact prior state. Either is `None` when it
    /// could not be read; the caller then leaves that setting alone.
    pub fn record_original_state(
        &mut self,
        pid: u32,
        create_time: i64,
        priority: Option<PriorityClass>,
        efficiency: Option<bool>,
    ) {
        if let Some(r) = self.restraints.get_mut(&(pid, create_time)) {
            r.original_priority = priority;
            r.original_efficiency = efficiency;
        }
    }

    /// Returns whether a process is currently under active moderator restraint.
    pub fn is_restrained(&self, pid: u32, create_time: i64) -> bool {
        self.restraints.contains_key(&(pid, create_time))
    }

    pub fn restrained_count(&self) -> usize {
        self.restraints.len()
    }

    fn find_foreground_tree(snap: &Snapshot, foreground_pid: Option<u32>) -> HashSet<u32> {
        let mut set = HashSet::new();
        let Some(fg_pid) = foreground_pid else { return set };
        set.insert(fg_pid);
        // A shell host (explorer.exe with the desktop or taskbar focused) is the parent of nearly
        // every app launched from Start or the taskbar; its descendants are not the user's focus.
        if snap.processes.iter().any(|p| p.pid == fg_pid && is_safety_excluded(&p.name)) {
            return set;
        }

        // Include all child descendants of the foreground process
        let mut added = true;
        while added {
            added = false;
            for p in &snap.processes {
                if set.contains(&p.parent_pid) && !set.contains(&p.pid) {
                    set.insert(p.pid);
                    added = true;
                }
            }
        }
        set
    }
}
