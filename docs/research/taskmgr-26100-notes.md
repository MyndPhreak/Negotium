# Taskmgr.exe 10.0.26100.9549 — reverse-engineering notes

Source: `C:\Windows\System32\Taskmgr.exe`, SHA-256 `8215acab6fb1c747279954fdf55c05e915381fc32f8b9e996c54eecf189f1227`.
Tools: REA 6.4.0 with Ghidra 12.1.4 (headless, Windows P0 provider), plus direct PE/RTTI/string parsing.
Addresses are Ghidra default-space addresses (image base `0x140000000`). Pseudocode is decompiler output, not source.

## Architecture

- Manifest identity `Microsoft.Windows.Diagnosis.AdvancedTaskManager`, `autoElevate=true`, `asInvoker`.
- UI: C++/WinRT `Windows.UI.Xaml` pages in namespace `TaskManagerApp` (HomePage, ProcessesPage, PerformancePage, AppHistoryPage, StartupAppsPage, UsersPage, DetailsPage, ServicesPage, SettingsPage, AgentsPage). Compiled XAML lives in `%SystemRoot%\SystemResources\Windows.UI.TaskManager\Windows.UI.TaskManager.pri`.
- Legacy DirectUI (`DUI70.dll`, 378 imports) grid hosted via `DUI::View::TaskManager::DataGridNativeHWNDHost`.
- Data engine: `Wdc*Monitor` classes; source paths under `base\diagnosis\pdui\atm\main\` (e.g. `cpu.cpp`, `process.cpp`, `memory.cpp`, `rumdatasrcs.cpp`, `apiwrappers.cpp`, `processcommon.cpp`, `runtimesettings.cpp`).

## Data sources (confirmed in pseudocode)

| Data | Mechanism | Where |
|---|---|---|
| CPU (system/per-core) | PCW query on counter set `"Processor Information"` | `WdcCpuMonitor::InitializePCWQuery` (`FUN_1400e1090`); `WdcProcessMonitor::InitializePCWQuery` (`FUN_14004b5d4`) |
| Disk activity | PCW query on `"FileSystem Disk Activity"` | `CRUMPCHelper::PCHelperInitialize` (`FUN_14000dfc0`) |
| Hyper-V memory | PCW query on `"Hyper-V Dynamic Memory Integration Service"` | `WdcMemoryMonitor::InitializePCWQuery` (`FUN_14019c270`) |
| GPU | PDH `\GPU Engine(*)\Utilization Percentage`, `\GPU Engine(*)\Running Time`, `\GPU Process Memory(*)\{Dedicated,Shared,Local,Non Local} Usage`, `\GPU Process Memory(*)\Total Committed`; DXCore + `D3DKMTQueryAdapterInfo` | strings + imports |
| Per-process network | `srumapi.dll` loaded dynamically: `SruRegisterRealTimeStats`, `SruQueryStats`, `SruFreeRecordSet`, `SruUnregisterRealTimeStats` | `SruApiWrapper::Initialize` (`FUN_1400bb338`), singleton at `DAT_140542a30` (`FUN_14004bc80`) |
| Processes | `NtQuerySystemInformation`, `NtQueryInformationProcess`, `GetProcessTimes`, `K32GetProcessMemoryInfo` | imports |
| Network adapters | `GetIfEntry2`, `GetAdaptersAddresses`, `NsiGetAllParameters` | imports |
| Services | `EnumServicesStatusExW`, `SubscribeServiceChangeNotifications` | imports |

### SRUM real-time stats call pattern (`CRUMAPIHelper::SrumThread`, `FUN_14004b850`)

Runs on a dedicated thread with COM initialized and a message pump:

1. Get wrapper singleton; require state `1` (initialized).
2. `GetSystemTime(&st)`.
3. Call `SruRegisterRealTimeStats(0, &st, mode, context)` where `mode` is `1` or `2` (from a flag) and `context` is the helper object.
4. Loop on `MsgWaitForMultipleObjectsEx(1, &stopEvent, INFINITE, QS_ALLINPUT, 0)`, pumping messages until the stop event fires.
5. A 5 s / 1 s `CreateTimerQueueTimer` watchdog surrounds the registration; `CoEnableCallCancellation` is used (registration is a cross-process COM/RPC call).

The callback shape and record format are **not yet recovered** — needs a spike before relying on it (spec §11).

## Actions

- End task: `TerminateProcess`.
- Efficiency mode: `SetProcessInformation` (`DataMonitors::TaskManager::EcoMode`).
- Dumps: `MiniDumpWriteDump`; live kernel dump: `NtSystemDebugControl`.
- Wait chain: `GetThreadWaitChain`. Affinity: `SetProcessAffinityMask`.

## Navigation and the Agents page

- Tab tags are static strings: Home, Processes, Performance, App History, Startup Apps, Users, Details, Services, Agents (page index 9), plus one unresolved (index 8) (`FUN_1400ff290`).
- `NavigationControl` constructor (`FUN_1400c0628`) iterates menu items in reverse and calls `IVector::RemoveAt` (vtable slot `0x60`, `FUN_140265014`) for tags `"Home"` and `"Agents"` — unconditionally in this build.
- `AgentsPage.xbf` is 1,288 bytes: title bar only.
- `AgenticAppContainer` capability: `FUN_140185098` calls `DeriveCapabilitySidsFromName(L"AgenticAppContainer", …)` and caches the SID (`DAT_14054a488`); used by the Isolation column's token check (`GetIsolationForProcess` telemetry).

## Process grouping hints

- `SubMonitors::TaskManager::{CopilotWorkUnitSubMonitor, EdgeChromiumWorkUnitSubMonitor, WebView2WorkUnitSubMonitor}` fed by a "WorkUnit" performance counter set (`App Owner Process ID`, `Host Process ID`) that apps publish to describe their process structure.
