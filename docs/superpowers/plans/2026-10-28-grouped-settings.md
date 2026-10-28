# Grouped Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put the Settings page's rows into seven collapsible groups on one page, with a search box, and remember which groups are open.

**Architecture:** Rust owns the state. `Settings` gains a list of collapsed group ids, and the page receives a `[bool]` of open groups. A new `SettingGroup` Slint component wraps the existing `SettingRow`s. Search text lives in Slint. Matching is a Rust pure callback, so it is unit-tested and case-insensitive.

**Tech Stack:** Rust, Slint 1.18.1, serde_json settings file, `i_slint_backend_testing` UI tests.

**Spec:** [docs/superpowers/specs/2026-10-28-users-details-settings-design.md](../specs/2026-10-28-users-details-settings-design.md), section 1.

## Summary

Today the Settings page ([settingspage.slint:298](../../../crates/app/ui/settingspage.slint)) is one flat column of about 20 `SettingRow`s. This phase groups them into seven sections. Each section has a header (name, one-line summary, chevron) that opens and closes it. The open/closed state is saved in `settings.json`. A search box above the groups hides rows that don't match and opens every group with a match. Clearing the search box returns each group to its saved state. No setting changes behaviour, only position.

## Global Constraints

- No feature flags in Negotium (user decision, 2026-10-28). The grouped page ships on.
- Red/green commits per task: `Add failing tests for <area>`, then `Implement <area>`. Each red commit must compile and fail on an assertion.
- UI text in ASD-STE100 Simplified Technical English. The exact copy is fixed in the table below.
- Narrow windows: nothing may need a window wider than today's minimum. Existing test `settings_rows_wrap_without_overlap_at_the_smallest_window` (760 × 440) must keep passing.
- Settings-file compatibility: new fields take defaults; unknown values load. Existing rule at [settings.rs:1](../../../crates/app/src/settings.rs).
- Build into a scratch target dir if `target\release\negotium.exe` is locked by the user's running copy. Never rename the exe aside, and never close the user's instance.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

### Groups, ids and copy (exact)

| Index | Id | Header | Summary | Rows (element ids, in order) |
|---|---|---|---|---|
| 0 | `appearance` | Appearance | Theme, backdrop, tint and graphs | theme-row, backdrop-row, tint-row, opacity-row, smooth-row |
| 1 | `window` | Window and tray | Always on top and the tray icon | top-row, tray-row, tray-cpu-row |
| 2 | `processes` | Processes | Refresh rate, grouping and Aequilibrium | refresh-row, group-row, aequilibrium-row |
| 3 | `startup` | Startup and shortcuts | Start with Windows, admin mode and shortcut keys | start-row, admin-row, hotkey-row, picker-row |
| 4 | `updates` | Updates | Update checks | updates-row, check-row |
| 5 | `addons` | Add-ons | CPU temperature | the "Coming in a later version" text, temp-row |
| 6 | `advanced` | Advanced | GPU rendering and Windows Task Manager | gpu-row, taskmgr-row |

- The version text stays at the bottom of the page, outside every group.
- Search box placeholder and accessible label: `Search settings`.
- No-match text: `No settings match your search.`

## Architectural Decisions

- **Collapsed ids, not open ids, are stored.** A group added in a later version then opens by default, and on first run every group is open (spec: "On first run every group is open").
- **Ids not indexes in the file.** Reordering groups later doesn't scramble saved state. Unknown ids in the file are kept on save, so a newer version's state survives a round trip through an older one.
- **Matching in Rust (pure callback), filtering in Slint.** Slint 1.18 has `to-lowercase()` but no substring search. A `pure callback setting-matches(query, title, description) -> bool` follows the existing `pure callback redraw-hz` pattern ([graph.slint:31](../../../crates/app/ui/graph.slint)).
- **Search matches title and description.** This was left to the builder; a description match helps people who search for what a setting does ("blur", "CPU").
- **Search does not save.** Groups opened by a search are not written to `collapsed_groups`.
- **No animation** on open and close. This was left to the builder; skipping it keeps `a_tall_window_leaves_the_settings_their_size_and_place` and the layout tests deterministic.
- **Accessibility:** the header is a `FocusButton` with `accessible-expandable: true` and `accessible-expanded` bound to its state. The testing backend can invoke it with `invoke_accessible_default_action`.

## Persisted shape (contract)

`settings.json` gains one field:

```json
{ "collapsed_groups": ["updates", "advanced"] }
```

