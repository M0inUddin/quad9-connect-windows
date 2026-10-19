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

The service rejects managed DNS policies, profile-specific settings, unknown encryption properties, unsupported adapters, uncovered active resolver scopes, and IPv6 without a preferred global address and gateway. These conservative checks require compatibility testing before broader support.

## Validation

Deterministic Rust tests cover transaction boundaries, external edits, missing adapters, unavailable storage, malformed requests, stale revisions, and retries after restart. Windows GNU builds and tests run locally. CI also uses Windows MSVC and Linux.

The `Native DNS contract in disposable VMs` workflow uses fresh Windows Server 2022 and 2025 machines. It checks IPv4 and IPv6 strict DoH writes and exact restoration of automatic settings, ordered static servers, and existing DoH properties. The service portion installs a real LocalSystem service and runs clients under separate standard-user tokens. The `vm-validation` feature permits the hosted VM's virtual Ethernet adapter only in an explicitly selected development entry point.

Server results are API development evidence. They do not establish Windows 11 support. No service or DNS mutation is installed or run on the developer's host by these checks. Interrupted transaction decisions are covered by deterministic tests; full Windows crash/power-loss and filesystem durability scenarios still require dedicated testing.

## Remaining release work

1. Run the complete write, restore, service, authorization, and recovery matrix on disposable Windows 11 x64 builds and physical adapters.
2. Add network change notifications, invalidate old verification, and reconcile external changes without rewriting them.
3. Build the Tauri/React desktop interface, tray, themes, and shared state contract.
4. Add direct upstream diagnostics and bounded operational logging for startup failures.
5. Implement NSIS installation, signed distribution, service replacement, upgrade recovery, and uninstall lifecycle tests.
6. Complete accessibility, resource, sleep/wake, VPN interference, and specification acceptance measurements.
