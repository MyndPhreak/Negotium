//! Static checks on the installer's WiX source (`wix/main.wxs`): what it installs where, the fixed
//! UpgradeCode, the first-install checkboxes, and when each custom action runs `negotium.exe`
//! (`--integrate …`, `--exit-running`). The MSI itself is built by `cargo wix -p negotium` and
//! installed by hand; these checks never build or run it.
use negotium::integrate::{Action, ELEVATE, EXIT_RUNNING, RELAUNCH};
use roxmltree::{Document, Node};
use std::collections::{BTreeSet, HashMap};

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
    /// Runs only when the install rolls back (`Execute="rollback"`).
    rollback: bool,
    /// Deferred or rollback, not impersonated: runs as LocalSystem.
    system: bool,
    /// The `Return` attribute (absent: "check").
    ret: String,
    /// The condition's clauses, split on ` AND `, whitespace normalised.
    clauses: BTreeSet<String>,
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

/// A condition as its `AND`ed clauses. The conditions here are plain conjunctions; an `OR` would
/// need real parsing, so it fails the test instead.
fn clauses(condition: &str) -> BTreeSet<String> {
    let condition = condition.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(!condition.contains(" OR "), "{condition:?}: only conjunctions are checked");
    condition.split(" AND ").map(str::to_string).filter(|c| !c.is_empty()).collect()
}

fn set(parts: &[&str]) -> BTreeSet<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

/// Every custom action the execute sequence schedules, with its resolved position: `Before=X`
/// just before X, `After=X` just after (chains of custom actions resolve through each other, one
/// fixed step per hop).
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
        match (attr(custom, "Before"), attr(custom, "After")) {
            (Some(before), None) => at(before, customs, depth + 1) - 0.001,
            (None, Some(after)) => at(after, customs, depth + 1) + 0.001,
            _ => panic!("{name}: exactly one of Before/After"),
        }
    }
    customs
        .iter()
        .map(|c| {
            let id = attr(c, "Action").expect("Custom Action");
            let action = actions.get(id).unwrap_or_else(|| panic!("no CustomAction {id}"));
            let execute = attr(action, "Execute").unwrap_or("immediate");
            let deferred = execute == "deferred";
            let rollback = execute == "rollback";
            Step {
                id: id.to_string(),
                command: attr(action, "ExeCommand").unwrap_or_default().to_string(),
                deferred,
                rollback,
                system: (deferred || rollback) && attr(action, "Impersonate") == Some("no"),
                ret: attr(action, "Return").unwrap_or("check").to_string(),
                clauses: clauses(c.text().unwrap_or_default()),
                at: at(id, &customs, 0),
            }
        })
        .collect()
}

/// The one scheduled step running `command`, a rollback step or not.
fn step<'a>(steps: &'a [Step], command: &str, rollback: bool) -> &'a Step {
    let found: Vec<&Step> = steps.iter().filter(|s| s.command == command && s.rollback == rollback).collect();
    assert_eq!(found.len(), 1, "exactly one scheduled action (rollback: {rollback}) runs {command:?}: {steps:#?}");
    found[0]
}

fn pos(name: &str) -> f64 {
    standard()[name]
}

const FIRST_INSTALL: [&str; 2] = ["NOT Installed", "NOT WIX_UPGRADE_DETECTED"];
const REAL_UNINSTALL: [&str; 2] = ["REMOVE=\"ALL\"", "NOT UPGRADINGPRODUCTCODE"];

/// `step`'s condition is exactly `parts` and `also` (no clause more or less).
fn assert_clauses(step: &Step, parts: &[&str], also: &[&str]) {
    let want: BTreeSet<String> = set(parts).union(&set(also)).cloned().collect();
    assert_eq!(step.clauses, want, "{}", step.id);
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

/// The MSI's ProductVersion is the workspace version: Windows Installer takes only
/// `major.minor.build` numbers (major and minor up to 255, build up to 65535), so a pre-release
/// (`0.2.0-beta.1`) or larger number fails here, plainly, before any installer is built.
#[test]
fn the_workspace_version_is_one_an_msi_can_carry() {
    let version = env!("CARGO_PKG_VERSION");
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(parts.len(), 3, "{version}: X.Y.Z only");
    let numbers: Vec<u32> = parts
        .iter()
        .map(|p| {
            assert!(!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()), "{version}: digits only (no pre-release)");
            p.parse().unwrap()
        })
        .collect();
    assert!(numbers[0] <= 255 && numbers[1] <= 255 && numbers[2] <= 65535, "{version}: out of an MSI version's range");
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
    // A rebuild of the same version replaces the installed one instead of installing beside it.
    assert_eq!(attr(&upgrade, "AllowSameVersionUpgrades"), Some("yes"));
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
        assert_eq!(clauses(p.text().unwrap_or_default()), set(&FIRST_INSTALL));
    }
}

