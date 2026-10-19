# Network observation in the development client

The desktop process subscribes to Windows IP interface, unicast address, and route changes for both address families. A notification invalidates the previous resolver diagnostic and triggers a fresh, read-only service status and adapter read. The native tray also reads status after the notification burst settles. These events never enable Quad9, restore DNS, or replace the journal.

The native callback advances an observation generation and sends a coalesced wakeup through a channel with capacity one. It does no adapter inspection or IPC. The observer waits for notifications and combines bursts after 250 milliseconds of quiet, with a two-second limit. It does not continuously poll adapters or run resolver probes.

Status and diagnostic reads that span a network generation change are rejected. The webview also tracks invalidation while a request is running, so a late diagnostic result cannot republish the old observation. Tray diagnostic events carry their generation to handle delayed delivery. The exact identity of an uncertain mutation remains available for an explicit Retry.

The native subscriptions are owned by the observer thread. Cancellation runs outside the callback, without holding a lock the callback needs. If Windows cannot confirm cancellation, the small callback context is retained rather than freeing memory that a callback might still access. See Microsoft's documentation for [IP interface notifications](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-notifyipinterfacechange), [address notifications](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-notifyunicastipaddresschange), and [cancellation](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-cancelmibchangenotify2).

## Validation and limits

Windows tests exercise real notification registration, initial callbacks, repeated cancellation, and event coalescing. Browser tests cover clearing a completed diagnostic, a diagnostic that finishes after a network change, and the absence of automatic DNS mutations. These checks do not establish physical network handoff, sleep/resume, VPN, or Windows 11 behavior.

DNS-only registry or policy edits are not guaranteed to produce these IP notifications. Dedicated DNS configuration observation remains release work. Focusing or reopening the interface still reads current service state, and the service checks ownership before every mutation. No cached diagnostic establishes protection across every adapter or application.

Observation runs while the desktop process is alive, including when its window is hidden in the tray. Quitting the interface stops observation; Windows retains the explicitly configured DNS settings independently. The service remains responsible for mutation authorization, durable recovery, and exact restoration. A registration failure appears in the interface and can be queried again after the webview starts, so an early failure is not lost before event listeners are installed.
