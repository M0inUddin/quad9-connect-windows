# Quad9 desktop development

This is the first Tauri 2, React, TypeScript, Vite, and Tailwind desktop increment. It includes connection, diagnostics, explicit adapter selection, per-user themes, and native tray controls. It communicates with the separate service through one typed Rust command. The frontend has no shell, HTTP, or filesystem plugin access.

## Run

Use Node.js 24 and the pinned Rust toolchain. Native Windows builds need the MSVC C++ tools, Windows SDK, and WebView2 prerequisites documented by [Tauri](https://v2.tauri.app/start/prerequisites/).

```powershell
cd apps/desktop
npm ci
npm run tauri dev
```

Build a standalone development executable with `npm run tauri build -- --debug --no-bundle`. Direct Cargo builds need `--features custom-protocol` to embed the built frontend. Desktop CI publishes a standalone unsigned executable. The separate installer workflow builds and validates the development installer and its service lifecycle.

The app normally runs unprivileged. Use the [development installer](../../docs/windows-installer.md) or install the service inside a disposable VM using the [service development guide](../../docs/service-development.md), then sign in again after operator enrollment. Without the service, the app shows "Service unavailable" and never offers an Enable mutation.

Closing an idle window, or pressing Ctrl+W, releases its webview and keeps the native tray running. Opening the app recreates the window with saved preferences. An in-flight operation or uncertain mutation receipt keeps its hidden document until resolved. The tray opens the app, runs diagnostics, opens settings, restores DNS, or quits the interface. Quit leaves committed DNS settings and the service running. The native host serializes pipe calls and verifies service identity through the existing IPC client. A second app instance opens the original host.

Launch `quad9-desktop.exe --start-in-tray` to create only the native tray, without a window or WebView2 process. It reads service status once and subscribes to network changes. Opening the application later creates the window. Repeating `--start-in-tray` leaves an existing interface as it is; launching without arguments opens it. Unsupported arguments exit with code 2. This command does not register automatic sign-in startup or change DNS management. The opt-in sign-in setting remains unimplemented.

## Browser preview and tests

```powershell
npm run dev
# Open http://127.0.0.1:1420/?preview=off
npm test
npm run build
npx playwright install chromium
npm run test:e2e
```

The browser preview is a development-only fixture and carries a visible label. It never changes DNS. Preview scenarios include `off`, `configured`, `recovery`, and `unavailable`. Production browser content cannot use fixture data, even with a preview URL parameter. The packaged Tauri app always uses native IPC.

Tests cover status precedence, corrupt preferences, explicit selection, enable/restore, diagnostic scope, unavailable service, recovery, theme persistence, keyboard navigation, high contrast, reduced motion, and narrow layouts. Screenshots of the running browser interface are generated under the ignored `output/playwright/` directory. Set `PLAYWRIGHT_CHANNEL=msedge` to use an existing Edge installation locally.

`node scripts/native-smoke.mjs src-tauri/target/debug/quad9-desktop.exe` starts the actual packaged Windows app with an isolated WebView2 profile and a temporary local debugging port. It checks embedded assets, real Tauri-to-Rust IPC, the unavailable-service path, disabled Enable, theme selection, and a native screenshot. It expects the service to be absent and changes no DNS settings. The helper terminates only its own app process. Its profile and generated screenshot remain under ignored validation output.

`node scripts/native-tray-startup.mjs <desktop.exe> <output-directory>` checks tray startup with the service absent. Cumulative Windows job accounting detects even a short-lived child process before opening the window. It also checks repeated tray launches, rejected unsupported arguments, a status read before any document exists, normal second-launch activation, and preference retention after close/reopen. Its fresh job terminates all owned descendants on completion or driver failure. This is a functional check, not a resource-budget or startup-speed qualification.

`node scripts/native-validation.mjs <desktop.exe> 20 <output-directory>` checks native accessibility, process-cold startup, retained-document safety, preference persistence, and close/reopen behavior. `node scripts/native-resources.mjs <desktop.exe> <output-directory>` samples the installed service and complete desktop process tree in window and tray modes. It settles each mode for 30 seconds and samples for five minutes. Run desktop tests unelevated with DNS management off and an authorized operator token. Perform administrator setup and cleanup separately; elevated WebView2 hosts ignore the driver's diagnostic environment flags.

`--desktop-only --service-unavailable` measures the desktop with the service absent and leaves complete-process budgets unassessed. Add `--counter-smoke --verify-pwsh` for a short compatibility check under both PowerShell editions. `--service-unavailable` can also assert an inaccessible installed service while retaining its process counters, after a separate administrator check confirms DNS management is off. See [measurement methods and results](../../docs/performance-accessibility.md).

## Current limits

The interface deliberately says "Quad9 configured" for matching saved settings. A separate result describes one system-resolver lookup, with its time and duration. It does not claim every application uses encrypted DNS. Successful results are cleared when a new check starts, the window regains focus, or a native network event invalidates them. Full Windows 11 compatibility validation remains release work.

An [unsigned development installer](../../docs/windows-installer.md) installs the service and handles upgrades, repair, and safe removal. Autostart preferences, native notifications, and a public signed package are not implemented. Light/dark/system themes and selected adapters are local per-user preferences. [Artwork provenance and terms](ASSETS.md) are recorded separately from the source license.
