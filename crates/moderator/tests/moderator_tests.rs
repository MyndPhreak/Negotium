use negotium_actions::PriorityClass;
use negotium_moderator::{
    Moderator, ModeratorAction, ModeratorConfig, ModeratorEvent, PersistentRule, RestraintMode,
    RestoreReason,
};
use negotium_snapshot::{ProcessSample, Snapshot, SystemTotals};
use std::collections::HashMap;

fn make_snap(taken_at_ms: u64, sys_cpu: f32, procs: Vec<ProcessSample>) -> Snapshot {
    Snapshot {
        taken_at_ms,
        totals: SystemTotals {
            cpu_pct: Some(sys_cpu),
            ..Default::default()
        },
        processes: procs,
        windows: Vec::new(),
        source_errors: Vec::new(),
    }
}

fn make_proc(pid: u32, parent_pid: u32, create_time: i64, name: &str) -> ProcessSample {
    ProcessSample {
        pid,
        parent_pid,
        create_time,
        name: name.to_string(),
        ..Default::default()
    }
}

#[test]
fn aequilibrium_restrains_runaway_background_process() {
    let mut moderator = Moderator::new();
    let config = ModeratorConfig::default();

    let p = make_proc(100, 1, 1000, "burner.exe");
    let mut rates = HashMap::new();
    rates.insert((100, 1000), 45.0);

    // Tick 1: t=0ms, system CPU 85%
    let snap1 = make_snap(0, 85.0, vec![p.clone()]);
    let (actions, events) = moderator.tick(&snap1, &rates, None, &config);
    assert!(actions.is_empty(), "First tick over quota should only start timer");
    assert!(events.is_empty());
    assert!(!moderator.is_restrained(100, 1000));

    // Tick 2: t=500ms (still under time_over_quota_ms of 1000ms)
    let snap2 = make_snap(500, 85.0, vec![p.clone()]);
    let (actions, _) = moderator.tick(&snap2, &rates, None, &config);
    assert!(actions.is_empty(), "500ms is below 1000ms threshold");

    // Tick 3: t=1000ms (reaches 1000ms)
    let snap3 = make_snap(1000, 85.0, vec![p.clone()]);
    let (actions, events) = moderator.tick(&snap3, &rates, None, &config);
    assert_eq!(
        actions,
        vec![ModeratorAction::Restrain {
            pid: 100,
            create_time: 1000,
            name: "burner.exe".to_string(),
            mode: RestraintMode::Both,
        }]
    );
    assert_eq!(
        events,
        vec![ModeratorEvent::RestraintStarted {
            pid: 100,
            name: "burner.exe".to_string(),
            cpu_pct: 45.0,
            mode: RestraintMode::Both,
        }]
    );
    assert!(moderator.is_restrained(100, 1000));
}

#[test]
fn aequilibrium_exempts_foreground_window_and_children() {
    let mut moderator = Moderator::new();
    let config = ModeratorConfig::default();

    let fg_root = make_proc(200, 1, 1000, "game.exe");
    let fg_child = make_proc(201, 200, 1005, "render_worker.exe");

    let mut rates = HashMap::new();
    rates.insert((200, 1000), 50.0);
    rates.insert((201, 1005), 40.0);

    // Foreground PID is 200
    let snap = make_snap(1500, 95.0, vec![fg_root, fg_child]);
    let (actions, _) = moderator.tick(&snap, &rates, Some(200), &config);

    assert!(actions.is_empty(), "Foreground process and its children must never be restrained");
    assert!(!moderator.is_restrained(200, 1000));
    assert!(!moderator.is_restrained(201, 1005));
}

#[test]
fn aequilibrium_exempts_critical_system_services() {
    let mut moderator = Moderator::new();
    let config = ModeratorConfig::default();

    let audiodg = make_proc(300, 1, 1000, "audiodg.exe");
    let dwm = make_proc(301, 1, 1000, "dwm.exe");
    let csrss = make_proc(302, 1, 1000, "csrss.exe");

    let mut rates = HashMap::new();
    rates.insert((300, 1000), 60.0);
    rates.insert((301, 1000), 50.0);
    rates.insert((302, 1000), 40.0);

    let snap = make_snap(2000, 99.0, vec![audiodg, dwm, csrss]);
    let (actions, _) = moderator.tick(&snap, &rates, None, &config);

    assert!(actions.is_empty(), "Critical system processes must never be restrained");
}

