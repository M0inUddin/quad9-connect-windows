# Network observation in the development client

The desktop process subscribes to Windows IP interface, unicast address, and route changes for both address families, plus DNS configuration and policy registry changes. A notification invalidates the previous resolver diagnostic and triggers a fresh, read-only service status and adapter read. The native tray also reads status after the notification burst settles. These events never enable Quad9, restore DNS, or replace the journal.

The native callback advances an observation generation and sends a coalesced wakeup through a channel with capacity one. It does no adapter inspection or IPC. The observer waits for notifications and combines bursts after 250 milliseconds of quiet, with a two-second limit. It does not continuously poll adapters or run resolver probes.

Status and diagnostic reads that span a network generation change are rejected. The client coordinator captures the generation after acquiring the request gate, so a queued diagnostic carries the generation it actually observed. The webview tracks invalidation while a request is running and queues a replacement read if an event invalidates the final post-action refresh. A late diagnostic result cannot republish the old observation. Tray diagnostic events carry their generation to handle delayed delivery. The exact identity of an uncertain mutation remains available for an explicit Retry.

The native subscriptions are owned by the observer thread. Cancellation runs outside the callback, without holding a lock the callback needs. If Windows cannot confirm cancellation, the small callback context is retained rather than freeing memory that a callback might still access. See Microsoft's documentation for [IP interface notifications](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-notifyipinterfacechange), [address notifications](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-notifyunicastipaddresschange), and [cancellation](https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-cancelmibchangenotify2).

## DNS configuration notifications

A dedicated thread uses [RegNotifyChangeKeyValue](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-regnotifychangekeyvalue) and waits on kernel events. Production opens these local 64-bit HKLM paths with `KEY_NOTIFY` only. It requests no registry value reads or mutation rights.

| Path below HKLM | Coverage |
| --- | --- |
| `SYSTEM\CurrentControlSet\Services\Tcpip\Parameters` | IPv4 adapter and global DNS configuration |
| `SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters` | IPv6 adapter and global DNS configuration |
| `SYSTEM\CurrentControlSet\Services\Dnscache` | DNS client parameters, local namespace policy, and interface-specific DoH settings |
| `SOFTWARE\Policies\Microsoft\Windows NT\DNSClient` | DNS client group policy, including namespace rules |

Each one-shot registration watches values, subtree names, and security changes. If an optional path is missing, the observer subscribes to its nearest existing ancestor. After an event it closes the old key handle, reopens the current path or ancestor, and rearms before publishing the final invalidation. This covers policy creation and replacement without keeping a handle to a deleted key. Generation changes before and after rearming prevent a read begun during that gap from publishing an old result.

Shutdown signals a stop event and joins the registry thread before closing its keys and events. Startup or runtime registration failures invalidate old evidence and leave a persistent notice in the interface. A manual check can still return a fresh scoped result, but it cannot clear the notice. Restarting the interface retries registration. No DNS query names or registry values are collected by this observer.

Desktop setup waits until subscriptions are armed or their startup failure has been saved before exposing the webview. Successful registration advances the generation and queues a status reconciliation, so the tray and any reads begun during registration cannot retain earlier evidence. This closes the gap before registry notifications begin, since they cannot report earlier edits.

## Validation and limits

Windows tests exercise real notification registration, initial callbacks, repeated cancellation, and event coalescing. Private HKCU test keys cover repeated value edits, missing policy keys, deletion and recreation, shutdown, and partial startup failure. They never change developer-host DNS or policy settings. Portable client tests cover a queued diagnostic, a generation change during a check, and preservation of a known mutation receipt. Browser tests cover diagnostic invalidation and post-action refresh, plus a mocked native failure event that leaves the warning visible after a manual check.

Three ignored native tests are selected explicitly by CI in disposable Windows Server 2022 and 2025 VMs. With no IP Helper subscriptions, they check real IPv4/IPv6 DNS writes and exact restores, DoH property changes with unchanged server addresses, and a uniquely named policy marker that blocks backend writes until removed. Each DNS baseline is restored even if an observation assertion panics. The policy marker is removed; a newly created empty policy container may remain in the disposable VM. These checks do not establish physical network handoff, sleep/resume, VPN, or Windows 11 behavior.

The DoH property test uses the backend's complete settings write and subscribes only to the DNS client service tree. TCP/IP address-key writes cannot produce that observer's notification. Readback must show the changed template and identical server addresses for both families.

Registry hive restores through `RegRestoreKey` are not reported by the Windows notification API, and changes outside these roots are not covered. Focusing or reopening the interface still reads current service state, and the service checks ownership before every mutation. No cached diagnostic establishes protection across every adapter or application.

Observation runs while the desktop process is alive, including when its window is hidden in the tray. Quitting the interface stops observation; Windows retains the explicitly configured DNS settings independently. The service remains responsible for mutation authorization, durable recovery, and exact restoration. A registration failure appears in the interface and can be queried again after the webview starts, so an early failure is not lost before event listeners are installed.
