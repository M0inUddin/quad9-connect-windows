# Version 0.1 implementation status

Version 0.1 is under development. It is not a supported public release.

## Implemented

- A fixed strict Quad9 DoH profile, typed settings, and a versioned transaction journal.
- Intent before writes, readback, initial verification, rollback, original baseline retention, and conflict-aware restore.
- Native adapter inspection, family-specific DNS reads and writes, and automatic/static assignment detection crosschecked against the fixed Windows static configuration registry values.
- Protected atomic recovery files, exclusive storage ownership, and version/size validation. The service rejects untrusted directory and file ACLs and reparse points.
- A LocalSystem SCM host and local named pipe with per-connection token authorization for administrators, SYSTEM, and the explicitly enrolled local operators group.
- Typed commands, bounded frames and I/O, server identity checks, durable caller-scoped receipts, and revision checks. Old Enable retries do not undo a later Restore.
- A cancellable system-resolver query with one outstanding query at most. A protocol observation describes that lookup, not all network traffic.
- Development service install/removal scripts. Removal restores app-owned settings first and retains the service when restoration cannot complete.
- An unsigned NSIS development installer with explicit local-user enrollment, protected versioned payloads, active-state upgrades, failed replacement rollback, interrupted-setup reconciliation, and offline administrator recovery. Uninstall restores under the exclusive store lock before deleting the service and retains recovery records. See [installer behavior and validation](windows-installer.md).
- A first Tauri 2/React desktop client with explicit adapter and family selection, enable/restore controls, scoped diagnostic results, and per-user light/dark/system themes. The native host uses typed authenticated IPC, a single app instance, close-to-tray behavior, and a tray menu with restore and quit controls.
- Native IP interface, address, route, DNS configuration, and policy notifications with coalesced status reads and diagnostic invalidation. Missing or replaced policy keys remain observed. Reads that span a change cannot republish stale observations, and monitoring failures remain visible. See [network observation](network-observation.md) for coverage and limits.
- Bounded, protected service startup events with fixed codes, seven-day retention while running, and cleanup during safe uninstall. Logging failures preserve the controller's existing recovery decision. See [operational logging](operational-logging.md).

The service rejects managed DNS policies, profile-specific settings, unknown encryption properties, unsupported adapters, uncovered active resolver scopes, and IPv6 without a preferred global address and gateway. These conservative checks require compatibility testing before broader support.

Adapter inspection excludes the complete Windows IPv6 discovery-default list `fec0:0:0:ffff::1` through `::3` only when no static or profile DNS configuration exists and no machine IPv6 route covers any of those addresses. The software loopback may report these defaults without an IPv6 DNS settings object. Explicit settings, mixed lists, matching routes, and unknown routing/configuration remain active scopes. See Microsoft's [discovery-default description](https://download.microsoft.com/download/9/4/6/946958ef-7b86-4ddc-bfdb-c7ed2af4ce51/tcpip_fund.pdf) and [route table API](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-getipforwardtable2). This classification does not change adapter or DNS settings.

## Validation

Deterministic Rust tests cover transaction boundaries, external edits, missing adapters, unavailable storage, malformed requests, stale revisions, and retries after restart. Windows GNU builds and tests run locally. CI also uses Windows MSVC and Linux.

The `Native DNS contract in disposable VMs` workflow uses fresh Windows Server 2022 and 2025 machines. It checks IPv4 and IPv6 strict DoH writes and exact restoration of automatic settings, ordered static servers, and existing DoH properties. The service portion installs a real LocalSystem service and runs clients under separate standard-user tokens. The `vm-validation` feature permits the hosted VM's virtual Ethernet adapter only in an explicitly selected development entry point.

Desktop validation includes eight state/preference tests and 31 browser interaction tests. The browser cases cover the enable/restore flow, retry identity after a lost response, diagnostics while a mutation receipt is pending, diagnostic invalidation after network changes or a failed replacement check, final-refresh races, native visibility races, monitoring failures, service loss, recovery, themes, keyboard access, high contrast, reduced motion, and layout. Automated accessibility checks cover all three screens in both themes across four connection states. A production browser check confirms preview fixtures cannot activate. [Screenshots](screenshots/README.md) identify browser fixture images separately from native captures. CI builds the native Windows host with MSVC and checks accessibility, window recreation, retained-document safety, three native startup-retention unit tests, and live resource counters under Windows PowerShell 5.1 and PowerShell 7.

The actual packaged client also passed a startup/IPC smoke check on the developer's Windows 11 Pro machine, build 10.0.26300, with the service absent. It loads embedded assets, reaches the Rust pipe client, reports service unavailability, disables Enable, changes its theme, and produces a native WebView screenshot. This changes no host DNS settings and does not establish tray lifecycle or Windows 11 service write/restore support. That smoke check used the standard-user token.

A separately authorized privileged run passed on the developer's Windows 11 Pro machine, build 26300, using the production service path and a physical Wi-Fi IPv4 adapter. It checked native strict DoH write/readback/exact restoration from automatic assignment, LocalSystem service installation with DNS still off, Enable/Restore with the required fresh system DoH verification, committed settings across service restart, and historical retry after restoration and another restart. The original baseline matched after cleanup. The temporary service, new operator enrollment, and timed recovery task were removed, with recovery records archived in a protected local directory. The [sanitized validation record](validation/windows11-26300-wifi-ipv4.json) identifies the development binary and tested scope.

This host had IPv6 disabled on Wi-Fi. IPv6, static-DNS fixtures, and fresh standard-user authorization were not exercised on that host; disposable VM tests cover those separate scenarios. Server results and this one Windows 11 development run do not complete the release support matrix. Interrupted transaction decisions are covered by deterministic tests; full Windows crash/power-loss and filesystem durability scenarios still require dedicated testing.

Release MSVC performance and accessibility validation on the same Windows 11 host passed six native axe screen/theme scans, keyboard navigation, 200% content reflow, WebView media emulation, and the native document retention, reload, and close/reopen regressions. Twenty process-cold launches with fresh profiles produced a 3.24-second p95 including CDP observation, above the three-second target. The installed LocalSystem service peaked at 9.65 MiB and used 0.0260% of one logical CPU over five settled minutes. Its safe uninstall preserved the original DNS baseline and removed the temporary service and operator group.

The native UI and desktop resource tests ran unelevated after service cleanup and observed the actual unavailable-service state. Desktop-only intervals leave combined service/desktop budget decisions unassessed. A fresh authorized physical UI run and simultaneous counters remain release work. See the [measurement report](performance-accessibility.md) and [sanitized record](validation/windows11-26300-performance-accessibility.json) for hashes, methods, resource results, and remaining gates. Windows accessibility settings were unchanged; manual Narrator and operating-system contrast-theme checks were not performed.

## Remaining release work

1. Run the complete write, restore, service, authorization, and recovery matrix on disposable Windows 11 x64 builds and physical adapters.
2. Validate IP and DNS notifications during physical handoffs, policy deployment, and sleep/resume. Reconcile external changes without rewriting them.
3. Complete native tray-menu and manual assistive-technology validation on Windows 11, then add application startup preferences and connection notifications.
4. Add direct upstream diagnostics and complete physical validation of service startup events.
5. Complete signed distribution and the remaining physical Windows 11 installer, upgrade, and recovery support matrix.
6. Meet the startup, main-window memory, and open-window idle CPU targets and complete reference-machine, accessibility, sleep/wake, VPN interference, and specification acceptance measurements. See [performance and accessibility validation](performance-accessibility.md) for measured scope and outstanding gates.
