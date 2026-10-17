# App List Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Overview's app list into a sortable, grouped list of Fluent cards with program icons, an × End task icon and processes that open inside their card.

**Architecture:** The list stays one flat, virtualised Slint model; each row gains a kind (section heading, app card, process row inside a card) so RowLayout's freezing and nav.rs keep working on a flat `Vec<RowView>`. Sorting generalises the model crate's `Ranker` from a metric to a `Sort` (column + direction); grouping is a stable partition of that order by `AppKind`. Icons are extracted on their own thread, cached by image path, and attached to rows by the bridge.

**Tech Stack:** Rust 2024, Slint 1.18.1, `windows` 0.62.2 (Shell/GDI for icons).

**Spec:** `docs/superpowers/specs/2026-10-16-negotium-app-list-redesign-design.md` (amends spec 5.1 item 3 of `2026-10-14-negotium-design.md`). Mockups: `.superpowers/brainstorm/56476-1792203480/content/table-styles.html` (option B) and `expanded-cards.html` (option "Inside the card").

## Global Constraints

- Every colour comes from `Theme` (`crates/app/ui/theme.slint`); light theme must look right.
- No `animation-tick()`; the ScrollIndicator stays in place of the Fluent scrollbar.
- Icon extraction never runs on the UI thread and never blocks a frame; each image path is decoded at most once per run.
- Tests never touch the user's real settings, log, renderer marker or registry, and never launch the real `negotium.exe` window. Never close or kill the user's running Negotium.
- If `target\release\negotium.exe` is locked, build into a scratch `CARGO_TARGET_DIR`; never rename the exe aside.
- Existing accessibility and keyboard behaviour stays: Tab into the list, arrows/Home/End/PageUp/PageDown, Left/Right expand/collapse, Enter/Space, Delete, Menu key / Shift+F10, the parking key handler, focus/hover freezing the list, and the existing accessible labels ("End task <name>", "Force close <name>", "Expand <name>", "Collapse <name>", "Search apps and processes", "Apps").
- Calm re-sorting rules stay: re-sort at most every `RESORT_INTERVAL_MS` (2000), never while frozen, immediately on an explicit sort change; frozen (hung) apps lead their section in every sort.
- Text sizes: app card name 13.5 px, app card second line 11.5 px, process rows 12.5 px, headers 11.5 px uppercase.
- Row heights (one source of truth, Rust constants mirrored in Slint, a test keeps them equal): section heading 32 px, app card 50 px (46 px card + 4 px gap), process row 30 px.

## Review Focus

1. **A frozen list holding a section heading or a card whose app exited:** rows hold their place, exited ones dim and stay inert; a held section heading must not toggle a different section. (Task 3 test `a_frozen_list_keeps_section_rows_in_place`.)
2. **Search that empties a section:** the heading disappears, counts show the matching apps only, and keyboard current row falls back as today. (Task 3 test `search_hides_empty_sections_and_counts_matches`.)
3. **Keyboard reveal with mixed heights:** PageDown/End onto a card far below must scroll exactly that card fully into view, including an expanded card's last process row. (Task 4 test `ensure_visible_uses_real_row_tops`.)
4. **An exe whose icon cannot be read** (access denied, protected process, path gone, `None` image path when not elevated): the row shows the generic glyph and the loader does not retry it every frame. (Task 5 test `a_failed_icon_is_remembered_as_missing`.)
5. **Sort by Name with equal or differently-cased names:** stable, case-insensitive A–Z with a deterministic tiebreak, so equal names don't swap every resort. (Task 1 test `name_sort_is_case_insensitive_and_stable`.)

---

### Task 1: Sort model