- Type `Vec<String>`, `#[serde(default)]` gives `[]`, so every group is open.
- No version field: the file has none today, and serde defaults plus ignored unknown fields cover old and new files ([settings.rs:1](../../../crates/app/src/settings.rs)).
- Ids not in `SETTING_GROUPS` are ignored on read and kept on write.

## Files

| File | Change |
|---|---|
| `crates/app/src/settings.rs` | `SETTING_GROUPS`, `collapsed_groups` field, `groups_open`, `toggle_group`, `setting_matches`, unit tests |
| `crates/app/ui/settingspage.slint` | `SettingGroup` component; rows moved into seven groups; search box; no-match text; new properties and callbacks |
| `crates/app/ui/main.slint` | `settings-groups-open` property, `settings-group-toggled(int)` callback, `setting-matches` pure callback, all passed through to `SettingsPage` |
| `crates/app/src/settingspage.rs` | `show_settings` sets `settings_groups_open`; wire `on_settings_group_toggled` (toggle, show, save) and `on_setting_matches` |
| `crates/app/tests/ui.rs` | New UI tests (below). Update `row_of` if a row id moves scope |

## Test Plan

`settings.rs` unit tests (`cargo test -p negotium --lib settings::`)
- default settings → all seven groups open
- `collapsed_groups = ["updates"]` → only index 4 closed
- toggle group 4 twice → `collapsed_groups` back to its starting value
- toggle keeps unknown ids: `["future", "updates"]`, toggle 4 → `["future"]`
- toggle with an out-of-range index → no change
- a file with `collapsed_groups` round-trips through serde; a file without it loads as `[]`
- `setting_matches("", t, d)` → true
- case-insensitive title match: `("TRAY", "Show CPU in tray icon", "")` → true
- description match: `("blur", "Backdrop", "...blurs the windows...")` → true
- surrounding whitespace ignored: `("  theme ", "Theme", "")` → true
- no match → false

`tests/ui.rs` (`cargo test -p negotium --test ui`)
- `settings_groups_are_listed_in_order_and_open_by_default`: seven headers with the exact names, in table order, each with `accessible_expanded == Some(true)`
- `each_setting_sits_in_its_group`: for every row in the table, its control's y lies between its header's y and the next header's y
- `a_group_header_closes_and_reopens_its_group_and_saves_it`: activate "Updates" → `Check for updates on startup` is not found, `accessible_expanded == Some(false)`, file saves `["updates"]`; activate again → row found, file saves `[]`
- `closed_groups_are_restored_at_launch`: `apply_settings` with `collapsed_groups: ["advanced"]` → "Use GPU rendering" not found, "Theme" found
- `search_shows_only_matching_rows_and_opens_their_groups`: with "Window and tray" closed, type `tray` → both tray switches found, "Theme" not found, Window and tray expanded
- `search_matches_descriptions`: type `blur` → Backdrop choice found
- `a_search_with_no_match_says_so`: type `zzz` → `No settings match your search.` found, no group header found
- `clearing_the_search_restores_saved_groups_and_saves_nothing`: closed "Updates", search `update`, clear → Updates closed again; file still `["updates"]`
- `an_open_modal_disables_the_group_headers_and_search`: `modal-open` → each header and the search box `accessible_enabled == Some(false)`
- existing tests keep passing unchanged, including `the_settings_page_reports_each_choice`, `settings_rows_wrap_without_overlap_at_the_smallest_window`, `a_tall_window_leaves_the_settings_their_size_and_place`, `apply_settings_sets_every_wired_property`

Not test-first: visual look of the header (chevron, spacing). Covered by manual check 1 under Definition of Done.

## Review Focus

1. **Acrylic-only rows under search.** With Mica, the Tint rows don't exist. Searching `tint` must show the no-match text, not an empty Appearance header. Test in Task 3: `a_search_for_a_hidden_row_says_no_match`.
2. **Tab reveal inside a group.** Tabbing to a control in a lower group still scrolls its row into view (`sv.reveal` with nested `absolute-position`). Test in Task 2: `tab_to_a_row_in_a_lower_group_scrolls_it_into_view` (small window, focus `Open Windows Task Manager`, assert its rect is within the viewport).
3. **Narrow window header.** At 760 × 440 the summary wraps under the title and never overlaps the chevron. Test in Task 2: `group_headers_wrap_at_the_smallest_window`.
4. **Settings from a newer version.** Unknown ids survive a toggle and save (unit test above).
5. **Search while a group is collapsed by the user, then a row toggled inside it.** Changing a setting during a search must not alter `collapsed_groups`. Covered by `clearing_the_search_restores_saved_groups_and_saves_nothing`; extend it to toggle `Check for updates on startup` while searching (installed fake or `available` false is fine; assert only `collapsed_groups`).

