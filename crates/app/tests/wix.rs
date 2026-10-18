//! Static checks on the installer's WiX source (`wix/main.wxs`): what it installs where, the fixed
//! UpgradeCode, the first-install checkboxes, and when each custom action runs `negotium.exe`
//! (`--integrate …`, `--exit-running`). The MSI itself is built by `cargo wix -p negotium` and
//! installed by hand; these checks never build or run it.
use negotium::integrate::{Action, ELEVATE, EXIT_RUNNING};
use roxmltree::{Document, Node};
use std::collections::HashMap;

const WIX_NS: &str = "http://schemas.microsoft.com/wix/2006/wi";
/// Fixed for the life of the product: every release's MSI must carry it, or upgrades install
/// side by side instead of replacing the old version.
const UPGRADE_CODE: &str = "7AA728A9-B8BC-4F0D-A154-EC88A6FA4077";

fn source() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wix/main.wxs");
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn is(node: &Node, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name && node.tag_name().namespace() == Some(WIX_NS)
}

fn all<'a>(doc: &'a Document, name: &str) -> Vec<Node<'a, 'a>> {
    doc.descendants().filter(|n| is(n, name)).collect()
}

fn one<'a>(doc: &'a Document, name: &str) -> Node<'a, 'a> {
    let found = all(doc, name);
    assert_eq!(found.len(), 1, "exactly one <{name}>");
    found[0]
}

fn attr<'a>(node: &Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attribute(name)
}

fn is_guid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12]) && groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A `<CustomAction>` and where `<InstallExecuteSequence>` puts it.
#[derive(Debug)]
struct Step {
    id: String,
    command: String,
    deferred: bool,
    /// Deferred and not impersonated: runs as LocalSystem.
    system: bool,
    condition: String,
    at: f64,
}

/// The standard actions the custom actions are placed against, at their usual sequence numbers
/// (RemoveExistingProducts as `MajorUpgrade Schedule="afterInstallInitialize"` places it).
fn standard() -> HashMap<&'static str, f64> {
    HashMap::from([
        ("AppSearch", 50.0),
        ("FindRelatedProducts", 25.0),
        ("CostFinalize", 1000.0),
        ("InstallValidate", 1400.0),
        ("InstallInitialize", 1500.0),
        ("RemoveExistingProducts", 1501.0),
        ("RemoveFiles", 3500.0),
        ("InstallFiles", 4000.0),
        ("InstallFinalize", 6600.0),
    ])
}

/// Every custom action the execute sequence schedules, with its resolved position: `Before=X`
/// just before X, `After=X` just after (chains of custom actions resolve through each other).
fn steps(doc: &Document) -> Vec<Step> {
    let sequence = one(doc, "InstallExecuteSequence");
    let customs: Vec<Node> = sequence.children().filter(|n| is(n, "Custom")).collect();
    let actions: HashMap<&str, Node> =
        all(doc, "CustomAction").into_iter().map(|a| (attr(&a, "Id").expect("CustomAction Id"), a)).collect();
    fn at(name: &str, customs: &[Node], depth: u32) -> f64 {
        if let Some(n) = standard().get(name) {
            return *n;
        }
        assert!(depth < 20, "sequencing loop at {name}");
        let custom = customs
            .iter()
            .find(|c| attr(c, "Action") == Some(name))
            .unwrap_or_else(|| panic!("{name} is neither scheduled nor a known standard action"));
        let step = 0.01 / f64::from(depth + 1);
        match (attr(custom, "Before"), attr(custom, "After")) {
            (Some(before), None) => at(before, customs, depth + 1) - step,
            (None, Some(after)) => at(after, customs, depth + 1) + step,
            _ => panic!("{name}: exactly one of Before/After"),
        }
    }
    customs
        .iter()
        .map(|c| {
            let id = attr(c, "Action").expect("Custom Action");
            let action = actions.get(id).unwrap_or_else(|| panic!("no CustomAction {id}"));
            let deferred = attr(action, "Execute") == Some("deferred");
            Step {
                id: id.to_string(),
                command: attr(action, "ExeCommand").unwrap_or_default().to_string(),
                deferred,
                system: deferred && attr(action, "Impersonate") == Some("no"),
                condition: c.text().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "),
                at: at(id, &customs, 0),
            }
        })
        .collect()
}

fn step<'a>(steps: &'a [Step], command: &str) -> &'a Step {
    let found: Vec<&Step> = steps.iter().filter(|s| s.command == command).collect();
    assert_eq!(found.len(), 1, "exactly one scheduled action runs {command:?}: {steps:#?}");
    found[0]
}

fn pos(name: &str) -> f64 {
    standard()[name]
}

const FIRST_INSTALL: [&str; 2] = ["NOT Installed", "NOT WIX_UPGRADE_DETECTED"];
const REAL_UNINSTALL: [&str; 2] = ["REMOVE=\"ALL\"", "NOT UPGRADINGPRODUCTCODE"];

