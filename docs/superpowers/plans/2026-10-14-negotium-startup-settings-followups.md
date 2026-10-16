# Plan 3b (startup and settings) — rulings and follow-ups

Carried out of the SDD ledger for `docs/superpowers/plans/2026-10-14-negotium-startup-settings.md` when the branch finished.

## User reports and decisions

- User report (2026-10-15, after 3a merge): GPU temp works; meters no longer jump; Shift+F10 does nothing (bug — investigate); window slow to resize (investigate); no way to change theme yet (3b Task 2/3 adds it)
- User report: Shift+F10 works after Tab into the list (so bug = nothing focused → no target); light mode verified
- User report (screenshot, Performance GPU page): every graph's baseline sits mid-height — line/area are vertically compressed and centred instead of spanning bottom (0) to top (max); suspect Slint Path preserving the 1000×100 viewbox aspect (contain fit) inside non-matching graph rects (affects Performance graphs, cells, meters' sparklines, device-list sparks) — investigate
- User report: resize slow on BOTH pages → Overview (≈5 ms render) slowness is outside Slint rendering — investigate softbuffer realloc/present, Win32 modal size loop, DWM/Mica, frame application during resize
- User report: SLINT_BACKEND=winit-femtovg makes resizing smooth → software renderer/present is the resize cost; renderer choice to be decided with user
- User decision (2026-10-16): renderer option 2 — GPU (femtovg) by default with a safety net: fall back to software if the window doesn't appear within a few seconds; Settings gets a 'Use GPU rendering' switch. Implementation after the investigator reports feasibility (in-process vs relaunch); switch folds into Plan 3b Settings tasks

## Rulings made during execution