#[test]
fn the_checkbox_actions_run_after_the_files_on_a_first_install_only() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);

    let hotkey = step(&steps, "--integrate hotkey-on", false);
    assert_clauses(hotkey, &FIRST_INSTALL, &["HOTKEY"]);
    assert!(hotkey.system, "the HKLM redirect needs the installer's elevation (LocalSystem)");
    assert!(hotkey.at > pos("InstallFiles") && hotkey.at < pos("InstallFinalize"));

    // HKCU Run belongs to the installing user: impersonated, never LocalSystem.
    let startup = step(&steps, "--integrate startup-on", false);
    assert_clauses(startup, &FIRST_INSTALL, &["STARTUP", "NOT ADMINMODE"]);
    assert!(startup.deferred && !startup.system);
    assert!(startup.at > pos("InstallFiles") && startup.at < pos("InstallFinalize"));

    // The admin task is the installing user's (their SID, which LocalSystem doesn't have): run in
    // their context once the install is done, elevating through Negotium itself.
    for (command, with_startup) in [
        (format!("{ELEVATE} --integrate admin-on"), "NOT STARTUP"),
        (format!("{ELEVATE} --integrate admin-on --logon"), "STARTUP"),
    ] {
        let admin = step(&steps, &command, false);
        assert_clauses(admin, &FIRST_INSTALL, &["ADMINMODE", with_startup]);
        assert!(!admin.deferred && !admin.rollback, "{command}: immediate, in the user's context");
        assert!(admin.at > pos("InstallFinalize"));
    }
}

/// A first install that fails or is cancelled after a checkbox step undoes it before its files
/// go: never a redirect (or Run value) naming a removed exe.
#[test]
fn a_rolled_back_first_install_undoes_its_checkbox_steps() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);
    for (on, off) in [("--integrate hotkey-on", "--integrate hotkey-off"), ("--integrate startup-on", "--integrate startup-off")] {
        let on = step(&steps, on, false);
        let undo = step(&steps, off, true);
        assert_eq!(undo.clauses, on.clauses, "{}: the same condition as {}", undo.id, on.id);
        assert_eq!(undo.system, on.system, "{}: in the same context as {}", undo.id, on.id);
        // Rollback runs the script backwards: scheduled just before the step it undoes, it runs
        // right after that step's own undoing, while InstallFiles' undoing (removing the exe) is
        // still to come.
        assert!(undo.at < on.at && undo.at > pos("InstallFiles"), "{}", undo.id);
        assert!(!steps.iter().any(|s| s.at > undo.at && s.at < on.at), "{}: right before {}", undo.id, on.id);
    }
}

#[test]
fn uninstall_cleanup_runs_only_on_a_real_uninstall_before_the_files_go() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);
    let uninstall = step(&steps, "--integrate uninstall", false);
    assert_clauses(uninstall, &REAL_UNINSTALL, &[]);
    assert!(uninstall.system, "the redirect and every user's task: LocalSystem");
    assert!(uninstall.at > pos("InstallInitialize") && uninstall.at < pos("RemoveFiles"));

    // The uninstalling user's Run value (LocalSystem's HKCU is not theirs), after the task folder
    // is gone so startup-off only removes the value.
    let run = step(&steps, "--integrate startup-off", false);
    assert_clauses(run, &REAL_UNINSTALL, &[]);
    assert!(run.deferred && !run.system, "impersonated");
    assert!(run.at > uninstall.at && run.at < pos("RemoveFiles"));
}