#[test]
fn aequilibrium_immediate_restoration_when_process_becomes_foreground() {
    let mut moderator = Moderator::new();
    let config = ModeratorConfig::default();

    let p = make_proc(100, 1, 1000, "photoshop.exe");
    let mut rates = HashMap::new();
    rates.insert((100, 1000), 50.0);

    // Restrain in background
    let snap1 = make_snap(0, 90.0, vec![p.clone()]);
    moderator.tick(&snap1, &rates, None, &config);
    let snap2 = make_snap(1000, 90.0, vec![p.clone()]);
    moderator.tick(&snap2, &rates, None, &config);
    moderator.record_original_state(100, 1000, Some(PriorityClass::Normal), Some(false));
    assert!(moderator.is_restrained(100, 1000));

    // User switches to photoshop window (becomes foreground PID 100)
    let snap3 = make_snap(1500, 90.0, vec![p.clone()]);
    let (actions, events) = moderator.tick(&snap3, &rates, Some(100), &config);

    assert_eq!(
        actions,
        vec![ModeratorAction::Restore {
            pid: 100,
            create_time: 1000,
            name: "photoshop.exe".to_string(),
            reason: RestoreReason::BecameForeground,
            original_priority: Some(PriorityClass::Normal),
            original_efficiency: Some(false),
        }]
    );
    assert_eq!(
        events,
        vec![ModeratorEvent::RestraintRestored {
            pid: 100,
            name: "photoshop.exe".to_string(),
            reason: RestoreReason::BecameForeground,
            duration_ms: 500,
        }]
    );
    assert!(!moderator.is_restrained(100, 1000));
}

#[test]
fn aequilibrium_cooldown_restoration_when_cpu_drops() {
    let mut moderator = Moderator::new();
    let config = ModeratorConfig::default();

    let p = make_proc(100, 1, 1000, "heavy_job.exe");
    let mut rates = HashMap::new();
    rates.insert((100, 1000), 50.0);

    // Restrain at t=1000
    let snap1 = make_snap(0, 90.0, vec![p.clone()]);
    moderator.tick(&snap1, &rates, None, &config);
    let snap2 = make_snap(1000, 90.0, vec![p.clone()]);
    moderator.tick(&snap2, &rates, None, &config);
    assert!(moderator.is_restrained(100, 1000));

    // CPU drops to 2% at t=2000 (cool down starts, minimum restraint is 4000ms)
    rates.insert((100, 1000), 2.0);
    let snap3 = make_snap(2000, 30.0, vec![p.clone()]);
    let (actions, _) = moderator.tick(&snap3, &rates, None, &config);
    assert!(actions.is_empty(), "Must not restore before minimum duration");

    // At t=4000 (restrained for 3000ms < 4000ms)
    let snap4 = make_snap(4000, 30.0, vec![p.clone()]);
    let (actions, _) = moderator.tick(&snap4, &rates, None, &config);
    assert!(actions.is_empty(), "Still in cooldown");

    // At t=6000 (restrained for 5000ms >= 4000ms AND cooled for 4000ms)
    let snap5 = make_snap(6000, 30.0, vec![p.clone()]);
    let (actions, events) = moderator.tick(&snap5, &rates, None, &config);
    assert_eq!(actions.len(), 1);
    assert_eq!(events.len(), 1);
    assert!(!moderator.is_restrained(100, 1000));
}

#[test]
fn persistent_rules_applied_once_per_process_instance() {
    let mut moderator = Moderator::new();
    let mut config = ModeratorConfig::default();
    config.rules.push(PersistentRule {
        name: "starcitizen.exe".to_string(),
        priority: Some(PriorityClass::High),
        affinity_mask: Some(0xFFFF),
        cpu_sets: None,
        efficiency_mode: Some(false),
    });

    let sc = make_proc(500, 1, 1000, "starcitizen.exe");
    let rates = HashMap::new();

    // First tick: rule is applied
    let snap1 = make_snap(0, 20.0, vec![sc.clone()]);
    let (actions, events) = moderator.tick(&snap1, &rates, None, &config);
    assert_eq!(
        actions,
        vec![ModeratorAction::ApplyRule {
            pid: 500,
            create_time: 1000,
            name: "starcitizen.exe".to_string(),
            priority: Some(PriorityClass::High),
            affinity_mask: Some(0xFFFF),
            cpu_sets: None,
            efficiency_mode: Some(false),
        }]
    );
    assert_eq!(events.len(), 1);

    // Second tick: rule must NOT be applied again
    let snap2 = make_snap(1000, 20.0, vec![sc.clone()]);
    let (actions2, events2) = moderator.tick(&snap2, &rates, None, &config);
    assert!(actions2.is_empty());
    assert!(events2.is_empty());

    // Process exits and restarts with new create_time 2000
    let sc_new = make_proc(500, 1, 2000, "starcitizen.exe");
    let snap3 = make_snap(2000, 20.0, vec![sc_new]);
    let (actions3, events3) = moderator.tick(&snap3, &rates, None, &config);
    assert_eq!(actions3.len(), 1, "New process instance should have rule applied");
    assert_eq!(events3.len(), 1);
}