- Ruling: PF1 (T3,T7) — Toggle and ChoiceButton inherit FocusButton (switch/radio roles); overlays Banner gains text + dismissable and Startup reuses it — no duplication (3a PF13) — cost: none
- Ruling: PF2 (T3) — Settings ScrollView uses perfpage's fixed 14px-gutter content-w pattern — avoids width/scrollbar loop — cost: 14px blank gutter
- Ruling: PF3 (T7) — every StartupPage element except the list gets vertical-stretch: 0, UI test at a tall window — 3a meter-jump lesson — cost: none
- Ruling: PF4 (T7) — on_toggle_startup returns early in Rust while startup-loading is set — prevents an older list landing last — cost: a toggle during a refresh is ignored (switch shows real state)
- Ruling: PF5 (T7,T8) — page switches go through b.borrow_mut().set_page(&ui, page), then load_startup() outside the borrow; T8 restore uses the same path — current 3a code — cost: none
- Ruling: PF6 (T8) — restart_as_admin thread holds ComApartment::enter() — ShellExecuteEx needs COM — cost: none
- Ruling: PF7–PF21 (minors, fold into their tasks) — T3 Theme.on-accent; T2 pure dark_from(Option<u32>) tested + shared read_hkcu_dword + import WM_DWMCOLORIZATIONCOLORCHANGED + no duplicate AtomicU32; T3 choice tests filter by RadioButton role; tests for no-per-tick build on Startup/Settings and Light/Dark not following system; T4–T6 read version resource once and share name/publisher code; T7 one startup-thread tail and one Enabled/Disabled text; drop duplicate on-top state; real-list test asserts empty errors; T9 includes the 3a README docs fix; note column fits "Set by policy"; settings tests clean up temp dirs — cost: none
- Ruling: R1 — renderer: default winit+femtovg (selector no longer forces software); watchdog thread: if no first frame within 3 s, record fallback (settings/flag file) and relaunch self with the software renderer (release the single-instance mutex first / child waits like --restarted), then exit; later launches honour the recorded choice; SLINT_BACKEND still overrides; frame stats work again under femtovg (README); "Use GPU rendering" switch folds into Plan 3b Settings (Tasks 1-3) — user chose option 2 — cost if wrong: ~450 ms slower start than software
- Ruling: R2 — accept: GPU becomes default (Task 0b) so software is the fallback path; bench keeps the 16 ms gate on realistic smooth data and logs the worst-case noise number without failing — cost if wrong: software fallback users see sluggish resize on noisy graphs. Review dispatched
- Ruling: R3 — fix all four Importants: TerminateProcess(self) instead of process::exit (plan-mandated; avoids loader-lock hang); Armed/Presented/Fired CAS state shared by watchdog and first-frame timer (race test both ways); fire only when no first frame AND no UI-thread heartbeat (chrome timer posts heartbeat; minimised/hidden window never triggers); tray gated on 'watchdog resolved' incl. TaskbarCreated path; timeout 10 s with message/README formatted from the constant; minors: #[cfg(debug_assertions)] stall binding, args_os, remove SLINT_BACKEND from own env after selection — R1.6 and no permanent false fallback — cost if wrong: a real stall waits 10 s before recovery
- Ruling: R4 — per-field tolerance (parse to Value; syntax/non-object → set aside; object → each field falls back to its default, bad field names reported, original copied to .bad) + tests; fold minors: set_aside logs real rename outcome and drops the needless remove; only parse/InvalidData errors set aside (other I/O errors → defaults, file untouched); save syncs tmp before rename + comment on single-instance tmp name; renderer tests use a TempDir-style guard — spec Review Focus 1 binding — cost if wrong: none
- Ruling: R5 — fix Important (keyboard focus scrolls the focused row into view; test) + minors: failed marker clear re-shows the fallback line; restart note from the effective renderer decision (FromEnv → "Overridden by SLINT_BACKEND"); bounded ImmersiveColorSet compare + SAFETY wording; copy theme before apply (no Ref across SetWindowPos); radio groups labelled + disabled rows describe why + description as accessible-description; ChoiceButton hover visible, disabled switch/title dimmed; stale renderer doc — keyboard users must see what they change — cost: none
- Ruling: R6 — host executables (rundll32, regsvr32, cmd, powershell, pwsh, wscript, cscript, mshta, conhost, explorer, msiexec) fall back to the value name with no publisher (RF2; no false Microsoft publisher); write guard default-deny (library starts confined; app main calls registry::allow_system_writes() once); minors: unreadable StartupApproved key → list entries + error line; >64 KB value → error line; error lines use message text (Display for ActionError); retry on ERROR_MORE_DATA in value(); exe_from_command handles '/' and ':' paths and last .exe boundary; fmt nit — safety + RF2 — cost if wrong: a legit host-launched entry shows its value name instead of the host's
- Ruling: R7 — accept (b) reading large values in full instead of error-skipping, and (f) fmt only on new files (repo not rustfmt-clean; formatting churn deferred) — better than asked / avoids unrelated churn — cost: none
- Ruling: R8 — one shared is_task_key predicate (not in NOT_TASKS, State DWORD 0–4, UserEnabledStartupOnce DWORD present) used by listing AND set_enabled; fixtures gain UserEnabledStartupOnce=0; write opens existing key only (no re-create; not-found → "no longer installed"); out-of-range state refused as unknown; entries sorted; doc line wrapped — write safety — cost if wrong: a real task lacking the marker is hidden (none on this machine)
- Ruling: R9 — keep 'Start <name> with Windows' as the switch label (brief + tests; clearer action), non-virtualized list accepted (startup lists are small; Tab must reach every row) — cost: none. Review dispatched
- Ruling: R10 — fix plan-mandated Important (read errors surface inline: startup-read-note quiet line + UI test) and minors 1-5 (catch_unwind in spawn_job, 'Updating…' while loading with rows, log only read errors / shared Log, toast inset + keyboard test, stronger PF3 test) — spec §7 binding — cost: none
- Ruling: R11 — fix the test isolation (private find_main_window_where predicate; test matches its own hwnd; production stays cross-process) + README items 6/7/10/11/13 sharpened + rationale comment on owner cast — tests must not depend on user's running app — cost: none
- Ruling: R12 — one fix wave: I1 (inject Log/path into finish; tests use temp); M1 (drop SLINT_BACKEND env from watchdog relaunch — marker + --renderer-fallback already force software); M2 README bullet; M4 toast-inset on Performance and Settings; M5 ERROR_CANCELLED → cancel text, else ActionError message; M6 machine-reading tests #[ignore = "reads this machine"] with a non-ignored doesn't-panic variant; M7 unlocked may_write allow-lists StartupApproved\{Run,Run32,StartupFolder} and SystemAppData\…\State, write_shortcut confined to temp dir; M8 copy keep_in_tray before begin_exit. Not fixed: M3 blank publisher kept (matches Task Manager); M9 collector test → follow-up — cost if wrong: none

## Final review