fn assert_condition(step: &Step, parts: &[&str]) {
    for part in parts {
        assert!(step.condition.contains(part), "{}: condition {:?} lacks {part:?}", step.id, step.condition);
    }
}

#[test]
fn the_upgrade_code_is_fixed_and_a_guid() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let product = one(&doc, "Product");
    let code = attr(&product, "UpgradeCode").expect("UpgradeCode");
    assert!(is_guid(code), "{code}");
    assert_eq!(code, UPGRADE_CODE, "the UpgradeCode never changes");
    assert_eq!(attr(&product, "Id"), Some("*"), "a new ProductCode each build: every release is a major upgrade");
    assert_eq!(attr(&product, "Manufacturer"), Some("MyndPhreak"));
    assert_eq!(attr(&product, "Name"), Some("Negotium"));
    assert_eq!(attr(&product, "Version"), Some("$(var.Version)"), "the workspace version (cargo-wix)");
}

#[test]
fn it_installs_per_machine_and_blocks_downgrades() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let package = one(&doc, "Package");
    assert_eq!(attr(&package, "InstallScope"), Some("perMachine"));
    assert_eq!(attr(&package, "Platform"), Some("x64"));
    let upgrade = one(&doc, "MajorUpgrade");
    assert!(attr(&upgrade, "DowngradeErrorMessage").is_some_and(|m| !m.is_empty()), "downgrades are refused with a message");
    assert_eq!(attr(&upgrade, "Schedule"), Some("afterInstallInitialize"));
    assert!(attr(&upgrade, "AllowDowngrades").is_none());
}

#[test]
fn the_exe_goes_to_program_files_negotium_with_a_start_menu_shortcut() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let folder = all(&doc, "Directory")
        .into_iter()
        .find(|d| attr(d, "Id") == Some("INSTALLFOLDER"))
        .expect("INSTALLFOLDER");
    assert_eq!(attr(&folder, "Name"), Some("Negotium"));
    let parent = folder.parent().unwrap();
    assert_eq!(attr(&parent, "Id"), Some("ProgramFiles64Folder"));
    let exe = all(&doc, "File").into_iter().find(|f| attr(f, "Id") == Some("NegotiumExe")).expect("NegotiumExe");
    assert_eq!(attr(&exe, "Name"), Some("negotium.exe"));
    assert!(exe.ancestors().any(|a| attr(&a, "Id") == Some("INSTALLFOLDER")));
    // Nested in the exe's File: it targets that file; not advertised, so it starts the exe itself.
    let shortcut = one(&doc, "Shortcut");
    assert_eq!(attr(&shortcut, "Name"), Some("Negotium"));
    assert_eq!(shortcut.parent().and_then(|p| attr(&p, "Id")), Some("NegotiumExe"));
    assert_eq!(attr(&shortcut, "Directory"), Some("ProgramMenuFolder"));
    assert_eq!(attr(&shortcut, "Advertise"), Some("no"));
}

#[test]
fn the_checkboxes_are_off_by_default() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let properties = all(&doc, "Property");
    for (id, label) in [
        ("HOTKEY", "Open on Ctrl+Shift+Esc"),
        ("ADMINMODE", "Admin mode (no prompt)"),
        ("STARTUP", "Start with Windows"),
    ] {
        let declared: Vec<&Node> = properties.iter().filter(|p| attr(p, "Id") == Some(id)).collect();
        assert_eq!(declared.len(), 1, "{id} declared once");
        let p = declared[0];
        assert!(attr(p, "Value").is_none() && p.text().is_none_or(|t| t.trim().is_empty()), "{id} has no default value");
        // Public properties set in the UI only reach the elevated install when they are secure.
        assert_eq!(attr(p, "Secure"), Some("yes"), "{id} is secure");
        let boxes: Vec<Node> = all(&doc, "Control")
            .into_iter()
            .filter(|c| attr(c, "Type") == Some("CheckBox") && attr(c, "Property") == Some(id))
            .collect();
        assert_eq!(boxes.len(), 1, "one checkbox for {id}");
        assert_eq!(attr(&boxes[0], "Text"), Some(label));
    }
}

#[test]
fn the_checkboxes_are_offered_on_a_first_install_only() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let to_options: Vec<Node> = all(&doc, "Publish")
        .into_iter()
        .filter(|p| attr(p, "Event") == Some("NewDialog") && attr(p, "Value") == Some("OptionsDlg"))
        .filter(|p| attr(p, "Dialog") != Some("OptionsDlg"))
        .collect();
    assert!(!to_options.is_empty(), "the options page is reachable");
    for p in to_options {
        let condition = p.text().unwrap_or_default();
        for part in FIRST_INSTALL {
            assert!(condition.contains(part), "{condition:?} lacks {part:?}");
        }
    }
}

