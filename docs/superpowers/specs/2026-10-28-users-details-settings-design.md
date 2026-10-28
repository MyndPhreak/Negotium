# Users page, fuller Details popup, grouped Settings: design

Date: 2026-10-28
Status: agreed in brainstorming, awaiting plan

## Why

The user compared Negotium with Windows Task Manager and found three gaps: a Users tab, a Details tab, and a Settings page that is one flat list. Each one gets a different answer:

- **Users.** Task Manager's Users tab gives no useful information about your own account. Negotium gets a Users page that tells you about your account and shows anyone else signed in. Most PCs have one person, but some don't.
- **Details.** No separate Details page. Negotium's existing per-app Details popup becomes the place for everything about one app. The process context menu gains the Task Manager Details-menu items worth having.
- **Settings.** The 20 settings get grouped into collapsible sections on one page. Task Manager windows are usually narrow, so a side category list (Windows Settings style) was rejected.

## Order of work

Three independent pieces, shipped in this order (smallest first):

1. Grouped Settings
2. Fuller Details popup and new context menu items
3. Users page

Each piece is its own phase with its own PR.

## Constraints (all pieces)

- Golden rules apply: plan first, red/green commits. **Exception (user decision): no feature flags in Negotium.** Each piece ships switched on.
- Base branch is `main`. The acrylic tint work (Tint colour, Tint opacity) is already on `main` via PR #33.
- UI text follows ASD-STE100 Simplified Technical English. Record any departure as a deviation.
- Fields Windows refuses to give (access denied, protected process) show a dash, as the current popup does ([dispatch.rs:126](../../../crates/app/src/dispatch.rs)). A section never disappears because one field failed.
- Must work at the window widths people use for a task manager (narrow). No layout may need a wider window than today's pages.
- Settings file compatibility stays as it is: new fields take defaults, unknown fields are ignored ([settings.rs:1](../../../crates/app/src/settings.rs)).

---

## 1. Grouped Settings

### What the user gets

One page, same width as today. The existing settings sit in seven groups. Each group header shows its name and a one-line summary of what's inside. Clicking the header opens or closes the group.

| Group | Settings |
|---|---|
| Appearance | Theme, Backdrop, Tint colour, Tint opacity, Smooth graphs |
| Window and tray | Always on top, Keep running in the tray when closed, Show CPU in tray icon |
| Processes | Refresh rate, Group apps by type, Aequilibrium |
| Startup and shortcuts | Start with Windows, Admin mode, Open on Ctrl+Shift+Esc, Pick a window to end on Ctrl+Alt+Shift+Esc |
| Updates | Check for updates on startup, Check for updates now |
| Add-ons | CPU temperature add-on |
| Advanced | Use GPU rendering, Open Windows Task Manager |

### Decided

- Layout A from brainstorming: one page with collapsible groups, not a category sidebar.
- The grouping above (the user saw it and kept it).
- Negotium remembers which groups are open, across restarts.
- No setting changes behaviour. Only its position changes.

### My calls (the user did not decide these)

- On first run every group is open, so nothing looks hidden.
- A search box at the top filters rows as you type and opens the groups that have a match. When the search box is cleared, groups return to their remembered open/closed state.
- Group headers are keyboard-focusable buttons with the expanded/collapsed state exposed to accessibility.
- Open/closed state is stored in settings.json as a list of collapsed group ids (so a group added later defaults to open).

### Left to the builder

- The exact summary text per group.
- Whether opening and closing animates.
- Whether search matches setting titles only or also their descriptions.
- Whether the search box itself can be removed from scope if it makes the phase too big (record as a follow-up if so).

---

## 2. Fuller Details popup and context menu

### What the user gets

The Details item on an app's menu opens a larger, scrollable popup. Today it shows six lines of plain text for the app's first process only. The new popup has these sections, top to bottom (the user kept all six):

1. **Header.** App icon, name, publisher, file version. Chips for: signature check result (signed by X / not signed), 32-bit / 64-bit / ARM64 or emulated, running as admin or not, efficiency mode on/off. A UAC virtualization chip appears only when it is on.
2. **Processes in this app.** Every process in the app with name, PID, a role hint where known, CPU and memory. Selecting a process switches every section below to that process. The app's main process is selected first.
3. **Identity.** Path, command line, start time and running time, parent process (name and PID), user. A **Copy all** button copies the section as text.
4. **Resources.** Live CPU, memory, disk, network, GPU, threads, handles, peak memory, for the selected process.
5. **Settings.** Priority and affinity with **Change** buttons that open the existing priority and affinity controls.
6. **Network connections.** Protocol, local and remote address, port, state. List only; no action to close a connection.
7. **Services** (my call, from "Go to service(s)" below). When the selected process hosts Windows services, list them.

