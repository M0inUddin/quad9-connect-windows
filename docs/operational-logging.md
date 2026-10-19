# Service startup events

The Windows service records a small set of operational events in `%ProgramData%\Quad9Connect\startup-events.v1.json`. The existing protected machine directory permits SYSTEM and administrators only. The ordinary operators group cannot read it. The service never sends these records anywhere.

Records contain only a Unix timestamp, a fixed event code, and an optional numeric Windows error. They accept no caller-supplied text. They contain no request IDs, account names, adapter identifiers, domains, DNS answers, or network names. The component is always the service.

| Event | Meaning |
| --- | --- |
| `controller_starting` | Protected storage opened and controller initialization is starting. |
| `controller_start_failed` | Loading or initializing the controller failed. Inspect recovery records through the administrator recovery procedure. |
| `controller_started` | Controller initialization returned. This does not claim recovery or DNS verification succeeded; the service status reports those results. |
| `pipe_host_failed` | Creating or running the pipe host failed. A numeric Windows error is included when available. |
| `service_stopped` | The host returned normally or a service stop was requested. |

The file holds at most 4,096 records and 512 KiB. Atomic replacement uses a separate staging file capped at 512 KiB, so these operational files together remain below the specification's 10 MiB allowance. The zero-byte lock is independent of the recovery lock. A failed controller initialization can still record its stage after its recovery store closes.

The logger removes records aged seven days or more, as well as future-dated records after a clock rollback. It prunes on writes and startup. While the service runs, a cleanup worker waits until the next expiry, checking the clock at least once a minute. It does not query adapters, poll service state, or depend on the React interface. Retention resumes on the next service start when Windows or the service has been stopped. Successful installer and development-service removal delete these three operational files after service registration is removed. Recovery journals and unrelated files remain intact.

Logging is best effort. A locked, unreadable, malformed, oversized, or unsupported log never changes the controller's recovery decision or prevents an otherwise valid service startup. Unknown log contents remain available for administrator inspection. Directory validation and exclusive recovery-lock failures occur before the logger is available; the Windows Service Control Manager still reports those startup failures. This log does not replace the SCM or the protected recovery journal.

The format is versioned and rejects unknown fields. No path or logging command is exposed over IPC. Any future support export must provide a local preview, redaction, and an explicit save action. Routine logs are not a support-upload mechanism.

Unit tests cover expiry without new events, clock rollback, fixed fields, record and byte bounds, corrupt formats, stale staging files, independent ownership, and recovery-file preservation. Disposable Windows service tests exercise actual startup and stop events, startup failure with corrupt request state, continued service operation with an unusable operational log, and cleanup after safe removal. Local deterministic tests do not establish the full physical Windows 11 release matrix.
