# Quad9 desktop development

This is the first Tauri 2, React, TypeScript, Vite, and Tailwind desktop increment. It includes connection, diagnostics, explicit adapter selection, per-user themes, and native tray controls. It communicates with the separate service through one typed Rust command. The frontend has no shell, HTTP, or filesystem plugin access.

## Run

Use Node.js 24 and the pinned Rust toolchain. Native Windows builds need the MSVC C++ tools, Windows SDK, and WebView2 prerequisites documented by [Tauri](https://v2.tauri.app/start/prerequisites/).

```powershell
cd apps/desktop
npm ci
npm run tauri dev
```

Build a standalone development executable with `npm run tauri build -- --debug --no-bundle`. Direct Cargo builds need `--features custom-protocol` to embed the built frontend. CI uses that feature and publishes an unsigned development executable as an artifact, without a distribution installer or service installation.

The app stays unprivileged. Install the service separately inside a disposable VM using the [service development guide](../../docs/service-development.md), then sign in again after operator enrollment. Without the service, the app shows "Service unavailable" and never offers an Enable mutation.

Closing the window hides it to the tray. The tray opens the app, runs diagnostics, opens settings, restores DNS, or quits the interface. Quit leaves committed DNS settings and the service running. The native host serializes pipe calls and verifies service identity through the existing IPC client. A second app instance opens the original window.

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

## Current limits

The interface deliberately says "Quad9 configured" for matching saved settings. A separate result describes one system-resolver lookup, with its time and duration. It does not claim every application uses encrypted DNS. Successful results are cleared when the window regains focus; native network-event invalidation and full Windows 11 validation remain release work.

There is no distribution installer yet. Autostart preferences, native notifications, and a public signed package are not implemented. Light/dark/system themes and selected adapters are local per-user preferences. [Artwork provenance and terms](ASSETS.md) are recorded separately from the source license.
