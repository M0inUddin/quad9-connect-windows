//! Fixed-destination HTTPS reachability, independent of the system resolver.
use crate::NativeError;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct UpstreamReport {
    pub checked_at_unix_seconds: u64,
    pub elapsed_ms: u64,
    pub endpoints: Vec<EndpointReport>,
    pub scope: &'static str,
}

#[derive(Debug, Serialize)]
pub struct EndpointReport {
    pub address: &'static str,
    pub outcome: Outcome,
    pub http_status: Option<u16>,
    pub windows_error: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    HttpsResponse,
    Failed,
    TimedOut,
    Busy,
}

pub fn probe_upstream() -> Result<UpstreamReport, NativeError> {
    probe_until(std::time::Instant::now() + crate::PROBE_TIMEOUT)
}

pub(crate) fn probe_until(deadline: std::time::Instant) -> Result<UpstreamReport, NativeError> {
    #[cfg(not(windows))]
    {
        let _ = deadline;
        Err(NativeError::UnsupportedPlatform)
    }
    #[cfg(windows)]
    {
        Ok(http::probe(deadline))
    }
}

#[cfg(windows)]
mod http {
    use super::*;
    use std::{
        collections::BTreeMap,
        ffi::c_void,
        mem, ptr,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc, Arc, Mutex, OnceLock,
        },
        time::{Instant, SystemTime, UNIX_EPOCH},
    };
    use windows_sys::Win32::{Foundation::GetLastError, Networking::WinHttp::*};

    // A timed-out callback can outlive the caller. Keep this slot until every
    // HANDLE_CLOSING notification, so repeated checks cannot accumulate work.
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    // WINHTTP_CALLBACK_FLAG_HANDLES is a C macro omitted by windows-sys.
    const CALLBACK_HANDLES: u32 =
        WINHTTP_CALLBACK_STATUS_HANDLE_CREATED | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING;
    const ADDRESSES: [&str; 2] = ["9.9.9.9", "2620:fe::fe"];
    struct Slot;
    impl Drop for Slot {
        fn drop(&mut self) {
            ACTIVE.store(false, Ordering::Release);
        }
    }
    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: This owner closes each handle once, after calls using it
            // have returned. Closing the request cancels its asynchronous I/O.
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
    enum Event {
        Sent,
        Headers,
        Error(u32),
    }
    struct Context {
        events: mpsc::SyncSender<Event>,
        _slot: Arc<Slot>,
    }
    static NEXT_CONTEXT: AtomicUsize = AtomicUsize::new(1);
    static CONTEXTS: OnceLock<Mutex<BTreeMap<usize, Arc<Context>>>> = OnceLock::new();
    fn contexts() -> std::sync::MutexGuard<'static, BTreeMap<usize, Arc<Context>>> {
        CONTEXTS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    unsafe extern "system" fn callback(
        _: *mut c_void,
        context: usize,
        status: u32,
        data: *mut c_void,
        length: u32,
    ) {
        if context == 0 {
            return;
        }
        if status == WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING {
            // Context values are never-reused integer keys, not raw pointers.
            // In-flight callbacks own their own Arc; a late callback can only
            // find no entry, and can never read freed or replacement state.
            contexts().remove(&context);
            return;
        }
        let Some(context) = contexts().get(&context).cloned() else {
            return;
        };
        let event = match status {
            WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE => Event::Sent,
            WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE => Event::Headers,
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR
                if !data.is_null() && length as usize >= mem::size_of::<WINHTTP_ASYNC_RESULT>() =>
            {
                // SAFETY: WinHTTP supplies this initialized structure for the
                // duration of this callback; its reported size was checked.
                Event::Error(unsafe { (*(data as *const WINHTTP_ASYNC_RESULT)).dwError })
            }
            _ => return,
        };
        let _ = context.events.try_send(event);
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn last_error() -> u32 {
        // SAFETY: Reads this thread's last Windows API error.
        unsafe { GetLastError() }
    }
    fn handle(value: *mut c_void) -> Result<Handle, u32> {
        if value.is_null() {
            Err(last_error())
        } else {
            Ok(Handle(value))
        }
    }
    fn option<T>(handle: &Handle, name: u32, value: &T) -> Result<(), u32> {
        // SAFETY: Each caller supplies the option's documented fixed-size value.
        if unsafe {
            WinHttpSetOption(
                handle.0,
                name,
                ptr::from_ref(value).cast(),
                mem::size_of::<T>() as u32,
            )
        } == 0
        {
            Err(last_error())
        } else {
            Ok(())
        }
    }
    fn wait(
        events: &mpsc::Receiver<Event>,
        deadline: Instant,
        headers: bool,
    ) -> Result<(), Failure> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(Failure::Timeout)?;
        match events.recv_timeout(remaining) {
            Ok(Event::Error(code)) => Err(Failure::Windows(code)),
            Ok(Event::Headers) if headers => Ok(()),
            Ok(Event::Sent) if !headers => Ok(()),
            Ok(_) => Err(Failure::Windows(ERROR_WINHTTP_INCORRECT_HANDLE_STATE)),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Failure::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(Failure::Windows(ERROR_WINHTTP_OPERATION_CANCELLED))
            }
        }
    }
    #[derive(Debug)]
    enum Failure {
        Timeout,
        Windows(u32),
    }
    impl From<u32> for Failure {
        fn from(code: u32) -> Self {
            Self::Windows(code)
        }
    }

    fn request(
        address: &str,
        port: u16,
        deadline: Instant,
        slot: Arc<Slot>,
    ) -> Result<u16, Failure> {
        if Instant::now() >= deadline {
            return Err(Failure::Timeout);
        }
        let agent = wide("Quad9Connect/0.1 diagnostic");
        let host = wide("dns.quad9.net");
        let verb = wide("HEAD");
        let path = wide("/dns-query");
        // SAFETY: All strings are live NUL-terminated UTF-16. No proxy, supplied
        // credentials, body, redirect target, or user-selected URL is involved.
        let session = handle(unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                ptr::null(),
                ptr::null(),
                WINHTTP_FLAG_ASYNC,
            )
        })?;
        option(
            &session,
            WINHTTP_OPTION_ENABLE_HTTP_PROTOCOL,
            &WINHTTP_PROTOCOL_FLAG_HTTP2,
        )?;
        option(
            &session,
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            &(WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3),
        )?;
        // SAFETY: Session and host string remain live throughout handle creation.
        let connection = handle(unsafe { WinHttpConnect(session.0, host.as_ptr(), port, 0) })?;
        // SAFETY: Live connection and strings; WinHTTP copies request metadata.
        let request = handle(unsafe {
            WinHttpOpenRequest(
                connection.0,
                verb.as_ptr(),
                path.as_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        })?;
        option(
            &request,
            WINHTTP_OPTION_HTTP_PROTOCOL_REQUIRED,
            &WINHTTP_PROTOCOL_FLAG_HTTP2,
        )?;
        option(
            &request,
            WINHTTP_OPTION_DISABLE_FEATURE,
            &(WINHTTP_DISABLE_AUTHENTICATION | WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS),
        )?;
        option(
            &request,
            WINHTTP_OPTION_MAX_RESPONSE_HEADER_SIZE,
            &16_384u32,
        )?;
        let address = wide(address);
        // This documented request option changes resolution only. SNI, Host,
        // and normal certificate validation still use dns.quad9.net. Refuse an
        // unsupported option; never fall back to the system resolver.
        // SAFETY: Initialized UTF-16 buffer and its byte length remain live.
        if unsafe {
            WinHttpSetOption(
                request.0,
                WINHTTP_OPTION_RESOLUTION_HOSTNAME,
                address.as_ptr().cast(),
                (address.len() * 2) as u32,
            )
        } == 0
        {
            return Err(last_error().into());
        }
        let (send, events) = mpsc::sync_channel(4);
        let context = NEXT_CONTEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Failure::Windows(ERROR_WINHTTP_INTERNAL_ERROR))?;
        contexts().insert(
            context,
            Arc::new(Context {
                events: send,
                _slot: slot,
            }),
        );
        if let Err(error) = option(&request, WINHTTP_OPTION_CONTEXT_VALUE, &context) {
            contexts().remove(&context);
            return Err(error.into());
        }
        // SAFETY: Callback has the WinHTTP ABI. The context remains allocated
        // until the final closing event; callback does no blocking API work.
        let previous = unsafe {
            WinHttpSetStatusCallback(
                request.0,
                Some(callback),
                WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE
                    | WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE
                    | WINHTTP_CALLBACK_STATUS_REQUEST_ERROR
                    | CALLBACK_HANDLES,
                0,
            )
        };
        if previous.is_some_and(|f| f as usize == usize::MAX) {
            let error = last_error();
            contexts().remove(&context);
            return Err(error.into());
        }
        if Instant::now() >= deadline {
            return Err(Failure::Timeout);
        }
        // SAFETY: Live request; no optional body or headers. Context is a bound
        // integer registry key that remains owned through asynchronous closure.
        if unsafe { WinHttpSendRequest(request.0, ptr::null(), 0, ptr::null(), 0, 0, context) } == 0
        {
            return Err(last_error().into());
        }
        wait(&events, deadline, false)?;
        // SAFETY: Send completion was consumed; no other operation is in flight.
        if unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) } == 0 {
            return Err(last_error().into());
        }
        wait(&events, deadline, true)?;
        let mut protocol = 0u32;
        let mut protocol_length = mem::size_of::<u32>() as u32;
        // SAFETY: The completed request writes its negotiated protocol DWORD.
        if unsafe {
            WinHttpQueryOption(
                request.0,
                WINHTTP_OPTION_HTTP_PROTOCOL_USED,
                ptr::from_mut(&mut protocol).cast(),
                &mut protocol_length,
            )
        } == 0
        {
            return Err(last_error().into());
        }
        if protocol != WINHTTP_PROTOCOL_FLAG_HTTP2 {
            return Err(Failure::Windows(ERROR_WINHTTP_INVALID_SERVER_RESPONSE));
        }
        let mut status = 0u32;
        let mut length = mem::size_of::<u32>() as u32;
        // SAFETY: Headers have completed; the numeric query writes one DWORD.
        if unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                ptr::null(),
                ptr::from_mut(&mut status).cast(),
                &mut length,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error().into());
        }
        if !(200..=599).contains(&status) {
            return Err(Failure::Windows(ERROR_WINHTTP_INVALID_SERVER_RESPONSE));
        }
        Ok(status as u16)
    }
    fn result(address: &'static str, response: Result<u16, Failure>) -> EndpointReport {
        let (outcome, http_status, windows_error) = match response {
            Ok(status) => (Outcome::HttpsResponse, Some(status), None),
            Err(Failure::Timeout) => (Outcome::TimedOut, None, None),
            Err(Failure::Windows(ERROR_WINHTTP_TIMEOUT)) => {
                (Outcome::TimedOut, None, Some(ERROR_WINHTTP_TIMEOUT))
            }
            Err(Failure::Windows(code)) => (Outcome::Failed, None, Some(code)),
        };
        EndpointReport {
            address,
            outcome,
            http_status,
            windows_error,
        }
    }
    pub(super) fn probe(deadline: Instant) -> UpstreamReport {
        let started = Instant::now();
        let checked_at_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let endpoints = if ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            ADDRESSES
                .iter()
                .map(|address| EndpointReport {
                    address,
                    outcome: Outcome::Busy,
                    http_status: None,
                    windows_error: None,
                })
                .collect()
        } else {
            let slot = Arc::new(Slot);
            std::thread::scope(|scope| {
                let v6_slot = Arc::clone(&slot);
                let v6 = std::thread::Builder::new()
                    .name("quad9-direct-ipv6".into())
                    .spawn_scoped(scope, move || {
                        result(ADDRESSES[1], request(ADDRESSES[1], 443, deadline, v6_slot))
                    });
                let v4 = result(ADDRESSES[0], request(ADDRESSES[0], 443, deadline, slot));
                let v6 = match v6 {
                    Ok(worker) => worker.join().unwrap_or_else(|_| {
                        result(
                            ADDRESSES[1],
                            Err(Failure::Windows(ERROR_WINHTTP_INTERNAL_ERROR)),
                        )
                    }),
                    Err(error) => result(
                        ADDRESSES[1],
                        Err(Failure::Windows(
                            error
                                .raw_os_error()
                                .and_then(|code| u32::try_from(code).ok())
                                .unwrap_or(ERROR_WINHTTP_INTERNAL_ERROR),
                        )),
                    ),
                };
                vec![v4, v6]
            })
        };
        UpstreamReport { checked_at_unix_seconds, elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64, endpoints,
            scope: "TLS-validated HTTP/2 HEAD responses from fixed Quad9 IPv4/IPv6 endpoints. No DNS answer is checked; this does not verify the system resolver. HTTP status is reported without following redirects." }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::Duration;
        #[test]
        fn cancellation_releases_context_and_slot_after_a_silent_peer() {
            use std::{io::Read, net::TcpListener};
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            listener.set_nonblocking(true).unwrap();
            // No TLS response. The client must cancel the real pending request.
            let peer = std::thread::spawn(move || {
                let until = Instant::now() + Duration::from_secs(3);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < until =>
                        {
                            std::thread::sleep(Duration::from_millis(10))
                        }
                        other => panic!("Local fixture did not receive the connection: {other:?}"),
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut buffer = [0; 4096];
                while socket.read(&mut buffer).is_ok_and(|n| n > 0) {}
            });
            assert!(ACTIVE
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok());
            let slot = Arc::new(Slot);
            let busy = probe(Instant::now() + Duration::from_millis(50));
            assert!(busy
                .endpoints
                .iter()
                .all(|endpoint| endpoint.outcome == Outcome::Busy));
            assert!(
                contexts().is_empty(),
                "Busy checks must not create native requests"
            );
            let start = Instant::now();
            let response = request("127.0.0.1", port, start + Duration::from_millis(500), slot);
            assert!(matches!(response, Err(Failure::Timeout)), "{response:?}");
            assert!(start.elapsed() < Duration::from_secs(2));
            let deadline = Instant::now() + Duration::from_secs(3);
            while ACTIVE.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                !ACTIVE.load(Ordering::Acquire),
                "Closing callback retained the slot"
            );
            assert!(
                contexts().is_empty(),
                "Canceled request context was retained"
            );
            let retired = NEXT_CONTEXT.load(Ordering::Relaxed) - 1;
            // SAFETY: A retired integer key has no registered context. The
            // callback must ignore it without reading the absent data buffer.
            unsafe {
                callback(
                    ptr::null_mut(),
                    retired,
                    WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
                    ptr::null_mut(),
                    0,
                );
            }
            assert!(contexts().is_empty());
            peer.join().unwrap();
        }
        #[test]
        fn https_responses_preserve_error_status_without_claiming_dns_success() {
            let report = result("9.9.9.9", Ok(503));
            assert_eq!(report.outcome, Outcome::HttpsResponse);
            assert_eq!(report.http_status, Some(503));
            assert_eq!(
                result("9.9.9.9", Err(Failure::Timeout)).outcome,
                Outcome::TimedOut
            );
        }
    }
}
