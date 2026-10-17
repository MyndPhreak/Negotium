# Negotium — App list redesign (design)

Date: 2026-10-16. Amends spec 5.1 item 3 (the app list) of `2026-10-14-negotium-design.md`.

## Why

The Overview's app list works but looks like a generic grid: small 12 px text, a plain header row,
a text "End task" button that pops into the name column, no icons, and no way to sort other than
picking a meter. The user wants it to feel modern and match the rest of the app (Fluent cards, the
meters and top tiles), with real program icons, a regular sortable table and Task Manager's
Apps / Background processes grouping.

Mockups the user chose (git-ignored, kept for reference):
`.superpowers/brainstorm/56476-1792203480/content/table-styles.html` (option **B · Cards**) and
`expanded-cards.html` (option **Inside the card**).

## Goals

1. **Card rows.** Every app is a rounded card (about 46 px tall) on `Theme.card` with
   `Theme.card-border`, a small gap between cards:
   - a 24 px program icon, the name at about 13.5 px, and a muted second line underneath:
     "N processes" ("1 process"), or for a frozen app "Not responding · N s" in `Theme.danger-text`;
   - the five numbers right-aligned in tabular figures, each with a thin (3 px) bar under it in
     the accent colour on a faint track;
   - hover lightens the card and its border.
2. **Expanded apps open inside the card.** The card grows; its processes are compact rows (about
   30 px, 12.5 px text, a 16 px icon, name and PID in muted text) on a slightly darker strip
   separated from the card's head by a hairline. No per-process cards.
3. **End task as an icon.** A small × button at the end of the name area, shown while the card is
   hovered (or holds keyboard focus), red background on its own hover. Same action as today's End
   task. Frozen apps keep their Force close (it may become the same icon style in danger red; the
   accessible label stays "Force close <name>"). The context menu is unchanged.
4. **Sortable headers.** Small uppercase muted headers: Name · CPU · Memory · Disk · GPU ·
   Network. Clicking a header sorts by it; clicking it again flips the direction. Numbers start
   high-to-low, Name starts A–Z. The sorted header is accent-coloured with ▾ / ▴. Processes inside
   an expanded card follow the same sort.
5. **Grouped by type, on by default.** Collapsible section headings "Apps (N)", "Background
   processes (N)" and "Windows processes", in that order, using the existing `AppKind`
   (App / Background / Windows; no new classification). Sorting happens within each section.
   Empty sections (for example during a search) are hidden. A setting turns grouping off for one
   flat list.
   - **Windows processes is the one existing "Windows" card** (the user's decision): all
     session-0 / system processes stay in the single never-actionable `AppKey::Windows` row, which
     expands inside its card as today; its processes stay individually actionable as now. No
     per-process entries in that section. With grouping off, the Windows card sits in the flat list
     like any other app.
   - **A frozen app reopens a collapsed section** (my call), with the same rule as today's
     auto-expanding hung Windows group: the section opens when the hang starts, and if the user
     collapses it again it stays collapsed until that hang clears. Section collapse state is not
     saved across launches (my call); all sections start open.
6. **Real icons.** Each app and process shows its program's icon; the Windows group shows a
   Windows glyph; anything without an icon shows a neutral generic-program glyph.
7. **Slightly bigger text** throughout the list (13–13.5 px for app rows, 12.5 px for process
   rows, up from 12 px).

## Behaviour decided

- **Meters and headers stay in step** (my call, the user did not object): clicking a meter sorts by
  that metric high-to-low; clicking a metric header also selects that meter, so the "Top … users"
  tiles follow. Sorting by Name leaves the selected meter as it is.
- **Frozen apps stay pinned to the top** of their section in every sort (my call), as today.
- **Calm re-sorting stays:** the existing Ranker rules hold for every sort key and direction
  (re-sort at most every 2 s, never while hovered / keyboard-frozen, immediately on an explicit
  sort change).
- **The sort column, direction and grouping are remembered** across launches (my call) in
  settings, tolerant-loaded like the other fields.
- **Bars** (my call): CPU and GPU bars are the app's percentage of the whole machine; Memory is
  its share of total physical memory; Disk and Network bars are relative to the busiest app in the
  list this frame (there is no meaningful machine total). A bar never exceeds its track.
- Search keeps working as now (apps by name, processes by name); sections and the cards inside
  them filter with it.

## Non-goals

- No column chooser, no extra columns (PID, user, path …), no column resizing or reordering.
- No change to the Performance, Startup or Settings pages, the meters or the top tiles.
- No change to which processes belong to which app (grouping rules stay as they are).

## Anti-goals (would count as failure even if it "works")

- The list jumping around more than today, or rows moving under the pointer.
- Visibly higher CPU on the Overview: icon loading must never happen on the UI thread or block a
  frame, and must be cached (each image path decoded once).
- Losing any accessibility or keyboard behaviour that exists today (Tab into the list, arrow
  keys, Menu key / Shift+F10, screen-reader labels, focus freezing the list, the parking key
  handler).
- Light theme looking broken: every colour comes from `Theme`.

## Constraints

- Rust + Slint 1.18.1 as today; the list stays virtualised (only visible rows built). Rows now
  have different heights (section headings, cards, expanded cards); keyboard navigation's
  ensure-visible and the keyboard menu anchor currently assume a fixed `row-height` and must be
  reworked to match.
- The worker thread builds frames; icons must be produced off the UI thread (pixel buffers are
  `Send`, `slint::Image` is not) and handed over through the existing frame/bridge path or a
  small cache the bridge owns.
- Existing project rules hold: tests never touch the user's real settings, log or registry, and
  never launch the real window; the ScrollIndicator replaces the Fluent scrollbar; no
  `animation-tick()`.
- Overview CPU budget as measured after Plan 3c (about 3–4 % of a core visible, GPU renderer)
  must not grow noticeably; the builder measures with `negotium-bench` as in Plan 3c.

## Left to the builder

- How icons are extracted (`SHGetFileInfoW`, `IShellItemImageFactory`, a packaged app's logo)
  and at what size/DPI, and how the cache is bounded.
- Where the grouping switch lives in the UI (Settings page at minimum).
- How rows are modelled for variable heights (one flat model with a row kind, or nested models),
  as long as virtualisation, freezing and keyboard navigation keep working.
- Exact spacing, radii and colours within the Theme tokens, matched to the mockups.
