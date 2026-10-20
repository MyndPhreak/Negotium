//! Safety critical exclusions from dynamic CPU restraint.
//! Throttling these processes can bugcheck the machine, desync audio, or crash desktop rendering.

pub const CRITICAL_SYSTEM_PROCESSES: &[&str] = &[
    "audiodg",          // Windows Audio Device Graph - must NEVER be throttled (causes audio crackling)
    "csrss",            // Client Server Runtime Subsystem
    "dwm",              // Desktop Window Manager
    "explorer",         // Windows Shell
    "fontdrvhost",      // Usermode Font Driver Host
    "lsass",            // Local Security Authority
    "negotium",         // Negotium itself
    "services",         // Service Control Manager
    "sihost",           // Shell Infrastructure Host
    "smss",             // Session Manager Subsystem
    "spoolsv",          // Print Spooler
    "system",           // NT Kernel & System
    "taskhostw",        // Host Process for Windows Tasks
    "wininit",          // Windows Initialization
    "winlogon",         // Windows Logon Process
];

pub fn is_safety_excluded(name: &str) -> bool {
    let stem = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name);
    CRITICAL_SYSTEM_PROCESSES.iter().any(|&c| c.eq_ignore_ascii_case(stem))
}

pub fn is_user_excluded(name: &str, user_exclusions: &[String]) -> bool {
    let stem = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name);
    user_exclusions.iter().any(|ex| {
        let ex_stem = ex.strip_suffix(".exe").or_else(|| ex.strip_suffix(".EXE")).unwrap_or(ex);
        ex_stem.eq_ignore_ascii_case(stem)
    })
}