---

### Task 1: Group state and matching in `Settings`

**Files:**
- Modify: `crates/app/src/settings.rs` (struct at :59, `impl Settings` at :155, `mod tests` at :447)

**Interfaces:**
- Produces:
  - `pub const SETTING_GROUPS: [&str; 7] = ["appearance", "window", "processes", "startup", "updates", "addons", "advanced"];`
  - `Settings::collapsed_groups: Vec<String>` (serde default `[]`)
  - `impl Settings { pub fn groups_open(&self) -> [bool; 7]; pub fn toggle_group(&mut self, index: usize); }`
  - `pub fn setting_matches(query: &str, title: &str, description: &str) -> bool`

- [ ] **Step 1: Write the failing unit tests** listed under `settings.rs unit tests`, with stubs that compile: field added, `groups_open` returns `[false; 7]`, `toggle_group` does nothing, `setting_matches` returns `false`.
- [ ] **Step 2: Run** `cargo test -p negotium --lib settings::` and confirm the new tests fail on assertions.
- [ ] **Step 3: Commit** `Add failing tests for settings group state`.
- [ ] **Step 4: Implement.** `toggle_group` adds or removes only `SETTING_GROUPS[index]`, keeping other entries and their order. `setting_matches` trims the query, lowercases both sides, and checks substring in title or description.
- [ ] **Step 5: Run** the same command; all pass. Then run `cargo test -p negotium --lib` (no regressions).
- [ ] **Step 6: Commit** `Implement settings group state`.

### Task 2: Collapsible groups on the page

**Files:**
- Modify: `crates/app/ui/settingspage.slint`, `crates/app/ui/main.slint`, `crates/app/src/settingspage.rs`
- Test: `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: Task 1's `groups_open`, `toggle_group`.
- Produces (Slint):
  - `component SettingGroup` with `in property <string> title`, `in property <string> summary`, `in property <bool> open`, `in property <bool> interactive`, `callback toggled()`, `@children` shown only when `open`.
  - `SettingsPage`: `in property <[bool]> groups-open`, `callback group-toggled(int)`.
  - `MainWindow`: `in-out property <[bool]> settings-groups-open`, `callback settings-group-toggled(int)`. Rust setter `set_settings_groups_open(ModelRc<bool>)`.

- [ ] **Step 1: Write the failing UI tests:** `settings_groups_are_listed_in_order_and_open_by_default`, `each_setting_sits_in_its_group`, `a_group_header_closes_and_reopens_its_group_and_saves_it`, `closed_groups_are_restored_at_launch`, `an_open_modal_disables_the_group_headers_and_search` (headers part only), `tab_to_a_row_in_a_lower_group_scrolls_it_into_view`, `group_headers_wrap_at_the_smallest_window`. Land `SettingGroup`, the properties and the callback as stubs, so the tests compile and fail on assertions. Add a helper `group_header(ui, name) -> El` next to `switch`/`radio` ([ui.rs:1969](../../../crates/app/tests/ui.rs)).
- [ ] **Step 2: Run** `cargo test -p negotium --test ui settings` and confirm the new tests fail on assertions.
- [ ] **Step 3: Commit** `Add failing tests for collapsible settings groups`.
- [ ] **Step 4: Implement.** Move each row into its group per the table, keeping each row's element id and its `focus-gained => sv.reveal(...)`. The header is a `FocusButton` holding the title (13px, 600), summary (11px, muted, word-wrap) and a chevron. It sets `accessible-role: button`, `accessible-label: title`, `accessible-description: summary`, `accessible-expandable: true` and `accessible-expanded: open`. Hide closed content with `if root.open : VerticalLayout { @children }` or an equivalent that takes no layout space. `show_settings` sets `settings_groups_open` from `groups_open()`. The toggle handler calls `toggle_group`, re-shows the state, and saves through the existing `Saver`, the same as the other handlers in `settingspage.rs`.
- [ ] **Step 5: Run** `cargo test -p negotium --test ui` (all, including existing Settings tests) and confirm everything passes.
- [ ] **Step 6: Commit** `Implement collapsible settings groups`.

### Task 3: Search

**Files:**
- Modify: `crates/app/ui/settingspage.slint`, `crates/app/ui/main.slint`, `crates/app/src/settingspage.rs`
- Test: `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: Task 1's `setting_matches`; Task 2's `SettingGroup`, `groups-open`.
- Produces (Slint):
  - `SettingsPage`: `pure callback setting-matches(string, string, string) -> bool`, private `property <string> search-text`.
  - `MainWindow`: `pure callback setting-matches(string, string, string) -> bool`, wired in Rust with `on_setting_matches` to `settings::setting_matches`.
  - A group is open when `search-text == "" ? groups-open[i] : (any row in it matches)`; a group with no matching row is hidden while searching. A row is hidden when it doesn't match.

