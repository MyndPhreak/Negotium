# Negotium

A fast, modern Task Manager replacement for Windows 11, written in Rust with [Slint](https://slint.dev).

## What it does

- **Overview** — live CPU, memory, disk, GPU and network meters with smooth graphs, the top apps for the selected meter, and a sortable app list grouped into Apps, Background processes and Windows processes, with program icons and per-process detail.
- **Per-app network use** while running as administrator.
- **Performance** pages for CPU, memory, each disk, GPU and network adapter, with history and a hover list of the busiest apps.
- **Startup** apps: see and turn off what starts with Windows.
- **Actions** — end task, end process tree, efficiency mode, force-close frozen apps, open file location.
- **Tray** — keeps running in the tray with an optional CPU graph icon.

## Install

Download `negotium-X.Y.Z-x86_64.msi` from the [latest release](https://github.com/MyndPhreak/Negotium/releases/latest) and run it.

The installer is not code-signed yet, so Windows SmartScreen may warn you: choose **More info → Run anyway**.

The installer offers three optional extras (all off by default), also available later in Settings:

- **Open on Ctrl+Shift+Esc** — Windows opens Negotium wherever it would open Task Manager. Settings and the tray menu keep an "Open Windows Task Manager" button.
- **Admin mode (no prompt)** — Negotium starts with administrator rights without a UAC prompt each time (needs an administrator account). Anything that can start Negotium can then open it with admin rights, without a prompt.
- **Start with Windows** — starts hidden in the tray at sign-in.

Negotium checks GitHub for new releases at startup (switchable in Settings) and updates from the release's installer.

Uninstalling removes the program, the Ctrl+Shift+Esc redirect, the admin-mode task and the startup entry, and keeps your settings.

## Build from source

Requires Windows 10/11 x64 and the Rust toolchain pinned in `rust-toolchain.toml`.

```powershell
cargo build --release -p negotium
.\target\release\negotium.exe
```

Run the tests with `cargo test --workspace`. A development build runs from anywhere; admin mode, Ctrl+Shift+Esc, Start with Windows and updates are only offered by the installed copy.

## License

[MIT](LICENSE)
