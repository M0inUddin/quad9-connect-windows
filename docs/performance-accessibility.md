# Windows performance and accessibility validation

The resource limits in the [project specification](project-specification.md#resource-targets) are release gates. Passing CI alone does not establish those limits or complete the Windows support matrix.

## Reproduce the measurements

Use a release MSVC build. Record its SHA-256, service SHA-256, Windows build, WebView2 version, CPU, RAM, storage, and connection state. Keep DNS management off. Verify the original DNS settings after installation and cleanup. Close other Quad9 interfaces before running a native test.

Run the desktop commands in a normal, unelevated terminal. Use administrator rights separately for service installation, verification, and cleanup. Elevated WebView2 hosts ignore diagnostic environment flags, including this driver's debugging-port setting. See [Microsoft's WebView2 guidance](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/webview-features-flags).

From `apps/desktop`, run with an installed service and a fresh authorized operator token:

```powershell
node scripts/native-validation.mjs <release-desktop.exe> 20 <output-directory> --require-service
node scripts/native-resources.mjs <release-desktop.exe> <resource-output-directory>
```

This starts a new host and a fresh WebView profile for each run. The reported startup time ends when the real Rust IPC response has settled into the interface. It includes loopback CDP attachment and observation overhead. The script leaves normal OS caches and other applications running, so these are process-cold starts rather than cold boots. The 95th percentile uses the nearest-rank method across 20 measurements. CI uses one debug-build run for functional validation and leaves the startup budget unassessed.

The last run checks the real native page with axe, keyboard navigation, forced colors, reduced motion, and 200% content zoom. It reads native window visibility rather than assuming that WebView2's document visibility follows the host. It closes and recreates the window through the native close handler, verifies the original host survives, and checks that a second launch reopens it with the saved theme. A separate retained-document check verifies that closing a window marked busy keeps its runtime state. These tests never send Enable or Restore commands.

Measure idle resources separately, with CDP, axe, and screen readers stopped. Supply the desktop process ID for window or tray measurements:

```powershell
./scripts/validation/Measure-Quad9Resources.ps1 -Mode service -OutputPath artifacts/service-resources.json
./scripts/validation/Measure-Quad9Resources.ps1 -Mode window -HostProcessId <pid> -OutputPath artifacts/window-resources.json
# Close the idle window. Keep the native tray host running.
./scripts/validation/Measure-Quad9Resources.ps1 -Mode tray -HostProcessId <pid> -OutputPath artifacts/tray-resources.json
```

The resource driver uses an isolated profile and disconnects CDP before each counter interval. A debugging listener stays enabled so the driver can exercise the native close handler between modes. It verifies that the tray interval has no remaining WebView2 descendants. It sends no DNS mutation and leaves Windows settings unchanged.

Each full measurement settles for 30 seconds and samples for at least five minutes. Attribution includes the installed service, the supplied host, and all recursive descendants, including WebView2. Working sets include shared resident pages in every process that maps them, as required by the specification's complete process-set target. CPU is the change in accumulated kernel and user time divided by elapsed wall time, expressed as a percentage of one logical CPU. It is not divided by the machine's core count. A process exit leaves the CPU budget unassessed because its final counter may have been missed.

`-DesktopOnly` is available when a service cannot be installed. The report marks the process set incomplete and leaves complete-process budget results unassessed. Short runs and intervals with less than 30 seconds of settling also leave those results unassessed. The Node driver accepts `--desktop-only` and `--service-unavailable` for an explicitly unavailable-service test. The latter asserts the observed state without excluding service counters. If a service is installed, verify independently that DNS management is off before using that mode. Operator enrollment requires a new Windows sign-in before an existing standard-user token gains access.

CI uses `--desktop-only --counter-smoke --verify-pwsh --service-unavailable` to check real counters and window closure under both PowerShell editions. Raw reports and fresh browser profiles belong under ignored `artifacts/` or `output/playwright/` directories. Do not publish machine adapter identifiers, account names, local paths, or recovery records.

## Accessibility coverage

The browser suite checks all three screens in light and dark themes across off, configured, recovery, and unavailable states against WCAG 2.2 A/AA rules supported by axe. It also checks adapter group names, keyboard selection and enable/restore flows using labeled fixtures, diagnostic live announcements, 200% content zoom without clipped controls, and navigation and theme selection during a delayed diagnostic.

Native validation uses embedded production assets and real Rust IPC. Windows UI Automation and Narrator checks must be recorded separately from browser fixtures. Content zoom and forced-color emulation do not by themselves prove Windows display scaling, per-monitor DPI transitions, or every operating-system contrast theme.

Narrator speech and operating-system contrast-theme validation remain manual release checks. Starting a screen reader or inspecting its accessibility tree does not establish that these checks passed.

## Findings addressed