**Files:**
- Modify: `crates/model/src/ranking.rs`, `crates/model/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub enum SortColumn { Name, Metric(Metric) }` (Clone, Copy, Debug, PartialEq, Eq, Hash)
  - `pub struct Sort { pub column: SortColumn, pub descending: bool }` with `impl Default` = CPU descending, and `pub fn by(column: SortColumn) -> Sort` (the column's starting direction: metrics descending, Name ascending) and `pub fn flipped(self) -> Sort`
  - `Ranker::order(&mut self, rows: &[AppRow], sort: Sort, now_ms: u64, frozen: bool) -> Vec<usize>` (replaces the `metric` parameter; same calm rules; a changed `Sort` resorts immediately like a metric change did)
  - `pub fn compare_rows(a: &AppRow, b: &AppRow, sort: Sort) -> Ordering` (hung first, then the sort key, then a deterministic tiebreak) — reused for process rows in Task 3 through `pub fn compare_procs(a: &ProcRow, b: &ProcRow, sort: Sort) -> Ordering`
  - all re-exported from `negotium_model`.

- [ ] **Step 1: Write failing tests** in `ranking.rs`:
  - `name_sort_is_case_insensitive_and_stable`: rows "beta", "Alpha", "alpha" (different pids) under `Sort::by(SortColumn::Name)` order as `["Alpha", "alpha", "beta"]` (tiebreak: exact name, then key) and the same order on a second call after the interval; flipped gives the exact reverse of the non-hung part.
  - `ascending_metric_sort_puts_the_smallest_first_and_hung_still_leads`: cpu 5/1/9 with the 9 one hung, `Sort { column: Metric(Cpu), descending: false }` → hung first, then 1, 5.
  - `a_sort_change_resorts_at_once_even_while_frozen`: frozen = true, same `now_ms`, switching CPU desc → CPU asc reorders.
  - `sort_by_starts_metrics_descending_and_name_ascending`.
- [ ] **Step 2:** `cargo test -p negotium-model` — the new tests fail to compile.
- [ ] **Step 3:** Implement. `Metric(m)` compares `value(m)` with `total_cmp`; Name compares `to_lowercase()`; tiebreak by `name` then `key` (AppKey is `Ord`). Hung apps lead in both directions. Update the existing ranking tests and every caller (`crates/app/src/worker.rs` builds `Sort` from Controls in Task 2; until then pass `Sort { column: SortColumn::Metric(metric), descending: true }`).
- [ ] **Step 4:** `cargo test --workspace` passes.
- [ ] **Step 5:** Commit `feat(model): sort the app list by any column, either way`.

### Task 2: Sort and grouping state, settings and the Settings switch

**Files:**
- Modify: `crates/app/src/controls.rs`, `crates/app/src/settings.rs`, `crates/app/src/settingspage.rs`, `crates/app/src/worker.rs`, `crates/app/src/main.rs`, `crates/app/ui/main.slint`, `crates/app/ui/settingspage.slint`
- Test: `crates/app/src/controls.rs`, `crates/app/src/settings.rs` (unit), `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `Sort`, `SortColumn` (Task 1).
- Produces:
  - `Controls::set_sort(&self, s: Sort)`, `Controls::sort(&self) -> Sort`, `Controls::set_grouped(&self, on: bool)`, `Controls::grouped(&self) -> bool` (atomics, like `metric`).
  - `pub fn header_click(current: Sort, column: SortColumn) -> Sort` in `controls.rs`: the same column flips, another column starts at `Sort::by`.
  - Settings fields `sort_column: String` (one of `"name"`, `"cpu"`, `"memory"`, `"disk"`, `"gpu"`, `"network"`; unknown → default), `sort_descending: bool`, `group_by_type: bool`; defaults `"cpu"`, `true`, `true`; each tolerant-loaded with `field!` like the others; `Settings::sort(&self) -> Sort` and `Settings::set_sort(&mut self, s: Sort)`.
  - Slint: `MainWindow` gets `in-out property <int> sort-column` (−1 = Name, 0..4 = metric index), `in-out property <bool> sort-descending`, `in-out property <bool> settings-group-by-type: true`, `callback sort-clicked(int)` (same −1..4 encoding), `callback group-by-type-toggled(bool)`.
  - `FrameOptions` gains `pub sort: Sort` and `pub grouped: bool` (filled by the worker from Controls).

Behaviour (spec "Meters and headers stay in step"): `sort-clicked(c)` → `header_click`; if the new column is a metric, also do exactly what `select-metric` does (Controls metric + `selected-metric`). `select-metric(i)` → also sets the sort to `Sort::by(Metric(i))`. Both save settings through the existing `Saver`. At launch, the saved sort is applied and, when it is a metric, `selected-metric` starts on it.

- [ ] **Step 1: Write failing tests:**
  - `controls.rs`: `header_click_flips_the_same_column_and_starts_others_fresh` (CPU desc + CPU → CPU asc; CPU asc + Name → Name asc; Name asc + Name → Name desc; Name desc + Memory → Memory desc).
  - `settings.rs`: `sort_and_grouping_default_and_load_tolerantly` (missing → CPU desc grouped; `"sort_column": "banana"` → Partial with only that field lost; `"name"`/false/false round-trips through save + load).
  - `ui.rs`: `clicking_a_metric_header_selects_its_meter_and_clicking_again_flips` (invoke `sort-clicked(1)` via the wired window: `selected-metric` = 1, `sort-column` = 1, `sort-descending` true; again → false; `sort-clicked(-1)` leaves `selected-metric` at 1) and `selecting_a_meter_sorts_by_it_descending`. Use the existing test wiring helpers (`bridged()` / `wired()`), with settings saved to a temp dir only.
  - `ui.rs`: `the_group_by_type_switch_shows_and_reports_its_state` on the Settings page (accessible label "Group apps by type").
- [ ] **Step 2:** Run them — they fail.
- [ ] **Step 3:** Implement the state, settings fields, the wiring in `main.rs` / `settingspage.rs`, the worker's `FrameOptions`, and a Settings page switch row "Group apps by type" (subtitle "Apps, background processes and Windows processes under their own headings") next to the existing switches. The worker passes `controls.sort()` to `Ranker::order`.
- [ ] **Step 4:** `cargo test --workspace` passes.
- [ ] **Step 5:** Commit `feat(app): sort and grouping state, remembered, with a Settings switch`.

### Task 3: Rows: kinds, sections, bars, subtitles, sorted processes

**Files:**
- Modify: `crates/app/src/view.rs`, `crates/app/src/nav.rs`

**Interfaces:**
- Consumes: `FrameOptions { sort, grouped, .. }` (Task 2), `compare_procs` (Task 1).
- Produces (later tasks use these exact names):
  - `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub enum RowKind { Section, #[default] App, Proc }` and `RowView` fields: `kind: RowKind`, `sub: String` (app second line), `bars: [f32; 5]` (replaces `bar`), `icon: String` (image path, empty = none), `windows: bool` (the Windows group, draws the Windows glyph), `last_in_card: bool` (process row that closes its card).
  - Section ids: `pub const SECTION_APPS: i32 = i32::MAX - 2; pub const SECTION_BACKGROUND: i32 = i32::MAX - 1; pub const SECTION_WINDOWS: i32 = i32::MAX;` (never produced by `IdSeed`).
  - `pub const ROW_H_SECTION: f32 = 32.0; pub const ROW_H_APP: f32 = 50.0; pub const ROW_H_PROC: f32 = 30.0;` and `pub fn row_height(kind: RowKind) -> f32`.
  - `visible_rows(rows: &[RowView], expanded: &HashSet<i32>, collapsed: &HashSet<i32>, search: &str) -> Vec<RowView>` (new `collapsed` parameter: section ids the user closed).

Rules:
- Grouped: section heading rows "Apps", "Background processes", "Windows processes" (`kind: Section`, `name` = that text, `depth` 0, `actionable` false, `expandable` true) each followed by its apps in `order`, partitioned by `AppKind` stably. Not grouped: no section rows.
- `count` on a section is the number of its apps that `visible_rows` lets through (computed there, as `"(N)"`; the Windows section shows no count). A section with no visible apps is dropped. A collapsed section shows its heading only.
- App `sub`: `"N processes"` / `"1 process"`; a hung app: its `hung` text instead. `count` stays for the accessible label but the card shows `sub`.
- `bars` (spec "Bars"): CPU `cpu_pct/100`; GPU `gpu_pct/100` (0 when none); Memory `private_bytes / totals.memory.total` (0 when unknown); Disk and Network `value / max over rows this frame`; each clamped to 0..=1, NaN → 0. Process rows: all zero.
- Process rows of an app are ordered with `compare_procs(.., opts.sort)`; the last one has `last_in_card: true`.
- `icon`: the app's image path (its root process, else its first process), each process row its own; empty when unknown. `windows: true` on the Windows group row.
- nav.rs: a Section row is current-able; Left on an open section and Right on a closed one return `Toggle(section id)`; Enter/Space toggles; Delete and actions on a section return `Nothing`.

- [ ] **Step 1: Write failing tests** in `view.rs`:
  - `grouped_frames_put_apps_under_their_section_in_sort_order` (a Window app, a Background app, the Windows group → `[Section Apps, app, Section Background, bg, Section Windows, Windows]`; not grouped → no sections, `order` kept).
  - `search_hides_empty_sections_and_counts_matches` (search matching one background app → only that section heading with `"(1)"` and the app).
  - `a_collapsed_section_shows_its_heading_only`.
  - `bars_follow_the_spec` (cpu 12.7 → 0.127; mem 6 GB of 96 GB → 0.0625; disk relative to the busiest; NaN → 0).
  - `an_apps_sub_line_is_its_process_count_or_its_hang` (`"131 processes"`, `"1 process"`, hung → hung text).
  - `process_rows_follow_the_sort_and_the_last_closes_the_card`.
  - `a_frozen_list_keeps_section_rows_in_place` (RowLayout frozen, an app of section Apps exits → heading and rows keep their indices, the exited card is marked exited).
  - nav.rs: `left_and_right_close_and_open_a_section`, `delete_on_a_section_does_nothing`.
  - `row_heights_match_the_list` — reads `crates/app/ui/applist.slint` source and asserts it declares `32px`, `50px` and `30px` for section, app and process heights (keeps Rust and Slint in step; the property names are fixed in Task 6: `section-height`, `app-height`, `proc-height`). Mark `#[ignore]` with a reason until Task 6 lands, and un-ignore it there.
- [ ] **Step 2:** Run — fail.
- [ ] **Step 3:** Implement in `view.rs` / `nav.rs`. Section rows are `depth == 0` and not actionable, like the Windows group: every existing `depth == 0 && !actionable` check in `bridge.rs` (`auto_expand_hung_groups`, `key_focus_holds`) must also require `kind == RowKind::App`. Update `bridge.rs` callers of `visible_rows` to pass an empty `collapsed` set for now and `RowData` mapping to keep compiling (map `bars[metric]` into the old `bar`).
- [ ] **Step 4:** `cargo test --workspace` passes.
- [ ] **Step 5:** Commit `feat(app): rows know their kind: sections, cards, process rows, bars`.

### Task 4: Bridge: collapsed sections, hung reopening, real row tops

**Files:**
- Modify: `crates/app/src/bridge.rs`, `crates/app/ui/applist.slint` (only the `ensure-visible` function's signature), `crates/app/ui/main.slint` (the `ensure-row-visible` callback)
- Test: `crates/app/src/bridge.rs` (unit), `crates/app/tests/ui.rs`

**Interfaces:**
- Consumes: `RowKind`, section ids, `row_height`, `visible_rows(.., collapsed, ..)` (Task 3).
- Produces:
  - `Bridge.collapsed: HashSet<i32>`; `Bridge::toggle(id)` routes a section id to `collapsed` (no worker publish needed) and an app id to `expanded` as today.
  - `pub fn reopen_hung_sections(rows: &[RowView], collapsed: &mut HashSet<i32>, reopened: &mut HashSet<i32>) -> bool` — same rule as `auto_expand_hung_groups`: a section with a hung app opens once when the hang starts; if the user closes it again it stays closed until that section has no hung app.
  - `pub fn row_top(rows: &[RowView], i: usize) -> (f32, f32)` — y of row `i` and its height, by summing `row_height(kind)`.
  - Slint `ensure-row-visible(y: length, h: length)` replaces `ensure-row-visible(int)`; the keyboard menu anchor uses a new `in property <length> current-row-y` set by `sync_current`.

- [ ] **Step 1: Write failing tests:**
  - unit: `a_hung_app_reopens_its_section_once` (closed Background section, app turns hung → open; user closes → stays closed; hang clears then a new hang → opens again).
  - unit: `ensure_visible_uses_real_row_tops` (rows Section, App, App(expanded), Proc, Proc(last), Section → `row_top(rows, 4) == (32+50+50+30, 30)`).
  - ui.rs: `collapsing_a_section_hides_its_cards_and_reopens_on_a_hang` through the bridge.
  - ui.rs: `page_down_reveals_the_card_it_lands_on` with mixed rows (the current card's rect lies within the list's visible rect after the key).
- [ ] **Step 2:** Run — fail.
- [ ] **Step 3:** Implement; run `reopen_hung_sections` wherever `auto_expand` runs today (not while frozen; on unfreeze).
- [ ] **Step 4:** `cargo test --workspace` passes.
- [ ] **Step 5:** Commit `feat(app): sections collapse, reopen for a hang, and the list scrolls by real row tops`.

### Task 5: Program icons

**Files:**
- Create: `crates/app/src/icons.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/bridge.rs`, `crates/app/src/main.rs`, `crates/app/Cargo.toml` (windows features only if missing)
- Test: `crates/app/src/icons.rs` (unit), `crates/app/tests/ui.rs`

**Interfaces:**
- Produces:
  - `pub const ICON_PX: u32 = 32;` (drawn at 24 px on cards and 16 px on process rows; 32 px keeps them sharp up to 133 % scaling).
  - `pub fn extract(path: &str) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>` — Windows: `SHGetFileInfoW(SHGFI_ICON | SHGFI_LARGEICON)`, then `GetIconInfo` + `GetDIBits` to straight-alpha RGBA (icons without an alpha channel: use the mask); `DestroyIcon` always. Needs COM initialised on the calling thread (`CoInitializeEx` apartment-threaded on the loader thread).
  - `pub struct IconLoader` — owns one background thread fed by a channel; `IconLoader::spawn(extract: impl Fn(&str) -> Option<SharedPixelBuffer<Rgba8Pixel>> + Send + 'static, done: impl Fn(String, Option<SharedPixelBuffer<Rgba8Pixel>>) + Send + 'static) -> IconLoader` and `fn request(&self, path: &str)`; `done` posts to the UI thread in the app (`slint::invoke_from_event_loop`).
  - `pub struct IconCache` (UI thread): `fn get(&mut self, path: &str, loader: &IconLoader) -> Option<slint::Image>` — returns a cached image; on first sight requests it once; remembers failures (`None`) so they are never re-requested; `fn put(&mut self, path: String, px: Option<SharedPixelBuffer<Rgba8Pixel>>)`.
  - `RowData` gains `icon: image` and `has-icon: bool` (Task 6 draws them); the bridge fills them in `push_rows` from `IconCache`, and re-pushes rows when an icon arrives (only changed rows update, through `sync_model`).

- [ ] **Step 1: Write failing tests:**
  - `extracts_notepads_icon` (`C:\Windows\System32\notepad.exe` → 32×32 with at least one pixel alpha > 0; `#[cfg(windows)]`).
  - `a_missing_file_has_no_icon` (a path in a temp dir that does not exist → `None`).
  - `the_cache_requests_each_path_once` (fake loader recording requests: `get` × 3 on the same path → 1 request; after `put(path, Some(..))`, `get` returns `Some`).
  - `a_failed_icon_is_remembered_as_missing` (`put(path, None)` → `get` returns `None` and makes no new request).
  - `the_loader_extracts_off_the_calling_thread` (fake extract records `std::thread::current().id()`, differs from the test thread).
- [ ] **Step 2:** Run — fail.
- [ ] **Step 3:** Implement; wire the loader in `main.rs` (its `done` posts into the bridge's cache and calls a bridge method that re-pushes rows).
- [ ] **Step 4:** `cargo test --workspace` passes.
- [ ] **Step 5:** Commit `feat(app): program icons, extracted off the UI thread and cached by path`.

### Task 6: The card list in Slint

**Files:**
- Modify: `crates/app/ui/applist.slint`, `crates/app/ui/main.slint`, `crates/app/src/bridge.rs` (`push_rows` mapping)
- Test: `crates/app/tests/ui.rs`, un-ignore `row_heights_match_the_list` in `view.rs`

**Interfaces:**
- Consumes: everything above. `RowData` becomes `{ id, kind: int (0 section, 1 app, 2 proc), depth, name, count, sub, cpu, mem, disk, gpu, net, bars: [float], hung, expandable, expanded, actionable, exited, icon: image, has-icon, windows, last-in-card }`.
- Produces: the final look. Property names `section-height: 32px`, `app-height: 50px`, `proc-height: 30px` in `AppList`.

Look (match the mockups; values from the spec):
- Header row: Name · CPU · Memory · Disk · GPU · Network as focusable `FocusButton`s (accessible role button, label "Sort by <column>", `accessible-checked` on the sorted one), 11.5 px uppercase muted, the sorted one in `Theme.accent` with ▾ (descending) / ▴ (ascending) drawn as a Path (no glyphs Segoe UI Variable may lack). Clicking calls `sort-clicked`.
- Section heading: chevron (the existing drawn Path), name and count, 12 px semibold muted, accessible label "<name> section, expanded/collapsed", toggles on click and on Enter.
- App card: `Theme.card` fill, `Theme.card-border` 1 px, radius `Theme.radius`; when expanded only the top corners are rounded (`border-top-left-radius` / `border-top-right-radius`). Contents: chevron if expandable, 24 px icon (`has-icon` ? image : generic glyph Path; `windows` ? a four-square Windows glyph Path in `Theme.accent`), name 13.5 px + `sub` 11.5 px muted (danger text when hung), then the × button, then the five numbers right-aligned in tabular figures with a 3 px bar under each (`bars[i]` of a 70 px track, accent on `Theme.card-border`). Hover: card a step lighter and border `Theme.muted` at low alpha. Hung card background `Theme.danger-bg`.
- × End task: a 22 px square `FocusButton`, × drawn as a Path, shown while the card is hovered or the button holds focus (same rule as today's End task button: actionable app rows, `depth == 0`), red (`#c42b1c`-like danger fill from Theme; add `Theme.danger` if missing, with a light-theme value) on its own hover; accessible label "End task <name>"; calls `action("end-task", id)` and reports `row-focus` like the old button. Force close stays a Button with its label.
- Process row: inside the card — same side borders as the card, a slightly darker fill (`Theme.card` over a faint black, light theme a faint grey), a hairline above the first one, the last (`last-in-card`) rounds the bottom corners and carries the 4 px gap. 16 px icon, name 12.5 px, PID-less (PID is already in Details), numbers 12.5 px, no bars.
- The strong (600) weight on the selected metric's numbers stays.

- [ ] **Step 1: Write failing tests** (`ui.rs`):
  - `the_sorted_header_is_checked_and_clicking_another_sorts_by_it` (accessible "Sort by Memory" click → `sort-column` 1).
  - `the_end_task_icon_shows_on_hover_and_ends_that_app` (rework of the existing End task hover tests: the "End task Browser" button exists only while hovered, activating it reports `end-task` for that id; keep `end_task_button_shows_on_row_hover_and_survives_pointer_moving_onto_it` passing with the new button).
  - `cards_sections_and_process_rows_have_their_heights` (rects of a section heading, a card, a process row: 32 / 50 / 30 px tall within 1 px).
  - `a_section_heading_click_collapses_it`.
  - `an_icon_row_draws_its_image_and_one_without_draws_the_glyph` (has-icon true → an `Image` element with the row's accessible parent; false → no image) — or a pixel check in `render.rs` if the element tree is not reachable.
  - Un-ignore `row_heights_match_the_list`.
  - Every existing `ui.rs` test still passes (labels kept); update only tests whose geometry assumptions (32 px rows) the spec changes, and say which in the report.
- [ ] **Step 2:** Run — fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test --workspace` and `cargo clippy --workspace --all-targets` clean.
- [ ] **Step 5:** Commit `feat(app): the app list as Fluent cards with sortable headers and icons`.

### Task 7: Measure and document

**Files:**
- Modify: `docs/benchmarks/README.md`
- Create: `.superpowers/run-list-bench.ps1` (git-ignored) for the user to run elevated

- [ ] **Step 1:** Write `.superpowers/run-list-bench.ps1` running `negotium-bench` on the Overview, GPU renderer, smooth on, 60 s, twice, saving `docs/benchmarks/list-*.json`, refusing to run while `negotium.exe` runs (bench already does), same options as `.superpowers/run-3c-bench.ps1`.
- [ ] **Step 2:** Hand the script to the user (do not run elevated runs yourself); when results are back, add a "App list redesign" section to `docs/benchmarks/README.md` with the numbers against Plan 3c's Overview (3.13 % / 3.74 % of a core, smooth off / on) and the manual checks: sort by each header both ways; Name A–Z/Z–A; meter ↔ header in step; collapse each section; a hung app reopens its section; icons for Store apps, elevated processes (not elevated → generic glyph), the Windows glyph; light theme; keyboard: Tab, arrows across sections, Left/Right on a section, Menu key on a card.
- [ ] **Step 3:** Commit `docs(bench): app list redesign measurements and manual checks`.
