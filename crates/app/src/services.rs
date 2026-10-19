//! Services page: lists Windows services (`negotium_actions::services`), supports search, sorting,
//! and Start / Stop / Restart / Go to process actions.
use crate::MainWindow;
use negotium_actions::services::{ServiceEntry, list_services, restart_service, start_service, stop_service};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServiceRow {
    pub id: i32,
    pub name: String,
    pub pid: String,
    pub pid_num: u32,
    pub description: String,
    pub status: String,
    pub group: String,
    pub start_type: String,
    pub running: bool,
    pub can_stop: bool,
    pub search_text: String,
}

pub fn service_rows(entries: &[ServiceEntry]) -> Vec<ServiceRow> {
    let mut rows: Vec<ServiceRow> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let search_text = format!(
                "{} {} {} {} {}",
                e.name.to_lowercase(),
                e.display_name.to_lowercase(),
                e.pid.map(|p| p.to_string()).unwrap_or_default(),
                e.status.to_lowercase(),
                e.group.to_lowercase(),
            );
            ServiceRow {
                id: i as i32,
                name: e.name.clone(),
                pid: e.pid.map(|p| p.to_string()).unwrap_or_else(|| "—".into()),
                pid_num: e.pid.unwrap_or(0),
                description: e.display_name.clone(),
                status: e.status.clone(),
                group: if e.group.is_empty() { "—".into() } else { e.group.clone() },
                start_type: e.start_type.clone(),
                running: e.running,
                can_stop: e.running,
                search_text,
            }
        })
        .collect();
    sort_service_rows(&mut rows, 0, false);
    rows
}

/// Sorts service rows by column (0 = Name, 1 = PID, 2 = Description, 3 = Status, 4 = Group, 5 = Startup type).
pub fn sort_service_rows(rows: &mut [ServiceRow], col: i32, desc: bool) {
    rows.sort_by(|a, b| {
        let ord = match col {
            1 => a.pid_num.cmp(&b.pid_num),
            2 => a.description.to_lowercase().cmp(&b.description.to_lowercase()),
            3 => a.status.to_lowercase().cmp(&b.status.to_lowercase()),
            4 => a.group.to_lowercase().cmp(&b.group.to_lowercase()),
            5 => a.start_type.to_lowercase().cmp(&b.start_type.to_lowercase()),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        let ord = if desc { ord.reverse() } else { ord };
        ord.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.id.cmp(&b.id))
    });
}

pub fn filter_service_rows(rows: &[ServiceRow], search: &str) -> Vec<ServiceRow> {
    let needle = search.trim().to_lowercase();
    if needle.is_empty() {
        return rows.to_vec();
    }
    rows.iter().filter(|r| r.search_text.contains(&needle)).cloned().collect()
}

pub type ServiceJobResult = (Vec<ServiceEntry>, Option<String>);

pub fn list_job() -> ServiceJobResult {
    match list_services() {
        Ok(entries) => (entries, None),
        Err(e) => (Vec::new(), Some(format!("Couldn't list services: {e}."))),
    }
}

pub fn action_job(name: &str, action: &str, elevated: bool) -> ServiceJobResult {
    let res = match action {
        "start" => start_service(name),
        "stop" => stop_service(name),
        "restart" => restart_service(name),
        _ => Ok(()),
    };
    let failure = res.err().map(|e| e.message(name, elevated));
    let (entries, list_err) = list_job();
    (entries, failure.or(list_err))
}

pub fn begin_job(ui: &MainWindow) -> bool {
    if ui.get_services_loading() {
        return false;
    }
    ui.set_services_loading(true);
    true
}

pub fn spawn_job(weak: slint::Weak<MainWindow>, job: impl FnOnce() -> ServiceJobResult + Send + 'static) {
    std::thread::spawn(move || {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        let (entries, failure) = match res {
            Ok((entries, failure)) => (Some(entries), failure),
            Err(_) => (None, Some("Something went wrong on the Services page.".to_string())),
        };
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                if let Some(entries) = entries {
                    crate::bridge::with_bridge(|b| b.set_services(&ui, entries));
                }
                ui.set_services_loading(false);
                if let Some(msg) = failure {
                    ui.set_toast(msg.into());
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_filter_and_sort_by_column() {
        let entries = vec![
            ServiceEntry {
                name: "wuauserv".into(),
                display_name: "Windows Update".into(),
                status: "Stopped".into(),
                running: false,
                pid: None,
                start_type: "Manual".into(),
                group: "netsvcs".into(),
            },
            ServiceEntry {
                name: "Dnscache".into(),
                display_name: "DNS Client".into(),
                status: "Running".into(),
                running: true,
                pid: Some(1420),
                start_type: "Automatic".into(),
                group: "NetworkService".into(),
            },
        ];
        let mut rows = service_rows(&entries);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Dnscache", "wuauserv"]);
        sort_service_rows(&mut rows, 1, true); // PID descending
        assert_eq!(rows[0].name, "Dnscache");
        let filtered = filter_service_rows(&rows, "1420");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "Dnscache");
    }
}