#[test]
fn persistent_rule_added_while_process_runs_is_applied() {
    let mut moderator = Moderator::new();
    let mut config = ModeratorConfig::default();
    let game = make_proc(600, 1, 1000, "game.exe");
    let rates = HashMap::new();

    let (actions, _) = moderator.tick(&make_snap(0, 20.0, vec![game.clone()]), &rates, None, &config);
    assert!(actions.is_empty(), "no rule yet");

    config.rules.push(PersistentRule {
        name: "Game.Exe".to_string(),
        priority: Some(PriorityClass::High),
        affinity_mask: None,
        cpu_sets: None,
        efficiency_mode: None,
    });
    let (actions, _) = moderator.tick(&make_snap(1000, 20.0, vec![game.clone()]), &rates, None, &config);
    assert_eq!(actions.len(), 1, "a rule added while the process runs applies to it");

    let (actions, _) = moderator.tick(&make_snap(2000, 20.0, vec![game.clone()]), &rates, None, &config);
    assert!(actions.is_empty(), "an unchanged rule is not applied again");

    config.rules[0].priority = Some(PriorityClass::AboveNormal);
    let (actions, _) = moderator.tick(&make_snap(3000, 20.0, vec![game]), &rates, None, &config);
    assert_eq!(actions.len(), 1, "an edited rule is applied again");
}

#[test]
fn shell_focus_does_not_exempt_apps_it_launched() {
    let mut moderator = Moderator::new();
    let config = ModeratorConfig::default();
    let explorer = make_proc(300, 1, 900, "explorer.exe");
    let app = make_proc(301, 300, 1000, "burner.exe");
    let mut rates = HashMap::new();
    rates.insert((301, 1000), 45.0);

    for t in [0, 1000] {
        moderator.tick(&make_snap(t, 85.0, vec![explorer.clone(), app.clone()]), &rates, Some(300), &config);
    }
    assert!(moderator.is_restrained(301, 1000), "the desktop having focus doesn't make explorer's children foreground");
}

#[test]
fn user_exclusion_matches_any_exe_casing() {
    let mut moderator = Moderator::new();
    let mut config = ModeratorConfig::default();
    config.exclusions.push("burner".to_string());
    let p = make_proc(100, 1, 1000, "Burner.Exe");
    let mut rates = HashMap::new();
    rates.insert((100, 1000), 45.0);

    for t in [0, 1000] {
        moderator.tick(&make_snap(t, 85.0, vec![p.clone()]), &rates, None, &config);
    }
    assert!(!moderator.is_restrained(100, 1000));
}

#[test]
fn restore_all_ends_every_restraint_with_its_original_state() {
    let mut moderator = Moderator::new();
    let config = ModeratorConfig::default();
    let p = make_proc(100, 1, 1000, "burner.exe");
    let mut rates = HashMap::new();
    rates.insert((100, 1000), 45.0);
    for t in [0, 1000] {
        moderator.tick(&make_snap(t, 85.0, vec![p.clone()]), &rates, None, &config);
    }
    moderator.record_original_state(100, 1000, Some(PriorityClass::High), None);

    let (actions, events) = moderator.restore_all(RestoreReason::Shutdown);
    assert_eq!(
        actions,
        vec![ModeratorAction::Restore {
            pid: 100,
            create_time: 1000,
            name: "burner.exe".to_string(),
            reason: RestoreReason::Shutdown,
            original_priority: Some(PriorityClass::High),
            original_efficiency: None,
        }]
    );
    assert_eq!(events.len(), 1);
    assert_eq!(moderator.restrained_count(), 0);
}