- Final review: dispatched (opus) over 2ffe953..98f2a2d
- Final review: With fixes — I1 tests write fabricated errors into the user's real log; minors: Settings says "Overridden by SLINT_BACKEND" after watchdog relaunch; stale README no-flip-flop bullet; missing publisher blank; toast covers Performance/Settings bottoms; all restart failures read "cancelled"; machine-dependent real-list tests; unlocked guard allows any key; settings borrow across begin_exit; collector accuracy test load-flaky (pre-existing)
- Final: fix wave (7 commits 98f2a2d..dbfe13d); user's negotium.log holds fake test lines from earlier runs — ask user before cleaning
- Final: re-review — all addressed, no new Critical/Important. Final review clean.

## Carried, live-check and deferred items

- Task 0: minor (deferred → carry to Task 0b): bench gate on p25/min frame instead of median; misplaced doc comment render.rs:133; squeezed() ignores /* */ block comments
- Task 0b: implemented f70f50e, 641d480 (bench p25 8.88 ms); carry → Plan 3b Settings tasks: GPU switch calls renderer::clear_fallback(); any hidden/tray start must disarm the watchdog; false positive on a loaded machine sticks until cleared
- Task 0b: carry → Task 2 (touches main.rs): failed relaunch spawn leaves state Fired → resolve it so the tray appears; remaining std::env::args() at main.rs:55/:236 → args_os; resolve current_exe + relaunch args at arm time; suspend/resume guard (large poll overshoot = lost time, not silence)
- Task 0b: minor (deferred): heartbeat test timing-dependent; frame-first test name overstates
- Task 1: minor (deferred): unknown fields dropped on re-save (no passthrough); overlap test doesn't exercise coalescing
- Task 1: carry → Task 2: Partial branch ignores fs::copy failure but log claims .bad exists
- Task 1: minor (deferred): no null-field test; .bad rewritten each launch until next save
- Task 2: implemented af223f1; note: collector busy_child_reports_one_core_of_cpu flaky under load (pre-existing); review dispatched
- Task 2: carry → Task 3: filter WM_SETTINGCHANGE on lParam "ImmersiveColorSet" (+test); apply_settings(&ui,&Settings) seam with get_on_top UI test; handler moved to platform::on_settings_changed; SWP_FRAMECHANGED after dark-mode attribute change
- Task 3: carry → Task 8: marker clear and settings save as one ordered background job, or joined before relaunch/exit
- Task 3: minor (deferred): coalesce frame-change redraws on bursty color messages; worker test +2 s
- Task 3: minor (deferred): reveal may re-scroll after window reactivation; "Coming in a later version" announced twice on disabled rows
- Task 4: carry → Task 5: Cmd test should use cmd.exe (or try first word as host before .exe search); cap values() buffer ~1 MB with error line; real-list test tolerant of "Skipped" lines for non-text values (assert no read errors only)
- Task 5: carry → Task 6: skip hidden/system-attribute files in Startup folders; quote targets with spaces in folder command; program_entry args → small struct; skip version read for remote/UNC targets (avoid blocking); test list_startup with a pre-existing MTA apartment
- Task 5: live check (human): Task Manager StartupApproved\StartupFolder value name matches ours (file name incl .lnk)
- Task 5: minor (deferred): GetArguments 1024-unit truncation; test gaps (.exe/.url/.LNK, TooLarge approved value, 1 MB boundary)
- Task 6: live checks (human, Task 9): hidden +h shortcut in Startup folder vs Task Manager/sign-in; packaged toggle honoured by Settings→Apps→Startup; whether Settings/TM write LastDisabledTime on disable
- Task 6: carry → Task 7 reviewer: "Set by policy" inferred from source==Packaged && !changeable vs "Needs admin" from needs_admin()
- Task 6: minor (deferred): set_enabled path split does not reject .. (guard backstops); non-task State>4 message nuance
- Task 7: carry → Task 8: JOB_FAILED wording neutral; re-list after a panicked toggle; quiet the expected test panic on stderr
- Task 8: carry → Task 9: page_from_args via args_os; comment/log exit-while-UAC-pending; pass main HWND as UAC owner (or live-check); explicit panicked flag on JobOutcome; test Saver::run panic then flush
- Follow-up: collector test busy_child_reports_one_core_of_cpu is load-flaky (compare against the child's own CPU-time delta via GetProcessTimes)
- Follow-up: GPU fallback is sticky after one slow start; consider retrying GPU once after a healthy software session
