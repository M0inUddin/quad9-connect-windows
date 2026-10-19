# Version 0.1 implementation status

The first development increment establishes a Rust workspace, a platform-independent DNS transaction core, and a read-only Windows inspection executable. This is a foundation for the service and desktop application, not a completed version 0.1 release.

## Implemented in this increment

- A fixed Quad9 profile for IPv4 and IPv6 with DoH enabled and plaintext fallback absent.
- Validated adapter identifiers, address-family settings, encryption properties, and a versioned recovery journal.
- A transaction engine with intent recorded before writes, readback, initial verification, rollback, exact baseline retention, and conflict-aware restore.
- Recovery rules for failed writes, persistence failures, absent adapters, service restarts, and interrupted restoration, exercised with a simulated backend.
- Read-only native adapter enumeration and raw per-interface DNS readback, including server encryption properties when Windows returns them.
- A fresh system-resolver protocol probe with a 15-second CLI deadline and explicit evidence scope.
- Windows and Linux CI for builds, formatting, linting, and deterministic tests.

The transaction core accepts a backend and journal-store interface. No production native write backend or file-based recovery store is connected to it. The executable cannot change DNS.

## Required next

1. Prove family-specific configuration reads, automatic versus static assignment detection, strict DoH writes, and exact restore on disposable Windows 11 VMs. Raw `GetInterfaceDnsSettings` output is not yet treated as a restorable snapshot. Preserve unsupported properties instead of guessing.
2. Implement protected atomic journal storage, exclusive mutation ownership, and a native write backend using the validated behavior.
3. Implement the Windows service and authorized local pipe, including server identity validation and request retry handling.
4. Integrate cancellable system-resolver diagnostics, direct upstream reachability, and event-driven invalidation. The current synchronous probe worker is suitable for one CLI invocation; a persistent service needs cancellation and bounded outstanding work.
5. Implement the Tauri, React, and TypeScript interface, themes, tray, and shared state contract.
6. Integrate the NSIS installer, recovery command, upgrades, and uninstall restoration.
7. Complete the specification's Windows 11 compatibility, accessibility, and resource measurements before declaring 0.1 usable.

## Validation limits

Simulated transaction tests validate ordering and recovery decisions. They do not demonstrate Windows API restoration or disk durability. Read-only inspection does not prove eligibility for management, network-wide encryption, policy compatibility, or a safe automatic-mode restore.

The CLI reports physical adapter candidates only. Additional capability and policy checks are required before management. Its protocol result describes one fresh lookup, and it does not claim every application or adapter uses the same resolver.