/// A cancelled uninstall puts back the redirect and this user's Run value when AppSearch found
/// Negotium's (its files are restored by then: RemoveFiles' undoing comes first).
#[test]
fn a_rolled_back_uninstall_puts_back_what_it_found() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);
    let found = "~>< \"\\Negotium\\negotium.exe\"";
    for (cleanup, restore, property, system) in [
        ("--integrate uninstall", "--integrate hotkey-on", "NEGOTIUM_IFEO", true),
        ("--integrate startup-off", "--integrate startup-on", "NEGOTIUM_RUN", false),
    ] {
        let cleanup = step(&steps, cleanup, false);
        let restore = step(&steps, restore, true);
        assert_clauses(restore, &REAL_UNINSTALL, &[&format!("{property} {found}")]);
        assert_eq!(restore.system, system, "{}", restore.id);
        assert!(restore.at < cleanup.at && restore.at > pos("InstallInitialize"), "{}", restore.id);
        assert!(!steps.iter().any(|s| s.at > restore.at && s.at < cleanup.at), "{}: right before", restore.id);
        let search = all(&doc, "Property").into_iter().find(|p| attr(p, "Id") == Some(property)).expect(property);
        assert!(search.children().any(|n| is(&n, "RegistrySearch")), "{property} is read by AppSearch");
    }
}

#[test]
fn running_instances_are_asked_to_exit_before_any_file_is_replaced() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);
    let close = step(&steps, EXIT_RUNNING, false);
    assert!(!close.deferred, "in the user's session, where the windows are");
    for later in ["InstallValidate", "RemoveExistingProducts", "RemoveFiles", "InstallFiles"] {
        assert!(close.at < pos(later), "before {later}");
    }
    assert!(close.at > pos("AppSearch"), "after the installed exe is looked for");
    // Runs the exe that is installed now (the old version on an upgrade), found by AppSearch.
    let action = all(&doc, "CustomAction").into_iter().find(|a| attr(a, "Id") == Some(close.id.as_str())).unwrap();
    let property = attr(&action, "Property").expect("runs the exe AppSearch found");
    assert_eq!(close.clauses, set(&[property]));
    let search = all(&doc, "Property").into_iter().find(|p| attr(p, "Id") == Some(property)).expect("searched");
    assert!(search.descendants().any(|n| is(&n, "FileSearch") && attr(&n, "Name") == Some("negotium.exe")));
}

#[test]
fn an_update_starts_the_new_version_and_never_prompts_for_a_reboot() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);
    let relaunch = step(&steps, RELAUNCH, false);
    assert_clauses(relaunch, &[&format!("{}=\"1\"", negotium::update::RELAUNCH), "NOT REMOVE"], &[]);
    assert!(!relaunch.deferred && relaunch.at > pos("InstallFinalize"));
    let declared = all(&doc, "Property").into_iter().find(|p| attr(p, "Id") == Some(negotium::update::RELAUNCH));
    assert_eq!(declared.and_then(|p| attr(&p, "Secure")), Some("yes"));
    let reboot = all(&doc, "Property").into_iter().find(|p| attr(p, "Id") == Some("REBOOT")).expect("REBOOT");
    assert_eq!(attr(&reboot, "Value"), Some("ReallySuppress"));
}

/// No step that runs Negotium can fail (roll back) an install or block an uninstall: each ignores
/// its exit code. The relaunch isn't waited for at all (which ignores it too).
#[test]
fn every_negotium_step_ignores_its_result() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let steps = steps(&doc);
    let negotium: Vec<&Step> = steps.iter().filter(|s| !s.command.is_empty()).collect();
    // exit-running, relaunch, the four checkbox steps, the two uninstall steps and four rollbacks.
    assert_eq!(negotium.len(), 12, "{negotium:#?}");
    for s in negotium {
        let want = if s.command == RELAUNCH { "asyncNoWait" } else { "ignore" };
        assert_eq!(s.ret, want, "{}", s.id);
    }
    let rollbacks = steps.iter().filter(|s| s.rollback).count();
    assert_eq!(rollbacks, 4);
    // "Launch Negotium" on the last page (published by the dialog, not sequenced).
    let launch = all(&doc, "CustomAction").into_iter().find(|a| attr(a, "Id") == Some("LaunchNegotium")).unwrap();
    assert_eq!(attr(&launch, "Return"), Some("ignore"));
}

#[test]
fn every_integrate_command_line_is_one_negotium_accepts() {
    let source = source();
    let doc = Document::parse(&source).unwrap();
    let commands: Vec<&str> =
        all(&doc, "CustomAction").iter().filter_map(|a| attr(a, "ExeCommand")).filter(|c| c.contains("--integrate")).collect();
    assert_eq!(commands.len(), 10);
    for command in commands {
        let args: Vec<&str> = command.split(' ').collect();
        let args = if args.first() == Some(&ELEVATE) { &args[1..] } else { &args[..] };
        assert!(Action::parse(args).is_some(), "{command:?}");
    }
}
