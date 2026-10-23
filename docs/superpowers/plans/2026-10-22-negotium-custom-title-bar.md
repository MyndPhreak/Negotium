# Negotium: custom title bar (plan)

## Summary

With Settings > Backdrop = Acrylic, the client area lays `Theme.window-bg` (45% black in dark
mode, 80% of `#f3f3f3` in light) over the DWM backdrop. The caption is non-client: DWM draws it
over untinted Acrylic, so it is lighter than the rest of the window. `DWMWA_CAPTION_COLOR` only
sets a solid colour, so it can't help. This work keeps the native frame (resize borders, shadow,
rounded corners, snap) and removes only the caption: the window subclass answers `WM_NCCALCSIZE`
so the client area starts at the top of the window, and answers `WM_NCHITTEST` so the top band
still drags, resizes and opens snap layouts. Slint draws the icon, the title and the minimise,
maximise/restore and close buttons inside the client area, over the same `Theme.window-bg` tint.
It is behind a new setting, **Use the Negotium title bar** (`custom_title_bar`), off by default.

## Architectural decisions

1. **Subclass, not `no-frame`.** Slint 1.18 offers `WindowMoveArea`, which needs `no-frame: true`
   and calls winit's `drag_window`. That loses snap layouts (the shell asks `WM_NCHITTEST` for
   `HTMAXBUTTON`), double-click to maximise, right-click for the system menu and the native resize
   frame. winit 0.30 has no hit-test hook. The window is already subclassed
   (`platform::install_window_hooks`, `SetWindowSubclass`), and the subclass runs before winit's
   procedure, so it answers the two messages itself and passes everything else on.
2. **`WM_NCCALCSIZE`: only the top edge moves.** The subclass lets the default procedure compute
   the client rectangle, then puts its top back to the window's top. The left, right and bottom
   borders stay non-client, so the default procedure still sizes from them. A maximised window
   reaches past the monitor by the frame thickness, so its top is moved down by that much
   (`caption_client_top`), else the caption is cut off.
3. **`WM_NCHITTEST`: default first, then the caption band.** If the default procedure answers
   anything but `HTCLIENT` (the side and bottom borders, the corners there), that answer stands.
   In the client area a pure function, `caption_hit`, maps the point: the top frame-thickness band
   is `HTTOP`/`HTTOPLEFT`/`HTTOPRIGHT` (not when maximised), the maximise button is `HTMAXBUTTON`,
   the minimise and close buttons and everything below the caption are `HTCLIENT`, the rest of
   the caption is `HTCAPTION`. `HTCAPTION` gives drag, double-click to maximise, Aero Shake and
   right-click for the system menu from the default procedure, with no code of ours.
4. **Only the maximise button is non-client.** Windows 11 shows the snap layouts flyout only over
   an `HTMAXBUTTON` answer. Minimise and close stay `HTCLIENT`, so Slint handles their hover,
   press and accessibility like any other button. The maximise button's mouse input arrives as
   non-client messages: the subclass swallows `WM_NCLBUTTONDOWN` there (so the default procedure
   draws no legacy button), toggles on `WM_NCLBUTTONUP` by posting `WM_SYSCOMMAND`
   `SC_MAXIMIZE`/`SC_RESTORE`, and reports hover and press to Slint as
   `ShellEvent::MaxButton(state)`; `TrackMouseEvent(TME_LEAVE | TME_NONCLIENT)` ends the hover.