- Closing an idle window previously retained the complete WebView2 process set. The host now destroys that window and keeps its native tray and network observer. Opening the app recreates the frontend. Busy windows and uncertain mutation receipts remain alive until resolved.
- The dark primary button used white text on pink at a measured 2.88:1 contrast. Its foreground now meets the normal-text contrast requirement.
- The diagnostics shortcut was 21 by 21 CSS pixels. Its target is now at least 24 by 24.
- Adapter address-family controls now belong to a named fieldset, and the polite live region announces diagnostic results.
- Layout reflows under 200% content zoom. Tray receipt errors are consumed when an existing window reopens, so they cannot reappear after a later recreation.
- Starting another diagnostic clears its earlier result. A failed replacement cannot announce the earlier success through the live region.
- Tray diagnostics queue during startup and busy operations. Native listeners register before the startup handshake, and a pending diagnostic reserves its document before the initial service read. Its pending flag is independent of screen navigation. Repeated requests coalesce into a follow-up check after the current action and its final refresh.
- Native hide, destruction, and reopening share one transition lock outside the event thread. Busy retention records synchronously and competes atomically with destruction for the idle state. Reopening waits until Tauri removes the destroyed window. The host rejects commands from a closed document, ignores older retention sequence numbers, and cancels visibility workers superseded by a later request. Native validation posts retention and Close together, checks stale retention, and repeats both immediate close/reopen and concurrent release/reopen six times.
- Each mounted document establishes a native session for its retention sequence. A reload resets that sequence, drops the old document's obsolete retention, and rejects later retention commands from the replaced document. Native regressions prime the old sequence, reload, reserve the new document, verify that the old document cannot release it, and confirm that a reloaded idle document can still close completely.
- Resource samples use explicit objects that aggregate correctly under Windows PowerShell 5.1. The sampler checks root process roles and identities, measures a full five-minute interval, and keeps incomplete or unsettled results unassessed.

## Host results

The [sanitized Windows 11 record](validation/windows11-26300-performance-accessibility.json) contains measured values, tested SHA-256 hashes, and cleanup results. The release MSVC desktop came from installer workflow run [39260456324](https://github.com/M0inUddin/quad9-connect-windows/actions/runs/39260456324), with runtime source `2959b87`. This host has Windows 11 Pro build 26300, an i7-1370P with 20 logical processors, 32 GiB installed RAM, and NVMe storage. WebView2 was 155.0.4283.62; the native page reported device pixel ratio 2. This is larger than the specification's reference machine.

| Measurement | Observed | Assessment |
| --- | --- | --- |
| Service peak working set | 9.65 MiB | Within 40 MiB service-only target |
| Service idle CPU | 0.0260% of one logical CPU | Below 0.5% over five settled minutes |
| Window desktop-only peak | 394.00 MiB | Service excluded; combined budget unassessed |
| Window desktop-only CPU | 1.1741% of one logical CPU | Separate five-minute interval |
| Tray desktop-only peak | 27.24 MiB | Service excluded; combined budget unassessed |
| Tray desktop-only CPU | 0.0156% of one logical CPU | Separate five-minute interval |
| Tray WebView2 descendants | 0 | Idle close released the webview |
| Startup p95, 20 fresh-profile launches | 3.24 seconds | Above the three-second target; CDP observation included |

The temporary LocalSystem service had no active journal or recovery block. Administrator status checks confirmed DNS management was off before its service-only measurement. Safe uninstall preserved the exact original DNS baseline and removed the temporary service and operator group. Desktop tests subsequently ran under the existing standard-user token with the service absent. Their real unavailable-service state is visible in the [native screenshots](screenshots/README.md). Window and tray counters include the host and its recursive WebView2 descendants. They retain null complete-process budget decisions because the service was measured in a separate interval.

Native functional checks passed, including all six axe screen/theme scans, keyboard navigation, 200% content zoom, WebView forced-color/reduced-motion emulation, document reload retention, and six cycles each of immediate close/reopen and concurrent release/reopen. These results do not establish manual screen-reader speech or operating-system contrast-theme behavior. Windows accessibility settings were unchanged.

The original measured artifact reports `source_dirty: true`. Tauri normalized the equivalent `tauri-build` dependency declaration during packaging. That sole tracked mutation was reproduced locally, and its canonical form was committed in `c222860`. The CI checkout tree matches the tested source commit; the record retains the artifact's original dirty flag. Installer run [39267803917](https://github.com/M0inUddin/quad9-connect-windows/actions/runs/39267803917) subsequently passed and produced an installer with `source_dirty: false`. Runtime app and service source files did not change with that normalization. The physical measurements identify the original binary, rather than claiming the later binary was measured again.

Startup, main-window memory, and open-window idle CPU remain release gates. The desktop window alone exceeds the 250 MiB complete-process memory target and 0.5% CPU target on this host. Do not add separately measured service and desktop values and present the sum as a simultaneous measurement. Further work includes the four-logical-CPU/8-GiB reference machine, simultaneous counters with an installed service and fresh authorized standard-user token, native tray-menu use, manual Narrator and OS contrast themes, per-monitor DPI transitions, and the remaining supported Windows 11, sleep/resume, VPN, installer, and recovery matrix. This PR does not qualify a supported release.
