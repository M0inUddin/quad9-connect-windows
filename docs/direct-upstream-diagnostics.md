# Direct Quad9 reachability

Run diagnostics performs two independent checks concurrently. The system-resolver
result still describes one fresh TXT lookup of `proto.on.quad9.net`. Only that
lookup can produce the existing system DoH observation. Direct results never
change connection state, enable DNS management, or satisfy Enable verification.

The direct check sends HTTPS HEAD requests to the fixed Quad9 primary IPv4
`9.9.9.9` and IPv6 `2620:fe::fe` addresses on port 443. WinHTTP uses
`dns.quad9.net` for SNI, HTTP authority, and normal certificate validation.
`WINHTTP_OPTION_RESOLUTION_HOSTNAME` supplies each literal connection address,
avoiding a system-resolver dependency. An unsupported option produces a failure;
there is no hostname-resolution fallback. Global cross-session connection pooling
is disabled where the runtime supports that option. Every response must also
pass `WINHTTP_OPTION_CONNECTION_INFO` validation against the requested IP address,
family, and port. An unsupported pooling option does not bypass that check;
missing connection information or a mismatched destination fails the result.
Windows settings remain unchanged.

The requests require HTTP/2 and TLS 1.2 or 1.3. They use no proxy, cookies,
automatic authentication, request body, client certificate, or automatic
redirects. Response headers are capped at 16 KiB and no response body is read.
An HTTP response, including 405, 429, or 503,
means that the TLS-validated endpoint replied; its status remains visible. It
does not establish successful DNS resolution, filtering, or protection of other
applications. Secondary service addresses are not tested by this check.

Both address families share a 15-second direct-check deadline and run alongside
the system check rather than adding another sequential timeout. A pending async
request is canceled by closing its request handle. Its callback context remains
alive through `HANDLE_CLOSING`. A single-flight slot remains held until every
request context has closed, bounding outstanding work after a timeout. A retry
while cleanup is pending reports Busy rather than adding requests.

Results contain fixed endpoint addresses, outcome codes, HTTP status, optional
Windows error codes, a timestamp, and duration. They are returned to the requesting
client without being added to recovery journals or startup events. There is no
arbitrary host or URL parameter. The read-only inspector also exposes
`quad9-inspect upstream --json`. Its exit code is zero when at least one endpoint
returns a TLS-validated HTTP/2 response, and one when neither does. This does not
change the system probe's separate DoH success criterion.

Windows tests use a silent local TCP peer to exercise real asynchronous timeout
and context cleanup without contacting an external service. The accepted socket
explicitly uses blocking reads because Windows can inherit the listener's
nonblocking mode. Browser regressions
cover mixed family results, direct failure with system DoH, direct success
without system DoH, HTTP error visibility, invalidation, accessibility, and an
older service without the optional result. Live endpoint compatibility still
requires Windows 11 validation; disposable Server tests do not establish all
physical network behavior.

References: [Quad9 service addresses](https://docs.quad9.net/services/),
[Quad9 HTTP/2 requirement](https://docs.quad9.net/FAQs/),
[WinHTTP options](https://learn.microsoft.com/en-us/windows/win32/winhttp/option-flags),
[connected destination information](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/ns-winhttp-winhttp_connection_info),
[asynchronous cancellation and context lifetime](https://learn.microsoft.com/en-us/windows/win32/api/winhttp/nf-winhttp-winhttpclosehandle),
and [WinHTTP concurrency](https://learn.microsoft.com/en-us/windows/win32/winhttp/concurrency-in-winhttp).