5. **Buttons act through `WM_SYSCOMMAND`.** Minimise, maximise/restore and close post
   `SC_MINIMIZE`, `SC_MAXIMIZE`/`SC_RESTORE` and `SC_CLOSE`, the same commands the native caption
   sends. Close then takes the existing path (winit's close request, keep-in-tray), and minimise
   keeps the native animation.
6. **One geometry, checked on both sides.** The caption is 32 logical px high and each button
   46 px wide (the Windows 11 caption). Rust holds the constants (`CAPTION_HEIGHT`,
   `CAPTION_BUTTON_WIDTH`) for the hit test; Slint holds them in an exported `Caption` global for
   the layout; a test asserts they are equal. Physical sizes are `round(logical * scale)`, with the
   scale from `GetDpiForWindow` per message, so a DPI change needs no extra code.
7. **The caption is drawn last and on top.** It is the last child of `MainWindow`, at y = 0, and
   the content gets a spacer of the same height. A modal's scrim then lies under it, and the
   buttons still work while a modal is open, as the native ones do.
8. **Not in the Tab order.** Native caption buttons take no keyboard focus; the keyboard path is
   Alt+Space (the system menu, unchanged). The Slint buttons are exposed to accessibility (role
   button, label, default action) but have no `FocusScope`.
9. **Active and inactive.** The native caption dims its text and glyphs while the window is
   inactive. The subclass reports `WM_NCACTIVATE` as `ShellEvent::Active(bool)` and the title bar
   uses `Theme.muted` for them then.
10. **Backdrop cases need nothing extra.** The title bar has no background of its own: the
    window's background shows through it. With Acrylic that is the tint, with Mica it is
    transparent (as the native Mica caption), and under the software renderer it is the solid
    `Theme.window-bg`. The DWM frame extension is unchanged (`apply_window_chrome`).
11. **The flag.** `Settings.custom_title_bar`, default false; a file that predates it reads as
    off. Off, the subclass passes both messages straight on and Slint shows no title bar: the
    parity tests compare the hooked window with an unhooked one. Toggling applies at once
    (`SWP_FRAMECHANGED`).

## Files

| File | Change |
| --- | --- |
| `crates/app/src/platform.rs` | `CAPTION_HEIGHT`, `CAPTION_BUTTON_WIDTH`, `caption_hit`, `caption_client_top`, `CaptionButton`, `MaxButton`, `set_custom_caption`, `caption_command`, `install_caption`; `ShellEvent::MaxButton`, `ShellEvent::Active`; subclass handles `WM_NCCALCSIZE`, `WM_NCHITTEST`, `WM_NCLBUTTONDOWN/UP`, `WM_NCMOUSEMOVE`, `WM_NCMOUSELEAVE`, `WM_NCACTIVATE` while the flag is on; tests |
| `crates/app/src/settings.rs` | `custom_title_bar: bool` (default false), loaded per field; tests |
| `crates/app/src/settingspage.rs` | shows the setting; `custom-title-bar-toggled` saves it and calls `platform::set_custom_caption`; `apply_settings` applies it |
| `crates/app/src/main.rs` | `platform::install_caption(&ui)`; handle `ShellEvent::MaxButton`, `ShellEvent::Active`; `MovedOrResized` also sets `maximised` |
| `crates/app/ui/titlebar.slint` | new: `Caption` global, `TitleBar` component |
| `crates/app/ui/main.slint` | caption properties and callbacks, top padding, `TitleBar` on top, passes the setting to the page; exports `Caption` |
| `crates/app/ui/settingspage.slint` | the toggle row |
| `crates/app/Cargo.toml` | `windows` feature `Win32_UI_Input_KeyboardAndMouse` is already on (TrackMouseEvent); no change expected |
| `crates/app/tests/ui.rs` | title bar tests |

## Contracts (landed as compiling stubs in the red commit)

```rust
// platform.rs
pub const CAPTION_HEIGHT: f32 = 32.0;       // logical px
pub const CAPTION_BUTTON_WIDTH: f32 = 46.0; // logical px
/// Hit-test code (HTCLIENT, HTCAPTION, HTTOP, HTTOPLEFT, HTTOPRIGHT, HTMAXBUTTON) for a point in
/// the client area, physical px from its top-left. `frame` is the resize border in physical px.
pub fn caption_hit(x: i32, y: i32, width: i32, scale: f32, frame: i32, maximised: bool) -> u32;
/// The client top after WM_NCCALCSIZE: the window top, moved down by `frame` when maximised.
pub fn caption_client_top(window_top: i32, maximised: bool, frame: i32) -> i32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaxButton { Idle, Hover, Pressed }   // index 0, 1, 2 in Slint
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionButton { Minimise, MaximiseRestore, Close }
pub fn set_custom_caption(ui: &MainWindow, on: bool);
pub fn caption_command(ui: &MainWindow, button: CaptionButton);
pub fn install_caption(ui: &MainWindow); // wires the three Slint callbacks
// ShellEvent gains: MaxButton(MaxButton), Active(bool)

// settings.rs
pub custom_title_bar: bool, // default false
```

```slint
// titlebar.slint
export global Caption { out property <length> height: 32px; out property <length> button-width: 46px; }
// main.slint (MainWindow)
in property <bool> custom-caption;      // the setting
in property <bool> maximised;
in property <bool> active: true;
in property <int> max-button;           // MaxButton index
in-out property <bool> settings-custom-title-bar;
callback caption-minimise(); callback caption-maximise(); callback caption-close();
callback custom-title-bar-toggled(bool);
```

No persisted shape changes beyond one new boolean with a default: an older file loads as off, and
an older version reading a newer file ignores the unknown field (`serde(default)`, per-field load).

## Test plan

`platform.rs` (unit)

- `caption_hit`, restored, scale 1, width 1000, frame 8:
  - (500, 100) below the caption → `HTCLIENT`
  - (500, 20) in the caption → `HTCAPTION`
  - (10, 20) over the icon → `HTCAPTION`
  - (1000-46-23, 20) over maximise → `HTMAXBUTTON`
  - (1000-23, 20) over close and (1000-92-23, 20) over minimise → `HTCLIENT`
  - (500, 3) top band → `HTTOP`; (3, 3) → `HTTOPLEFT`; (997, 3) → `HTTOPRIGHT`
  - (1000-46-23, 3) the top band over the maximise button → `HTTOP` (resize wins)
  - (500, 31) the last caption row → `HTCAPTION`; (500, 32) → `HTCLIENT`
- `caption_hit`, maximised: (500, 3) → `HTCAPTION`, (1000-46-23, 3) → `HTMAXBUTTON`
- `caption_hit`, scale 1.5 (DPI 144): (500, 47) → `HTCAPTION`, (500, 48) → `HTCLIENT`,
  (1000-69-34, 20) → `HTMAXBUTTON`, (1000-69, 20) → `HTCLIENT` (close)
- `caption_client_top`: restored → the window top; maximised → top + frame
- parity, flag off: on a hooked `WS_OVERLAPPEDWINDOW` window and an unhooked twin of the same
  size, `WM_NCCALCSIZE` gives the same client rectangle and `WM_NCHITTEST` at a caption point
  gives the same answer (`HTCAPTION` from the native caption)
- flag on: `WM_NCCALCSIZE` keeps the window's top and the default left, right and bottom
- flag on: `WM_NCHITTEST` in the caption → `HTCAPTION`; over the maximise button → `HTMAXBUTTON`;
  on the left border → the default answer (`HTLEFT`)
- flag on: `WM_NCLBUTTONDOWN` with `HTMAXBUTTON` returns 0 and reports `MaxButton(Pressed)`;
  `WM_NCLBUTTONUP` then posts `WM_SYSCOMMAND` `SC_MAXIMIZE` (peeked, not run) and reports
  `MaxButton(Hover)`; `WM_NCLBUTTONUP` with no press first posts nothing
- flag on: `WM_NCMOUSEMOVE` with `HTMAXBUTTON` reports `Hover`, with `HTCAPTION` reports `Idle`;
  `WM_NCMOUSELEAVE` reports `Idle`
- `WM_NCACTIVATE` reports `Active(true)` / `Active(false)` (flag on); nothing with the flag off

`settings.rs` (unit)

- `custom_title_bar` defaults off, and a file without it loads as off
- it round-trips through save and load
- a bad value takes the default and keeps the other fields

`tests/ui.rs`

- the title bar is not shown by default (no "Close" button found)
- with `custom-caption` on: "Minimise", "Maximise" and "Close" buttons exist and their default
  actions call `caption-minimise`, `caption-maximise` and `caption-close`
- with `maximised` on, the middle button is labelled "Restore"
- with `custom-caption` on, the close button is at the top-right corner, `CAPTION_HEIGHT` high and
  `CAPTION_BUTTON_WIDTH` wide
- `Caption.height` and `Caption.button-width` equal `platform::CAPTION_HEIGHT` and
  `CAPTION_BUTTON_WIDTH`
- the Settings toggle "Use the Negotium title bar" calls `custom-title-bar-toggled`

### Exemptions from test-first (manual checks replace them)

Visual output and the live behaviour of DWM and the shell can't be asserted in a unit test. Each is
checked by hand on `try\negotium.exe` (`scripts/try.ps1`), setting on:

- M1 Acrylic, dark: the caption has the same tint as the window, with no lighter band.
- M2 Acrylic, light: as M1.
- M3 Mica, dark and light: the caption looks as the native Mica caption did.
- M4 GPU rendering off (solid): the caption is the solid window colour, the frame and shadow stay.
- M5 Drag by the caption; double-click maximises and restores; Aero Shake.
- M6 Hover over maximise shows the snap layouts flyout; picking a layout snaps the window.
- M7 Resize from every edge and corner, the top edge and top corners included.
- M8 Minimise, maximise/restore and close buttons: hover and press colours, close's red; close
  with keep-in-tray on hides to the tray.
- M9 Right-click on the caption and Alt+Space open the system menu; its Move, Size, Maximise and
  Close work.
- M10 Maximised: nothing is cut off at the top, the restore glyph shows, there is no top resize
  band.
- M11 Move the window to a monitor at another scale (or change the scale): the caption and the hit
  areas still match.
- M12 Inactive window: title and glyphs dim; active again: they return.
- M13 Narrator reads the three buttons by name and can press them.
- M14 Setting off (default): the native caption is back, the window as before.

## Deviations

- **No setting: the title bar is always on.** After the on-screen checks passed (Mica, Acrylic,
  GPU rendering off, drag, snap layouts, resize, the buttons, the system menu, maximised), the
  user asked to remove the switch. That drops the off-by-default flag and its flag-off parity
  test (decision 11). A settings file that still has `custom_title_bar` loads in full: unknown
  fields are ignored (`a_file_with_the_old_title_bar_switch_still_loads_in_full`).
- `min-height` does not grow by the caption height while the setting is on. With the caption in
  the client area, the window's smallest outer height is 32 px less than with the native caption.
  The testing backend does not enforce or report `min-height`, so the growth could not be tested
  first, and the difference is small. Replaced by a test of where the title bar lies.
- The frame tests use a window class whose procedure is `DefWindowProcW`: a sent WM_NCCALCSIZE
  to the system STATIC class gave no non-client area, so a test against it proved nothing.
- The `screen_readers_hear_each_setting_once_with_its_description` test counts 19 Settings rows
  now (one new row).
- `Cargo.lock` is out of date on `main` (0.7.0 after the 0.8.0 release); cargo rewrites it on each
  build. That change is not part of this work.

## Out of scope

- Turning the setting on by default (a later change, after use).
- Windows 10 (the app targets Windows 11; its top-border quirks with a removed caption are not
  handled).
- An auto-hide taskbar while maximised (the classic 1 px gap so it can be revealed).
- Clicking the icon for the system menu (`HTSYSMENU`); the icon is part of the drag area.
- High-contrast themes: the title bar uses `Theme` colours, not system colours. Follow-up if it
  matters.
- Right-to-left layout, and anything else in the caption (tabs, search).
- The picker's and tray's windows.

## Definition of done

- [ ] `Settings.custom_title_bar` is false by default and for a file without it (settings tests)
- [ ] Off, the subclass gives the same `WM_NCCALCSIZE` and `WM_NCHITTEST` answers as an unhooked
      window, and Slint shows no title bar (parity tests; M14)
- [ ] On, the client area starts at the window's top, and at top + frame when maximised
      (`caption_client_top`, NCCALCSIZE test; M10)
- [ ] On, every hit zone answers as the table says, at scale 1 and 1.5 (`caption_hit` tests; M11)
- [ ] The maximise button answers `HTMAXBUTTON`, toggles through `WM_SYSCOMMAND` and reports
      hover and press (NC button tests; M6, M8)
- [ ] Minimise, maximise and close are accessible buttons that call their callbacks (ui tests; M13)
- [ ] Rust and Slint caption geometry are equal (ui test)
- [ ] The caption matches the tint in Acrylic dark and light (M1, M2), and looks right with Mica
      and solid (M3, M4)
- [ ] Drag, double-click, snap layouts, resize, minimise, maximise/restore, close and the system
      menu work (M5–M9)
- [ ] `cargo test --workspace` passes and `cargo clippy --workspace --all-targets -- -D warnings`
      is clean after the last edit
- [ ] A PR against `main` is ready (pushed only after the user agrees)

## Acceptance criteria

1. A user who has not turned the setting on sees no change in the window. (DoD 1, 2)
2. With the setting on and Acrylic chosen, the top of the window has the same tint as the rest.
   (DoD 8)
3. With the setting on, the window moves, snaps, resizes, maximises, restores, minimises and
   closes as a window with a native caption does, by mouse and from the system menu. (DoD 3–5, 9)
4. The caption buttons can be found and pressed with a screen reader. (DoD 6)
5. The caption keeps its size and its hit areas at every display scale. (DoD 4, 7)

## Phases

One phase, two commits: `test(app): add failing tests for the custom title bar` (stubs plus
tests, failing on assertions), then `feat(app): custom title bar behind a setting`.