#[test]
fn the_checkbox_actions_run_after_the_files_on_a_first_install_only() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);

    let hotkey = step(&steps, "--integrate hotkey-on");
    assert_condition(hotkey, &["HOTKEY"]);
    assert_condition(hotkey, &FIRST_INSTALL);
    assert!(hotkey.system, "the HKLM redirect needs the installer's elevation (LocalSystem)");
    assert!(hotkey.at > pos("InstallFiles") && hotkey.at < pos("InstallFinalize"));

    // HKCU Run belongs to the installing user: impersonated, never LocalSystem.
    let startup = step(&steps, "--integrate startup-on");
    assert_condition(startup, &["STARTUP", "NOT ADMINMODE"]);
    assert_condition(startup, &FIRST_INSTALL);
    assert!(startup.deferred && !startup.system);
    assert!(startup.at > pos("InstallFiles") && startup.at < pos("InstallFinalize"));

    // The admin task is the installing user's (their SID, which LocalSystem doesn't have): run in
    // their context once the install is done, elevating through Negotium itself.
    for (command, with_startup) in [
        (format!("{ELEVATE} --integrate admin-on"), "NOT STARTUP"),
        (format!("{ELEVATE} --integrate admin-on --logon"), "STARTUP"),
    ] {
        let admin = step(&steps, &command);
        assert_condition(admin, &["ADMINMODE", with_startup]);
        assert_condition(admin, &FIRST_INSTALL);
        assert!(!admin.deferred, "{command}: immediate, in the user's context");
        assert!(admin.at > pos("InstallFinalize"));
    }
    let logon = step(&steps, &format!("{ELEVATE} --integrate admin-on --logon"));
    assert!(!logon.condition.contains("NOT STARTUP"));
}

#[test]
fn uninstall_cleanup_runs_only_on_a_real_uninstall_before_the_files_go() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);
    let uninstall = step(&steps, "--integrate uninstall");
    assert_condition(uninstall, &REAL_UNINSTALL);
    assert!(uninstall.system, "the redirect and every user's task: LocalSystem");
    assert!(uninstall.at > pos("InstallInitialize") && uninstall.at < pos("RemoveFiles"));

    // The uninstalling user's Run value (LocalSystem's HKCU is not theirs), after the task folder
    // is gone so startup-off only removes the value.
    let run = step(&steps, "--integrate startup-off");
    assert_condition(run, &REAL_UNINSTALL);
    assert!(run.deferred && !run.system, "impersonated");
    assert!(run.at > uninstall.at && run.at < pos("RemoveFiles"));
}

#[test]
fn running_instances_are_asked_to_exit_before_any_file_is_replaced() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);
    let close = step(&steps, EXIT_RUNNING);
    assert!(!close.deferred, "in the user's session, where the windows are");
    for later in ["InstallValidate", "RemoveExistingProducts", "RemoveFiles", "InstallFiles"] {
        assert!(close.at < pos(later), "before {later}");
    }
    assert!(close.at > pos("AppSearch"), "after the installed exe is looked for");
    // Runs the exe that is installed now (the old version on an upgrade), found by AppSearch.
    let action = all(&doc, "CustomAction").into_iter().find(|a| attr(a, "Id") == Some(close.id.as_str())).unwrap();
    let property = attr(&action, "Property").expect("runs the exe AppSearch found");
    assert_eq!(close.condition, property);
    let search = all(&doc, "Property").into_iter().find(|p| attr(p, "Id") == Some(property)).expect("searched");
    assert!(search.descendants().any(|n| is(&n, "FileSearch") && attr(&n, "Name") == Some("negotium.exe")));
}

#[test]
fn an_update_starts_the_new_version_and_never_prompts_for_a_reboot() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);
    let relaunch: Vec<&Step> = steps.iter().filter(|s| s.condition.contains(negotium::update::RELAUNCH)).collect();
    assert_eq!(relaunch.len(), 1);
    assert!(relaunch[0].condition.contains("NOT REMOVE"));
    assert!(!relaunch[0].deferred && relaunch[0].at > pos("InstallFinalize"));
    let declared = all(&doc, "Property").into_iter().find(|p| attr(p, "Id") == Some(negotium::update::RELAUNCH));
    assert_eq!(declared.and_then(|p| attr(&p, "Secure")), Some("yes"));
    let reboot = all(&doc, "Property").into_iter().find(|p| attr(p, "Id") == Some("REBOOT")).expect("REBOOT");
    assert_eq!(attr(&reboot, "Value"), Some("ReallySuppress"));
}

#[test]
fn every_integrate_command_line_is_one_negotium_accepts() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let commands: Vec<&str> =
        all(&doc, "CustomAction").iter().filter_map(|a| attr(a, "ExeCommand")).filter(|c| c.contains("--integrate")).collect();
    assert_eq!(commands.len(), 6);
    for command in commands {
        let args: Vec<&str> = command.split(' ').collect();
        let args = if args.first() == Some(&ELEVATE) { &args[1..] } else { &args[..] };
        assert!(Action::parse(args).is_some(), "{command:?}");
    }
}
