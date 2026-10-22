# Changelog

## [0.8.0](https://github.com/MyndPhreak/Negotium/compare/v0.7.0...v0.8.0) (2026-10-22)


### Features

* **app:** Ctrl+Alt+Shift+Esc picker shortcut and review fixes ([#25](https://github.com/MyndPhreak/Negotium/issues/25)) ([840f2d1](https://github.com/MyndPhreak/Negotium/commit/840f2d100237e4dfccb91fdbd5f0a9b3be49852d))
* **app:** End a window… from the tray ([#24](https://github.com/MyndPhreak/Negotium/issues/24)) ([d23f246](https://github.com/MyndPhreak/Negotium/commit/d23f246070b0e3e624f18fd746d29aac99b75a7b))
* **app:** show a Mica Alt backdrop behind the GPU renderer ([#19](https://github.com/MyndPhreak/Negotium/issues/19)) ([54babaf](https://github.com/MyndPhreak/Negotium/commit/54babaf557528e4f0c3383309c30a3429218b098))
* **app:** window picker core — pick a window's app and decide a click ([#22](https://github.com/MyndPhreak/Negotium/issues/22)) ([6346c0f](https://github.com/MyndPhreak/Negotium/commit/6346c0f3eb004fc5ef88ab05d3673e3a85fddf7c))
* **app:** window picker Win32 building blocks ([#23](https://github.com/MyndPhreak/Negotium/issues/23)) ([518cbb6](https://github.com/MyndPhreak/Negotium/commit/518cbb6729d98c8d62671d910cd83fc5642b1b3a))


### Bug Fixes

* **app:** embed a Common Controls 6 manifest and drop the picker's workarounds ([#26](https://github.com/MyndPhreak/Negotium/issues/26)) ([0fae9fd](https://github.com/MyndPhreak/Negotium/commit/0fae9fd5260171a17a158cebef4593dc2607da61))
* **app:** embed the icon in negotium.exe ([#21](https://github.com/MyndPhreak/Negotium/issues/21)) ([74cf027](https://github.com/MyndPhreak/Negotium/commit/74cf02762d9879a4cd69ea76da08f1cbe6d2be26))


### Continuous Integration

* run PR checks once and skip the release build on PRs ([#27](https://github.com/MyndPhreak/Negotium/issues/27)) ([d215c88](https://github.com/MyndPhreak/Negotium/commit/d215c88ba0f9aabf3a407625a9aba815a0ef4455))

## [0.7.0](https://github.com/MyndPhreak/Negotium/compare/v0.6.0...v0.7.0) (2026-10-20)


### Features

* **app:** persist window size, position, and maximized state across sessions ([#12](https://github.com/MyndPhreak/Negotium/issues/12)) ([420b441](https://github.com/MyndPhreak/Negotium/commit/420b441de4f30827b4b23ff1b85c3996f282a97c))


### Bug Fixes

* code-review findings across moderator, actions, collector, updater and release CI ([#13](https://github.com/MyndPhreak/Negotium/issues/13)) ([16b598e](https://github.com/MyndPhreak/Negotium/commit/16b598ee140f71c592aaf55930224f5a95e78e36))


### Build System

* **deps:** bump actions/checkout from 4 to 7 ([#16](https://github.com/MyndPhreak/Negotium/issues/16)) ([2960d92](https://github.com/MyndPhreak/Negotium/commit/2960d92868b925e04520957efe90ff2fed4476c0))
* **deps:** bump actions/download-artifact from 4 to 8 ([#17](https://github.com/MyndPhreak/Negotium/issues/17)) ([0ca740d](https://github.com/MyndPhreak/Negotium/commit/0ca740de49cab051cab7692240e3fb57d6d51453))
* **deps:** bump actions/upload-artifact from 4 to 7 ([#15](https://github.com/MyndPhreak/Negotium/issues/15)) ([5d65bc0](https://github.com/MyndPhreak/Negotium/commit/5d65bc0038a00a3421ae15948c4146f1897259da))
* **deps:** bump googleapis/release-please-action from 4 to 5 ([#18](https://github.com/MyndPhreak/Negotium/issues/18)) ([0511735](https://github.com/MyndPhreak/Negotium/commit/05117352462ab26f6cfa1af3554fd58d741e30fd))

## [0.6.0](https://github.com/MyndPhreak/Negotium/compare/v0.5.0...v0.6.0) (2026-10-20)


### Features

* **app:** add CPU affinity and priority modals with rule persistence ([#10](https://github.com/MyndPhreak/Negotium/issues/10)) ([1c787df](https://github.com/MyndPhreak/Negotium/commit/1c787dfc335151a7472ad491a5868bd3b94759d2))

## [0.5.0](https://github.com/MyndPhreak/Negotium/compare/v0.4.0...v0.5.0) (2026-10-20)


### Features

* **app:** add Aequilibrium UI controls, system tray toggle, context menu exclusion, and restraint icon ([#7](https://github.com/MyndPhreak/Negotium/issues/7)) ([886c66d](https://github.com/MyndPhreak/Negotium/commit/886c66d4f3464e65814e6a6e5f83e28d921b799c))
* **app:** add multi-line hardware telemetry to tray tooltip and 1s cadence ([#9](https://github.com/MyndPhreak/Negotium/issues/9)) ([c8db9ac](https://github.com/MyndPhreak/Negotium/commit/c8db9ac11d662213ee89eac807c678d5abf34303))


### Continuous Integration

* streamline release build and remove redundant test step ([#6](https://github.com/MyndPhreak/Negotium/issues/6)) ([d1f3698](https://github.com/MyndPhreak/Negotium/commit/d1f3698552976a7f0b74ce229141eef6e127fc64))

## [0.4.0](https://github.com/MyndPhreak/Negotium/compare/v0.3.2...v0.4.0) (2026-10-20)


### Features

* **actions:** add Windows Sandbox support to task launcher ([#4](https://github.com/MyndPhreak/Negotium/issues/4)) ([3099292](https://github.com/MyndPhreak/Negotium/commit/30992927f5842b4b7092e05d9b013c9559c725b3))
* **moderator:** implement native Moderator engine with Aequilibrium dynamic CPU restraint and persistent rules ([#3](https://github.com/MyndPhreak/Negotium/issues/3)) ([cfdb9d6](https://github.com/MyndPhreak/Negotium/commit/cfdb9d6a4ca4f47087b6f9e53bc6619a8ac89852))


### Bug Fixes

* **ui:** add 14px scrollbar gutter to Performance device list and Overview app list ([#2](https://github.com/MyndPhreak/Negotium/issues/2)) ([fd81d69](https://github.com/MyndPhreak/Negotium/commit/fd81d69b42462636c2906d8bd412f3d16ffc21f5))

## [0.3.2](https://github.com/MyndPhreak/Negotium/compare/v0.3.1...v0.3.2) (2026-10-19)


### Bug Fixes

* **ci:** configure release-please extra-files for cargo workspace manifest ([12ee295](https://github.com/MyndPhreak/Negotium/commit/12ee29534bbdf726bb74c8a38ea2a4872cc2d02b))
* **collector:** apply AMD Zen 49C range adjustment offset for CPU package temp ([689a786](https://github.com/MyndPhreak/Negotium/commit/689a78630529181772078ec01b29bbcb4d35f68f))
* **ui:** remove duplicate search clear button and replace unicode triangle with vector chevron ([37be679](https://github.com/MyndPhreak/Negotium/commit/37be67992a5b81094bd4db7a985edc309f7e9ce9))


### Continuous Integration

* configure Release Please automated versioning and WiX MSI release workflow ([eb771ee](https://github.com/MyndPhreak/Negotium/commit/eb771ee8ad0f31132c75fcb604c221c0f0ac733d))
