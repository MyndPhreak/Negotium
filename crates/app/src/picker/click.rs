//! What a click on the hovered window does (pure): ignore, tell, confirm, or run the end job.
use super::apps::AppInfo;
use super::target::Pickable;
use crate::dispatch::{Job, Plan, plan};

/// The target captured at button-down; passed unchanged at button-up.
#[derive(Clone, Debug, PartialEq)]
pub struct Pressed {
    pub kind: Pickable,
    pub info: Option<AppInfo>,
    pub needs_admin: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Shell / no target / not ready / Windows group.
    Ignore,
    /// Needs admin: a dialog with Close only.
    Tell(String),
    Confirm { title: String, body: String, job: Job, restart_shell: bool },
    Run { job: Job, restart_shell: bool },
}

const NEEDS_ADMIN: &str = "Needs admin \u{2014} restart Negotium as admin";
const EXPLORER_BODY: &str = "This also restarts the taskbar and the desktop. All File Explorer windows close.";

pub fn decide(pressed: &Pressed, shift: bool, all_procs: &[(u32, i64, u32)]) -> Outcome {
    if matches!(pressed.kind, Pickable::Shell | Pickable::PassThrough | Pickable::Block) {
        return Outcome::Ignore;
    }
    let Some(info) = &pressed.info else { return Outcome::Ignore };
    let planned = plan("end-task", &info.target, all_procs);
    if planned == Plan::Nothing {
        return Outcome::Ignore;
    }
    if pressed.needs_admin {
        return if shift { Outcome::Ignore } else { Outcome::Tell(NEEDS_ADMIN.to_string()) };
    }
    let title = format!("End {}?", info.target.name);
    match planned {
        Plan::Nothing => Outcome::Ignore,
        Plan::Confirm(body, job) => {
            Outcome::Confirm { title, body, job, restart_shell: pressed.kind == Pickable::ExplorerInShell }
        }
        Plan::Run(job) => {
            let restart_shell = pressed.kind == Pickable::ExplorerInShell;
            if restart_shell {
                let body = format!("{} {EXPLORER_BODY}", closes(job.targets.len()));
                Outcome::Confirm { title, body, job, restart_shell }
            } else if shift {
                Outcome::Run { job, restart_shell }
            } else {
                let body = closes(job.targets.len());
                Outcome::Confirm { title, body, job, restart_shell }
            }
        }
    }
}

