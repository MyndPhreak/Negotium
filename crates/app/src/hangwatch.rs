//! Decides when to offer the "isn't responding" prompt. Pure: the worker supplies the hung apps
//! each tick (`Controls::hung_apps`), the UI thread polls this and shows or hides the prompt.
use std::collections::{HashMap, HashSet};

/// How long Negotium has seen an app's window hung before it offers to deal with it. Windows
/// itself ghosts the window after about five seconds, so this is on top of that.
pub const PROMPT_AFTER_MS: u64 = 3_000;
/// How long "Wait" keeps the prompt away from that process.
pub const WAIT_SNOOZE_MS: u64 = 60_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HungApp {
    pub pid: u32,
    pub create_time: i64,
    /// What the user sees ("Notepad").
    pub name: String,
    /// The image name ("notepad.exe"): what "don't ask again" remembers.
    pub process_name: String,
    pub hung_ms: u64,
}

impl HungApp {
    pub fn key(&self) -> (u32, i64) {
        (self.pid, self.create_time)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    /// Nothing to change.
    Keep,
    /// Show the prompt for this app.
    Show(HungApp),
    /// The prompted app recovered or exited: take the prompt away.
    Hide,
}

#[derive(Default)]
pub struct HangWatch {
    snoozed: HashMap<(u32, i64), u64>,
    ignored: HashSet<String>,
    shown: Option<(u32, i64)>,
}

impl HangWatch {
    pub fn new(ignored: impl IntoIterator<Item = String>) -> Self {
        Self { ignored: ignored.into_iter().map(|n| n.to_lowercase()).collect(), ..Self::default() }
    }

    pub fn poll(&mut self, now_ms: u64, enabled: bool, apps: &[HungApp]) -> Poll {
        self.snoozed.retain(|key, until| *until > now_ms && apps.iter().any(|a| a.key() == *key));
        if let Some(key) = self.shown {
            let wanted = enabled && apps.iter().any(|a| a.key() == key);
            if wanted {
                return Poll::Keep;
            }
            self.shown = None;
            return Poll::Hide;
        }
        if !enabled {
            return Poll::Keep;
        }
        let next = apps
            .iter()
            .filter(|a| a.hung_ms >= PROMPT_AFTER_MS)
            .filter(|a| !self.snoozed.contains_key(&a.key()) && !self.ignored.contains(&a.process_name.to_lowercase()))
            .max_by_key(|a| a.hung_ms);
        match next {
            Some(app) => {
                self.shown = Some(app.key());
                Poll::Show(app.clone())
            }
            None => Poll::Keep,
        }
    }

    /// The user chose Wait: leave this process alone for a while.
    pub fn wait(&mut self, key: (u32, i64), now_ms: u64) {
        self.snoozed.insert(key, now_ms + WAIT_SNOOZE_MS);
        self.shown = None;
    }

    /// The prompt was closed after an action (or by the user); the next hung app may be offered.
    pub fn closed(&mut self) {
        self.shown = None;
    }

    /// "Don't ask for this app": remembered by image name, case-insensitively.
    pub fn ignore(&mut self, process_name: &str) {
        self.ignored.insert(process_name.to_lowercase());
        self.shown = None;
    }

    pub fn ignored(&self) -> Vec<String> {
        let mut v: Vec<String> = self.ignored.iter().cloned().collect();
        v.sort();
        v
    }
}

/// The arguments part of a process command line: everything after the (possibly quoted) program.
pub fn command_line_args(command_line: &str) -> &str {
    let s = command_line.trim_start();
    let rest = match s.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"').map_or("", |(_, after)| after),
        None => s.split_once(char::is_whitespace).map_or("", |(_, after)| after),
    };
    rest.trim_start()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(pid: u32, hung_ms: u64) -> HungApp {
        HungApp { pid, create_time: pid as i64, name: format!("App{pid}"), process_name: format!("app{pid}.EXE"), hung_ms }
    }

    #[test]
    fn waits_for_the_threshold_then_offers_the_longest_hung_app_once() {
        let mut w = HangWatch::default();
        assert_eq!(w.poll(0, true, &[app(1, 1_000)]), Poll::Keep);
        assert_eq!(w.poll(1, true, &[app(1, 4_000), app(2, 9_000)]), Poll::Show(app(2, 9_000)));
        assert_eq!(w.poll(2, true, &[app(1, 5_000), app(2, 10_000)]), Poll::Keep, "already showing one");
    }

    #[test]
    fn hides_when_the_app_recovers_or_the_feature_is_turned_off() {
        let mut w = HangWatch::default();
        w.poll(0, true, &[app(1, 5_000)]);
        assert_eq!(w.poll(1, true, &[]), Poll::Hide);
        w.poll(2, true, &[app(1, 5_000)]);
        assert_eq!(w.poll(3, false, &[app(1, 6_000)]), Poll::Hide);
        assert_eq!(w.poll(4, false, &[app(1, 7_000)]), Poll::Keep);
    }

    #[test]
    fn wait_snoozes_that_process_until_it_expires() {
        let mut w = HangWatch::default();
        w.poll(0, true, &[app(1, 5_000)]);
        w.wait((1, 1), 0);
        assert_eq!(w.poll(1_000, true, &[app(1, 6_000)]), Poll::Keep);
        assert_eq!(w.poll(WAIT_SNOOZE_MS, true, &[app(1, 70_000)]), Poll::Show(app(1, 70_000)));
    }

    #[test]
    fn ignored_apps_are_never_offered_whatever_the_case() {
        let mut w = HangWatch::new(["APP1.exe".to_string()]);
        assert_eq!(w.poll(0, true, &[app(1, 9_000)]), Poll::Keep);
        w.poll(1, true, &[app(2, 9_000)]);
        w.ignore("App2.exe");
        assert_eq!(w.poll(2, true, &[app(2, 9_000)]), Poll::Keep);
        assert_eq!(w.ignored(), ["app1.exe", "app2.exe"]);
    }

    #[test]
    fn splits_the_arguments_off_a_command_line() {
        assert_eq!(command_line_args(r#""C:\Program Files\A\a.exe" --flag "x y""#), r#"--flag "x y""#);
        assert_eq!(command_line_args(r"C:\a.exe -x"), "-x");
        assert_eq!(command_line_args(r#""C:\a.exe""#), "");
        assert_eq!(command_line_args(r"a.exe"), "");
    }
}