- [ ] **Step 1: Write the failing UI tests:** `search_shows_only_matching_rows_and_opens_their_groups`, `search_matches_descriptions`, `a_search_with_no_match_says_so`, `clearing_the_search_restores_saved_groups_and_saves_nothing` (with the Review Focus 5 extension), `a_search_for_a_hidden_row_says_no_match`, and the search part of `an_open_modal_disables_the_group_headers_and_search`. Add the `LineEdit` and callback as stubs so they compile. Type into the box with the testing backend's `set_accessible_value` on the element labelled `Search settings`.
- [ ] **Step 2: Run** `cargo test -p negotium --test ui search` and confirm failures on assertions.
- [ ] **Step 3: Commit** `Add failing tests for settings search`.
- [ ] **Step 4: Implement.** Put a `LineEdit` (placeholder `Search settings`, `enabled: root.interactive`) under the page title, styled like the Services search ([servicespage.slint:65](../../../crates/app/ui/servicespage.slint)). Each row's visibility uses `root.setting-matches(search-text, title, description)` with that row's own strings. Show `No settings match your search.` when the search is not empty and no row matches. Never write to `groups-open` from search.
- [ ] **Step 5: Run** `cargo test -p negotium --test ui` and `cargo test -p negotium --lib`; all pass. Then run `cargo clippy -p negotium --all-targets -- -D warnings` with no warnings.
- [ ] **Step 6: Commit** `Implement settings search`.

---

## Out of Scope

- Changing what any setting does, its title or its description.
- New settings (the Details and Users phases add none here).
- Animation of opening and closing.
- Keyboard shortcut to focus the search box.
- The Details popup and Users page (later phases of the same spec).

## Definition of Done

- [ ] Seven groups in the table's order with the exact header and summary text: `settings_groups_are_listed_in_order_and_open_by_default`, manual 1
- [ ] Every row is in its table group: `each_setting_sits_in_its_group`
- [ ] Header click closes/opens and saves `collapsed_groups`: `a_group_header_closes_and_reopens_its_group_and_saves_it`
- [ ] Saved state is restored at launch: `closed_groups_are_restored_at_launch`
- [ ] First run has every group open: unit test "default settings → all seven groups open"
- [ ] Unknown ids survive a save: unit test "toggle keeps unknown ids"
- [ ] Search hides non-matching rows, opens matching groups, matches descriptions, is case-insensitive: the search UI tests and the `setting_matches` unit tests
- [ ] No-match text shown, including for Acrylic-only rows under Mica: `a_search_with_no_match_says_so`, `a_search_for_a_hidden_row_says_no_match`
- [ ] Clearing search restores saved state; search never saves: `clearing_the_search_restores_saved_groups_and_saves_nothing`
- [ ] Headers and search disabled under a modal: `an_open_modal_disables_the_group_headers_and_search`
- [ ] Narrow window works: `group_headers_wrap_at_the_smallest_window`, `settings_rows_wrap_without_overlap_at_the_smallest_window`
- [ ] Tab reveals rows in lower groups: `tab_to_a_row_in_a_lower_group_scrolls_it_into_view`
- [ ] All existing tests pass, clippy clean: Task 3 Step 5
- [ ] Manual 1: run the app (`scripts/try.ps1`). Open Settings in light and dark mode with Mica and Acrylic, close two groups, restart, and check they stay closed. Take a screenshot for the PR.

## Acceptance Criteria

- The Settings page shows seven named groups, each opening and closing from its header. → DoD 1–3
- Negotium remembers which groups are closed across restarts, and a fresh install shows all of them open. → DoD 4–5
- Settings files from older or newer versions load and keep their group state. → DoD 6
- Typing in the search box shows only matching settings and says so when none match; clearing it returns the page as it was. → DoD 7–9
- The page stays usable under a modal, in the smallest window, and by keyboard. → DoD 10–12