Bottom buttons: Open file location, Close.

### New context menu items

Today's menu ([applist.slint:319](../../../crates/app/ui/applist.slint)): End task, End process tree, Efficiency mode, Set CPU affinity, Set CPU priority, Exclude from Aequilibrium, Open file location, Details.

Add:

| Item | Behaviour |
|---|---|
| Search online | Opens the default browser on a web search for the process's file name (for example `discord.exe`). |
| Properties | Opens the Windows file Properties dialog for the process's file. |
| Go to service(s) | Shown only when the process hosts services. Switches to the Services page with its search set to that PID (the Services search already matches PIDs, [servicespage.slint:68](../../../crates/app/ui/servicespage.slint)). |
| Create memory dump file | Writes a full memory dump of the process, then shows a notification with an Open folder button. The user, a developer, wanted it. |

Not added (decided): Analyze wait chain, Debug, UAC virtualization toggle, Provide feedback.

### My calls

- Numbers in the popup update live at the app's refresh rate while it is open.
- The popup grows and scrolls; it never needs the window to be wider.
- Memory dumps go to the user's Temp folder, named `<process>_<pid>_<timestamp>.dmp`, as Task Manager does. Dumping a process the user cannot open shows the usual admin-rights message.
- The menu items act on the app's main process (the same process Details uses today).
- The priority and affinity **Change** buttons reuse today's dialogs unchanged: they act on the whole app, whichever process is selected. The Settings section labels this ("Applies to all processes in this app"). The dialog opens on top of the popup; the popup stays open and refreshes after.

### Left to the builder

- How the signature check, file version, architecture and network connection list are read (APIs of the builder's choice).
- Exact menu order and separators.
- Whether the dump runs on an action thread with a "Creating dump…" notification first. It must not freeze the UI.
- How the role hint for processes is found, or whether to drop it when nothing reliable exists.
- The existing data the popup can reuse: per-process CPU, disk, GPU, network, parent PID and create time are already in the model ([model lib.rs:22](../../../crates/model/src/lib.rs)).

---

## 3. Users page

### What the user gets

A new **Users** entry on the left rail ([rail.slint](../../../crates/app/ui/rail.slint)) and a page with two parts (layout A from brainstorming).

**Top card: you.**
- Avatar (account picture, or initial when none), display name, email for a Microsoft account or the local account name.
- Chips: account type (Administrator / Standard), how long you have been signed in, where (this PC or remote).
- Details: profile folder and its size, sign-in methods (password, PIN, fingerprint, face), when the password was last set, groups the account is in, and whether Negotium itself runs as admin.
- Buttons: Lock, Switch user, Sign out (asks to confirm first), Open account settings (opens Windows Settings → Accounts).

**Below: other signed-in users.**
- One row per other signed-in session: name, account type, state (active / disconnected), idle time.
- Each row has a Sign out or Disconnect button. These need admin rights; without them the buttons say so instead of failing silently.
- When nobody else is signed in: "No other users are signed in."

### Decided

- Layout A ("account first"), not the resource-share layout.
- Built for everyone, even though the user is the only person on their own PC.
- No account creation, deletion or editing. "Open account settings" hands that to Windows.

### My calls

- Sign out of your own session asks to confirm first, because unsaved work is lost.
- Sign out and Disconnect for another user also ask to confirm first. Sign out says that user's unsaved work is lost.
- Without admin rights, those buttons show the existing "Restart as admin" path (the same banner action used elsewhere) rather than only an error.
- Whether each other-user row also shows that user's CPU and memory is left open (see below).

### Left to the builder

- How each field is read. Any field Windows refuses shows a dash. Profile folder size is computed in the background and can show "Calculating…" first.
- Whether other-user rows show per-user CPU and memory (nice to have, drop if costly).
- Refresh rate of the page (it does not need the process refresh rate).

## Out of scope (all pieces)

- A separate Details page or a flat table of every .exe.
- Managing user accounts.
- Closing network connections.
- Wait chain analysis, debugger launch, UAC virtualization toggle.