fn closes(n: usize) -> String {
    let noun = if n == 1 { "process" } else { "processes" };
    format!("This closes {n} {noun}. You lose unsaved work.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{JobKind, end_summary};
    use crate::view::Target;

    fn info(name: &str, procs: &[(u32, i64)], root: Option<(u32, i64)>) -> AppInfo {
        AppInfo {
            target: Target { name: name.into(), root, procs: procs.to_vec(), ..Target::default() },
            process_count: procs.len(),
            private_bytes: 0,
            hung: false,
        }
    }

    fn pressed(kind: Pickable, info: Option<AppInfo>) -> Pressed {
        Pressed { kind, info, needs_admin: false }
    }

    fn app() -> Pressed {
        pressed(Pickable::App, Some(info("Notepad", &[(10, 1), (11, 2)], Some((10, 1)))))
    }

    #[test]
    fn click_on_app_confirms_with_cancel_text() {
        let Outcome::Confirm { title, body, job, restart_shell } = decide(&app(), false, &[]) else {
            panic!("expected Confirm")
        };
        assert_eq!(title, "End Notepad?");
        assert_eq!(body, "This closes 2 processes. You lose unsaved work.");
        assert_eq!(job.kind, JobKind::EndMany);
        assert!(!restart_shell);
    }

    #[test]
    fn single_process_body_is_singular() {
        let p = pressed(Pickable::App, Some(info("Calc", &[(5, 1)], Some((5, 1)))));
        let Outcome::Confirm { body, .. } = decide(&p, false, &[]) else { panic!("expected Confirm") };
        assert_eq!(body, "This closes 1 process. You lose unsaved work.");
    }

    #[test]
    fn shift_click_runs_at_once() {
        let Outcome::Run { job, restart_shell } = decide(&app(), true, &[]) else { panic!("expected Run") };
        assert_eq!(job.name, "Notepad");
        assert!(!restart_shell);
    }

    #[test]
    fn critical_always_confirms_even_with_shift() {
        let mut i = info("wininit.exe", &[(4, 1)], Some((4, 1)));
        i.target.critical = true;
        let p = pressed(Pickable::App, Some(i));
        for shift in [false, true] {
            let Outcome::Confirm { body, restart_shell, .. } = decide(&p, shift, &[]) else {
                panic!("expected Confirm")
            };
            assert!(body.starts_with("Ending wininit.exe will make Windows unusable"));
            assert!(!restart_shell);
        }
    }

    #[test]
    fn critical_explorer_in_shell_still_restarts_the_shell() {
        let mut i = info("explorer.exe", &[(7, 1)], Some((7, 1)));
        i.target.critical = true;
        let p = pressed(Pickable::ExplorerInShell, Some(i));
        let Outcome::Confirm { restart_shell, .. } = decide(&p, true, &[]) else { panic!("expected Confirm") };
        assert!(restart_shell);
    }

    #[test]
    fn explorer_in_shell_always_confirms_and_restarts_the_shell() {
        let p = pressed(Pickable::ExplorerInShell, Some(info("File Explorer", &[(7, 1)], Some((7, 1)))));
        for shift in [false, true] {
            let Outcome::Confirm { title, body, restart_shell, .. } = decide(&p, shift, &[]) else {
                panic!("expected Confirm")
            };
            assert_eq!(title, "End File Explorer?");
            assert_eq!(
                body,
                "This closes 1 process. You lose unsaved work. This also restarts the taskbar and the desktop. All File Explorer windows close."
            );
            assert!(restart_shell);
        }
    }

    #[test]
    fn needs_admin_tells_on_click_and_ignores_shift_click() {
        let mut p = app();
        p.needs_admin = true;
        assert_eq!(decide(&p, false, &[]), Outcome::Tell("Needs admin \u{2014} restart Negotium as admin".into()));
        assert_eq!(decide(&p, true, &[]), Outcome::Ignore);
    }

    #[test]
    fn shell_and_not_ready_are_ignored() {
        let i = Some(info("Taskbar", &[(1, 1)], Some((1, 1))));
        assert_eq!(decide(&pressed(Pickable::Shell, i.clone()), false, &[]), Outcome::Ignore);
        assert_eq!(decide(&pressed(Pickable::PassThrough, i.clone()), false, &[]), Outcome::Ignore);
        assert_eq!(decide(&pressed(Pickable::Block, i), false, &[]), Outcome::Ignore);
        assert_eq!(decide(&pressed(Pickable::App, None), false, &[]), Outcome::Ignore);
        assert_eq!(decide(&pressed(Pickable::ExplorerInShell, None), true, &[]), Outcome::Ignore);
    }

    #[test]
    fn windows_group_is_ignored() {
        let mut i = info("Windows", &[(4, 1), (8, 1)], None);
        i.target.group = true;
        assert_eq!(decide(&pressed(Pickable::App, Some(i)), true, &[]), Outcome::Ignore);
    }

    #[test]
    fn the_target_is_fixed_at_button_down() {
        // `pressed` carries app A; the later snapshot (all_procs) describes other processes.
        let p = app();
        let changed = [(99, 9, 0), (10, 1, 99)];
        let Outcome::Run { job, .. } = decide(&p, true, &changed) else { panic!("expected Run") };
        assert_eq!(job.targets, vec![(11, 2), (10, 1)]);
    }

    #[test]
    fn a_full_success_summary_is_silent() {
        // The picker shows a dialog only for a non-empty summary; a full success is "".
        assert_eq!(end_summary("Notepad", 2, &[], false), "");
    }

    #[test]
    fn job_ends_children_before_root() {
        let Outcome::Run { job, .. } = decide(&app(), true, &[]) else { panic!("expected Run") };
        assert_eq!(job.targets, vec![(11, 2), (10, 1)]);
    }
}
