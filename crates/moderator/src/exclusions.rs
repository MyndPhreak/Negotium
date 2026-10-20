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

/// The image name without a trailing `.exe` in any letter case ("Game.Exe" -> "Game").
pub fn image_stem(name: &str) -> &str {
    match name.len().checked_sub(4) {
        Some(at) if name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(".exe") => &name[..at],
        _ => name,
    }
}

/// Whether two image names name the same program: case-insensitive, `.exe` optional on either.
pub fn same_image(a: &str, b: &str) -> bool {
    image_stem(a).eq_ignore_ascii_case(image_stem(b))
}

pub fn is_safety_excluded(name: &str) -> bool {
    CRITICAL_SYSTEM_PROCESSES.iter().any(|&c| same_image(c, name))
}

pub fn is_user_excluded(name: &str, user_exclusions: &[String]) -> bool {
    user_exclusions.iter().any(|ex| same_image(ex, name))
}
