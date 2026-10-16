//! Startup listing and toggling, sandboxed: every key lives under HKCU\Software\Negotium-Test\…
//! and is removed afterwards. The user's real startup entries are only ever read: this process
//! never calls `allow_system_writes`, so the registry helpers refuse any write outside the sandbox.
use negotium_actions::ActionError;
use negotium_actions::startup::registry::{self, Hive, RegKey};
use negotium_actions::startup::{
    StartupEntry, StartupRoots, StartupSource, is_read_error, list_startup, set_startup_enabled,
    write_shortcut,
};
use negotium_collector::VersionInfo;

const PING: &str = r"C:\Windows\System32\PING.EXE";

/// A registry sandbox under the test root and two fresh temp folders standing in for the
/// per-user and all-users Startup folders; all removed on drop.
struct Sandbox {
    base: RegKey,
    dir: std::path::PathBuf,
    roots: StartupRoots,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let base = RegKey::new(
            Hive::CurrentUser,
            &format!(r"{}\{name}-{}", registry::TEST_ROOT, std::process::id()),
        );
        let _ = registry::delete_test_tree(&base);
        let dir = std::env::temp_dir().join(format!(
            "negotium-startup-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let (user, machine) = (dir.join("user"), dir.join("machine"));
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&machine).unwrap();
        Self {
            roots: StartupRoots::under(&base.path).with_folders(user, machine),
            base,
            dir,
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = registry::delete_test_tree(&self.base);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn find<'a>(entries: &'a [StartupEntry], key: &str) -> &'a StartupEntry {
    entries
        .iter()
        .find(|e| e.key == key)
        .unwrap_or_else(|| panic!("no {key:?} in {entries:?}"))
}

fn approved(key: &RegKey, name: &str) -> Vec<u8> {
    registry::value(key, name)
        .and_then(|v| v.binary().map(<[u8]>::to_vec))
        .unwrap_or_else(|| panic!("no value {name:?}"))
}

#[test]
fn run_entries_are_listed_with_their_state_and_toggle_in_task_managers_format() {
    let sb = Sandbox::new("run");
    let r = &sb.roots;
    registry::set_string(
        &r.user_run,
        "Ping test",
        &format!("\"{PING}\" -n 1 127.0.0.1"),
    )
    .unwrap();
    registry::set_string(
        &r.user_run,
        "Missing app",
        r"C:\Nope\Missing App\missing.exe /x",
    )
    .unwrap();
    registry::set_string(&r.machine_run32, "Old app", r"C:\Nope\old.exe").unwrap();
    // A disabled state written by someone else, under a name that differs only in case.
    registry::set_binary(
        &r.user_approved.child("Run"),
        "missing APP",
        &[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8],
    )
    .unwrap();

    let (entries, errors) = list_startup(r);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(entries.len(), 3);
    let ping = find(&entries, "Ping test").clone();
    assert!(ping.enabled && ping.changeable);
    assert_eq!(ping.source, StartupSource::UserRun);
    assert!(ping.publisher.is_some(), "CompanyName of PING.EXE");
    assert_ne!(
        ping.name, "Ping test",
        "named after the program's description"
    );
    assert!(ping.location.ends_with(r"\UserRun"), "{}", ping.location);
    let missing = find(&entries, "Missing app");
    assert!(
        !missing.enabled,
        "StartupApproved names match case-insensitively"
    );
    assert_eq!(
        (missing.name.as_str(), missing.publisher.as_deref()),
        ("Missing app", None)
    );
    let old = find(&entries, "Old app").clone();
    assert_eq!(old.source, StartupSource::MachineRun32);
    assert!(old.source.needs_admin());

    set_startup_enabled(r, &ping, false).unwrap();
    let bytes = approved(&r.user_approved.child("Run"), "Ping test");
    assert_eq!(bytes.len(), 12);
    assert_eq!(bytes[..4], [3, 0, 0, 0]);
    assert!(bytes[4..].iter().any(|&b| b != 0), "disabling records when");
    assert!(!find(&list_startup(r).0, "Ping test").enabled);
    set_startup_enabled(r, &ping, true).unwrap();
    assert_eq!(
        approved(&r.user_approved.child("Run"), "Ping test"),
        [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert!(find(&list_startup(r).0, "Ping test").enabled);
    set_startup_enabled(r, &old, false).unwrap();
    assert_eq!(
        approved(&r.machine_approved.child("Run32"), "Old app")[0],
        3,
        "32-bit HKLM entries use Run32"
    );
}

#[test]
fn approved_values_with_other_flag_bits_keep_them() {
    let sb = Sandbox::new("flags");
    let r = &sb.roots;
    registry::set_string(&r.user_run, "Tool", r"C:\Nope\tool.exe").unwrap();
    registry::set_binary(
        &r.user_approved.child("Run"),
        "Tool",
        &[6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    )
    .unwrap();
    let tool = find(&list_startup(r).0, "Tool").clone();
    assert!(tool.enabled);
    set_startup_enabled(r, &tool, false).unwrap();
    assert_eq!(approved(&r.user_approved.child("Run"), "Tool")[0], 7);
    set_startup_enabled(r, &tool, true).unwrap();
    assert_eq!(
        approved(&r.user_approved.child("Run"), "Tool")[..4],
        [6, 0, 0, 0]
    );
}

/// Review Focus 4: values other tools wrote — short ones, 07 flags, a name differing in case.
#[test]
fn short_and_foreign_approved_values_read_correctly_and_toggle_into_one_full_value() {
    let sb = Sandbox::new("foreign");
    let r = &sb.roots;
    let states = r.user_approved.child("Run");
    registry::set_string(&r.user_run, "Short", r"C:\Nope\short.exe").unwrap();
    registry::set_string(&r.user_run, "Four", r"C:\Nope\four.exe").unwrap();
    registry::set_string(&r.user_run, "Cased", r"C:\Nope\cased.exe").unwrap();
    registry::set_binary(&states, "Short", &[3, 0]).unwrap();
    registry::set_binary(&states, "Four", &[7, 0, 0, 0]).unwrap();
    registry::set_binary(&states, "CASED", &[7, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9, 9]).unwrap();

    let entries = list_startup(r).0;
    assert!(
        find(&entries, "Short").enabled,
        "too short to carry flags: enabled"
    );
    assert!(
        !find(&entries, "Four").enabled,
        "four bytes carry the flags"
    );
    let cased = find(&entries, "Cased").clone();
    assert!(!cased.enabled);

    set_startup_enabled(r, find(&entries, "Short"), false).unwrap();
    let short = approved(&states, "Short");
    assert_eq!(
        (short.len(), &short[..4]),
        (12, &[3u8, 0, 0, 0][..]),
        "rewritten in full, default flags"
    );
    set_startup_enabled(r, find(&entries, "Four"), true).unwrap();
    assert_eq!(
        approved(&states, "Four"),
        [6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );

    set_startup_enabled(r, &cased, true).unwrap();
    let names: Vec<String> = registry::values(&states)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| n.eq_ignore_ascii_case("cased"))
        .collect();
    assert_eq!(
        names.len(),
        1,
        "the existing value is updated, not joined by a second one: {names:?}"
    );
    assert_eq!(
        approved(&states, "Cased")[..4],
        [6, 0, 0, 0],
        "the 0x04 bit is kept"
    );
    assert!(find(&list_startup(r).0, "Cased").enabled);
}

/// Review Focus 2: unusual commands are listed — under their value name and without a publisher
/// when the program can't be found — and can still be disabled and enabled.
#[test]
fn unusual_run_commands_are_listed_and_toggle() {
    let sb = Sandbox::new("unusual");
    let r = &sb.roots;
    let dir = std::env::temp_dir().join(format!("Negotium Test {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let spaced = dir.join("ping copy.exe");
    std::fs::copy(PING, &spaced).unwrap();
    let commands = [
        ("Spaced", format!("{} -n 1 127.0.0.1", spaced.display())),
        (
            "Spaced missing",
            r"C:\Nope\Missing App\missing app.exe /x".to_string(),
        ),
        (
            "System root",
            r"%SystemRoot%\System32\PING.EXE -n 1 127.0.0.1".to_string(),
        ),
        (
            "Rundll",
            "rundll32.exe shell32.dll,Control_RunDLL".to_string(),
        ),
        (
            "PowerShell",
            r"powershell.exe -NoProfile -File C:\Nope\startup.ps1".to_string(),
        ),
        // `cmd` without ".exe" is still the host: what it starts (PING.EXE, whose description
        // and publisher are readable) must not lend the entry its name.
        ("Cmd", "cmd /c start PING.EXE -n 1 127.0.0.1".to_string()),
        (
            "Cmd exe",
            r"C:\Windows\System32\cmd.exe /c echo hi".to_string(),
        ),
        ("Gone", r"C:\Nope\gone.exe".to_string()),
        (
            "Unknown variable",
            r"%NEGOTIUM_NO_SUCH_VAR%\x.exe".to_string(),
        ),
        ("Blank", String::new()),
    ];
    for (name, command) in &commands {
        registry::set_string(&r.user_run, name, command).unwrap();
    }

    let (entries, errors) = list_startup(r);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(entries.len(), commands.len());
    for (name, command) in &commands {
        let e = find(&entries, name);
        assert_eq!(&e.command, command);
        assert!(e.enabled && e.changeable, "{e:?}");
    }
    for name in ["Spaced", "System root"] {
        let e = find(&entries, name);
        assert_eq!(e.name, "TCP/IP Ping Command", "{e:?}");
        assert!(e.publisher.is_some(), "{e:?}");
    }
    // Host programs exist and carry Microsoft's name, but say nothing of what they run: unknown.
    for name in [
        "Rundll",
        "PowerShell",
        "Cmd",
        "Cmd exe",
        "Spaced missing",
        "Gone",
        "Unknown variable",
        "Blank",
    ] {
        let e = find(&entries, name);
        assert_eq!((e.name.as_str(), e.publisher.as_deref()), (name, None));
    }

    for e in &entries {
        set_startup_enabled(r, e, false).unwrap();
    }
    assert!(list_startup(r).0.iter().all(|e| !e.enabled));
    for e in &entries {
        set_startup_enabled(r, e, true).unwrap();
    }
    assert!(list_startup(r).0.iter().all(|e| e.enabled));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_command_is_listed_under_its_value_name() {
    let sb = Sandbox::new("blank");
    registry::set_string(&sb.roots.user_run, "Blank", "").unwrap();
    let (entries, _) = list_startup(&sb.roots);
    let blank = find(&entries, "Blank");
    assert_eq!(
        (
            blank.name.as_str(),
            blank.command.as_str(),
            blank.publisher.as_deref()
        ),
        ("Blank", "", None)
    );
    assert!(blank.enabled);
}

#[test]
fn missing_lists_are_empty_not_errors() {
    let sb = Sandbox::new("empty");
    let (entries, errors) = list_startup(&sb.roots);
    assert!(
        entries.is_empty() && errors.is_empty(),
        "{entries:?} {errors:?}"
    );
}

/// Review Focus 3: reading the all-users lists and the Startup folders needs no admin, so the real
/// lists read cleanly. "Skipped …" lines (say, a Run value that isn't a command) are information
/// about one value, not read errors.
#[test]
fn the_real_lists_can_be_read_without_changing_them() {
    let roots = StartupRoots::system();
    assert!(
        roots.user_folder.is_some() && roots.machine_folder.is_some(),
        "{roots:?}"
    );
    let (entries, errors) = list_startup(&roots);
    let read_errors: Vec<&String> = errors.iter().filter(|e| is_read_error(e)).collect();
    assert!(read_errors.is_empty(), "{read_errors:?}");
    for e in &entries {
        assert!(!e.name.is_empty() && !e.location.is_empty(), "{e:?}");
    }
}

/// Review Focus 3: a write that fails at write time comes back as an error, never a panic, and
/// leaves the listed state as it was.
#[test]
fn a_write_that_fails_returns_an_error() {
    let sb = Sandbox::new("fails");
    let r = &sb.roots;
    // Value names are limited to 16,383 characters: Windows refuses this one when it is written.
    let long = "x".repeat(20_000);
    let entry = StartupEntry {
        source: StartupSource::UserRun,
        key: long,
        name: "Long".to_string(),
        publisher: None,
        enabled: true,
        changeable: true,
        command: String::new(),
        location: r.user_run.display(),
    };
    match set_startup_enabled(r, &entry, false) {
        Err(ActionError::Other(text)) => {
            assert!(!text.is_empty() && !text.ends_with('.'), "{text:?}")
        }
        other => panic!("expected a readable error, got {other:?}"),
    }
    assert!(
        registry::values(&r.user_approved.child("Run"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_unreadable_approved_key_still_lists_the_run_entries_and_says_so() {
    let sb = Sandbox::new("unreadable");
    let mut roots = sb.roots.clone();
    registry::set_string(&roots.user_run, "Tool", r"C:\Nope\tool.exe").unwrap();
    // A key name over 255 characters can't be opened: the StartupApproved read fails.
    roots.user_approved = RegKey::new(
        Hive::CurrentUser,
        &format!(r"{}\{}", sb.base.path, "k".repeat(300)),
    );
    let folder = roots.user_folder.clone().unwrap();
    std::fs::write(folder.join("tool.bat"), "@echo off").unwrap();
    let (entries, errors) = list_startup(&roots);
    for key in ["Tool", "tool.bat"] {
        assert!(
            find(&entries, key).enabled,
            "listed, with the default state"
        );
    }
    assert_eq!(errors.len(), 2, "one per list: {errors:?}");
    for (line, list) in errors.iter().zip([r"\Run: ", r"\StartupFolder: "]) {
        assert!(
            line.starts_with(r"Couldn't read HKCU\")
                && line.contains(list)
                && !line.contains("Other(")
                && is_read_error(line),
            "plain English: {errors:?}"
        );
    }
}

#[test]
fn large_run_values_are_read_whole_and_odd_ones_are_reported() {
    let sb = Sandbox::new("large");
    let r = &sb.roots;
    let long = format!(r"C:\Nope\big.exe {}", "a".repeat(40_000)); // 80 KB as UTF-16
    registry::set_string(&r.user_run, "Big", &long).unwrap();
    registry::set_dword(&r.user_run, "Number", 1).unwrap();
    let (entries, errors) = list_startup(r);
    assert_eq!(entries.len(), 1);
    assert_eq!(find(&entries, "Big").command, long);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with(r"Skipped Number in HKCU\") && !is_read_error(&errors[0]),
        "{errors:?}"
    );
}

#[test]
fn values_over_a_megabyte_are_skipped_and_named() {
    let sb = Sandbox::new("huge");
    let r = &sb.roots;
    let huge = format!(r"C:\Nope\huge.exe {}", "a".repeat(600_000)); // 1.2 MB as UTF-16
    registry::set_string(&r.user_run, "Huge", &huge).unwrap();
    registry::set_string(&r.user_run, "Small", r"C:\Nope\small.exe").unwrap();
    let (entries, errors) = list_startup(r);
    assert_eq!(entries.len(), 1, "{entries:?}");
    find(&entries, "Small");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with(r"Skipped Huge in HKCU\") && errors[0].contains("1 MB"),
        "{errors:?}"
    );
}

#[test]
fn startup_folder_entries_resolve_shortcuts_and_toggle_under_startup_folder() {
    let sb = Sandbox::new("folders");
    let r = &sb.roots;
    let user = r.user_folder.clone().unwrap();
    write_shortcut(&user.join("Ping test.lnk"), PING, "-n 1 127.0.0.1").unwrap();
    std::fs::write(user.join("desktop.ini"), "[.ShellClassInfo]").unwrap();
    std::fs::write(user.join("notes.bat"), "@echo off").unwrap();
    std::fs::write(
        r.machine_folder.clone().unwrap().join("tool.bat"),
        "@echo off",
    )
    .unwrap();

    let (entries, errors) = list_startup(r);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(entries.len(), 3, "desktop.ini is not an entry: {entries:?}");
    let ping = find(&entries, "Ping test.lnk").clone();
    assert_eq!(ping.source, StartupSource::UserFolder);
    assert!(
        ping.command.to_ascii_uppercase().contains("PING.EXE") && ping.command.contains("-n 1"),
        "{}",
        ping.command
    );
    assert!(
        ping.publisher.is_some(),
        "the shortcut's target is read for the publisher"
    );
    let notes = find(&entries, "notes.bat");
    assert_eq!(notes.name, "notes");
    let tool = find(&entries, "tool.bat");
    assert_eq!(tool.source, StartupSource::MachineFolder);
    assert!(tool.source.needs_admin());

    set_startup_enabled(r, &ping, false).unwrap();
    assert_eq!(
        approved(&r.user_approved.child("StartupFolder"), "Ping test.lnk")[0],
        3
    );
    assert!(!find(&list_startup(r).0, "Ping test.lnk").enabled);
}

/// A shortcut to a host program, one whose target is gone and one that can't be resolved are
/// listed under their own names without a publisher, and still toggle (the all-users folder
/// under HKLM's StartupFolder key, here the sandbox's).
#[test]
fn host_and_broken_shortcuts_show_their_file_name() {
    let sb = Sandbox::new("hostlinks");
    let r = &sb.roots;
    let user = r.user_folder.clone().unwrap();
    let machine = r.machine_folder.clone().unwrap();
    let cmd = format!(
        r"{}\System32\cmd.exe",
        std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string())
    );
    assert!(
        VersionInfo::read(&cmd)
            .and_then(|i| i.string("CompanyName"))
            .is_some(),
        "cmd.exe has a publisher of its own, which the host check hides"
    );
    write_shortcut(&user.join("Script.lnk"), &cmd, "/c echo hi").unwrap();
    write_shortcut(&user.join("Gone.lnk"), r"C:\Nope\gone.exe", "").unwrap();
    std::fs::write(machine.join("Broken.lnk"), b"not a shortcut").unwrap();

    let (entries, errors) = list_startup(r);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(entries.len(), 3, "{entries:?}");
    for (key, name) in [
        ("Script.lnk", "Script"),
        ("Gone.lnk", "Gone"),
        ("Broken.lnk", "Broken"),
    ] {
        let e = find(&entries, key);
        assert_eq!(
            (e.name.as_str(), e.publisher.as_deref()),
            (name, None),
            "{e:?}"
        );
        assert!(e.enabled && e.changeable, "{e:?}");
    }
    let script = find(&entries, "Script.lnk");
    assert!(
        script
            .command
            .to_ascii_lowercase()
            .ends_with(r"\cmd.exe /c echo hi"),
        "{}",
        script.command
    );
    let broken = find(&entries, "Broken.lnk").clone();
    assert_eq!(broken.source, StartupSource::MachineFolder);
    assert_eq!(broken.command, "");
    set_startup_enabled(r, &broken, false).unwrap();
    assert_eq!(
        approved(&r.machine_approved.child("StartupFolder"), "Broken.lnk")[0],
        3
    );
    assert!(!find(&list_startup(r).0, "Broken.lnk").enabled);
}

#[test]
fn missing_startup_folders_are_empty_not_errors() {
    let sb = Sandbox::new("nofolders");
    let roots = sb
        .roots
        .clone()
        .with_folders(sb.dir.join("no-user"), sb.dir.join("no-machine"));
    let (entries, errors) = list_startup(&roots);
    assert!(
        entries.is_empty() && errors.is_empty(),
        "{entries:?} {errors:?}"
    );
}

#[test]
fn the_cleanup_helper_refuses_anything_outside_the_test_root() {
    assert!(
        registry::delete_test_tree(&RegKey::new(Hive::CurrentUser, r"Software\Microsoft")).is_err()
    );
    assert!(
        registry::delete_test_tree(&RegKey::new(
            Hive::LocalMachine,
            &format!(r"{}\x", registry::TEST_ROOT)
        ))
        .is_err()
    );
    assert!(
        registry::delete_test_tree(&RegKey::new(Hive::CurrentUser, registry::TEST_ROOT)).is_err(),
        "not the root itself"
    );
    assert!(
        registry::delete_test_tree(&RegKey::new(
            Hive::CurrentUser,
            r"Software\Negotium-TestX\y"
        ))
        .is_err()
    );
}
