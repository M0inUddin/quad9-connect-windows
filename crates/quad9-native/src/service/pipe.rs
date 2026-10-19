use super::{security::PipeSecurity, *};
use std::{
    mem,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{Pipes::*, Services::*, Threading::*, IO::*},
};

fn operation(
    handle: HANDLE,
    stop: &AtomicBool,
    deadline: Instant,
    start: impl FnOnce(*mut OVERLAPPED) -> i32,
) -> Result<u32, NativeError> {
    // SAFETY: An unnamed event is returned as one owned kernel handle.
    let event = Handle::checked(
        unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) },
        "CreateEventW",
    )?;
    let mut overlap = OVERLAPPED {
        hEvent: event.0,
        ..Default::default()
    };
    let started = start(&mut overlap);
    if started == 0 {
        // SAFETY: Retrieve the error from the operation just initiated.
        let code = unsafe { GetLastError() };
        if code == ERROR_PIPE_CONNECTED {
            return Ok(0);
        }
        if code != ERROR_IO_PENDING {
            return Err(NativeError::Windows {
                operation: "Pipe I/O",
                code,
            });
        }
    }
    let mut transferred = 0;
    loop {
        // SAFETY: event remains owned and valid while this operation is pending.
        let wait = unsafe { WaitForSingleObject(event.0, 100) };
        if wait == WAIT_OBJECT_0 || started != 0 {
            // SAFETY: overlap identifies this operation and the output is valid.
            if unsafe { GetOverlappedResult(handle, &overlap, &mut transferred, 0) } != 0 {
                return Ok(transferred);
            }
            return Err(last_error("GetOverlappedResult"));
        }
        if wait != WAIT_TIMEOUT || stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            // SAFETY: Cancel exactly this operation, then wait for its completion
            // before the stack OVERLAPPED, event, and borrowed buffers disappear.
            unsafe {
                CancelIoEx(handle, &overlap);
                GetOverlappedResult(handle, &overlap, &mut transferred, 1);
            }
            return Err(invalid("Pipe operation stopped or exceeded its deadline"));
        }
    }
}

fn read_exact(
    handle: HANDLE,
    bytes: &mut [u8],
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<(), NativeError> {
    let mut position = 0;
    while position < bytes.len() {
        let remaining = &mut bytes[position..];
        let received = operation(handle, stop, deadline, |overlap| {
            // SAFETY: remaining is writable and stays borrowed until operation
            // completes or cancellation has been fully drained.
            unsafe {
                ReadFile(
                    handle,
                    remaining.as_mut_ptr(),
                    remaining.len() as u32,
                    ptr::null_mut(),
                    overlap,
                )
            }
        })? as usize;
        if received == 0 {
            return Err(invalid("Pipe closed before the frame completed"));
        }
        position += received;
    }
    Ok(())
}

fn write_all(
    handle: HANDLE,
    bytes: &[u8],
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<(), NativeError> {
    let mut position = 0;
    while position < bytes.len() {
        let remaining = &bytes[position..];
        let sent = operation(handle, stop, deadline, |overlap| {
            // SAFETY: remaining stays live until completion or drained cancellation.
            unsafe {
                WriteFile(
                    handle,
                    remaining.as_ptr(),
                    remaining.len() as u32,
                    ptr::null_mut(),
                    overlap,
                )
            }
        })? as usize;
        if sent == 0 {
            return Err(invalid("Pipe closed before response completed"));
        }
        position += sent;
    }
    Ok(())
}

fn read_frame(
    handle: HANDLE,
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<u8>, NativeError> {
    let mut header = [0u8; 4];
    read_exact(handle, &mut header, stop, deadline)?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(invalid("Invalid IPC frame length"));
    }
    let mut body = vec![0u8; length];
    read_exact(handle, &mut body, stop, deadline)?;
    Ok(body)
}

fn write_frame(
    handle: HANDLE,
    body: &[u8],
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<(), NativeError> {
    if body.is_empty() || body.len() > MAX_FRAME {
        return Err(invalid("Invalid IPC response length"));
    }
    write_all(handle, &(body.len() as u32).to_le_bytes(), stop, deadline)?;
    write_all(handle, body, stop, deadline)
}

pub fn serve(
    stop: &AtomicBool,
    ready: Ready,
    mut handle_request: impl FnMut(&str, &[u8]) -> Vec<u8>,
) -> Result<(), NativeError> {
    let security = PipeSecurity::new()?;
    // SAFETY: Fixed local pipe, protected DACL, one instance, and overlapped I/O.
    let pipe = Handle::checked(
        unsafe {
            CreateNamedPipeW(
                wide(PIPE_NAME).as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                MAX_FRAME as u32,
                MAX_FRAME as u32,
                0,
                &security.descriptor.attributes(),
            )
        },
        "CreateNamedPipeW",
    )?;
    ready()?;
    while !stop.load(Ordering::Acquire) {
        let connected = operation(
            pipe.0,
            stop,
            Instant::now() + Duration::from_secs(1),
            |overlap| {
                // SAFETY: pipe is live and overlap remains valid through completion.
                unsafe { ConnectNamedPipe(pipe.0, overlap) }
            },
        );
        if connected.is_ok() {
            let _ = (|| -> Result<(), NativeError> {
                let request = read_frame(pipe.0, stop, Instant::now() + Duration::from_secs(5))?;
                // authorize reverts impersonation before returning to this scope.
                let actor = security.authorize(pipe.0)?;
                let response = handle_request(&actor, &request);
                write_frame(
                    pipe.0,
                    &response,
                    stop,
                    Instant::now() + Duration::from_secs(5),
                )?;
                let mut ack = [0u8; 1];
                read_exact(
                    pipe.0,
                    &mut ack,
                    stop,
                    Instant::now() + Duration::from_secs(2),
                )?;
                if ack[0] != 0xa5 {
                    return Err(invalid("Invalid response acknowledgment"));
                }
                Ok(())
            })();
        }
        // SAFETY: Reset this single instance. Never call blocking FlushFileBuffers
        // on an untrusted client; the bounded acknowledgment drains normal replies.
        unsafe {
            DisconnectNamedPipe(pipe.0);
        }
    }
    Ok(())
}

struct ScHandle(SC_HANDLE);
impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: This handle came from OpenSCManager/OpenService.
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

fn verify_server(pipe: HANDLE) -> Result<(), NativeError> {
    let mut pid = 0;
    // SAFETY: This is the connected client end and the output pointer is valid.
    if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 {
        return Err(last_error("GetNamedPipeServerProcessId"));
    }
    // SAFETY: Request only connection/query rights against the local SCM.
    let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(last_error("OpenSCManagerW"));
    }
    let manager = ScHandle(manager);
    // SAFETY: The fixed service name is terminated and manager is live.
    let service = unsafe {
        OpenServiceW(
            manager.0,
            wide(SERVICE_NAME).as_ptr(),
            SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG,
        )
    };
    if service.is_null() {
        return Err(last_error("OpenServiceW"));
    }
    let service = ScHandle(service);
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut bytes = 0;
    // SAFETY: status has the correct size for SC_STATUS_PROCESS_INFO.
    if unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut bytes,
        )
    } == 0
    {
        return Err(last_error("QueryServiceStatusEx"));
    }
    if status.dwCurrentState != SERVICE_RUNNING || status.dwProcessId != pid {
        return Err(invalid(
            "The pipe server does not match the installed Quad9 service",
        ));
    }
    // SCM configuration is queryable by ordinary operators, unlike a SYSTEM
    // process token. Require a dedicated LocalSystem process and its exact PID.
    // SAFETY: The first call requests the required output size only.
    unsafe {
        QueryServiceConfigW(service.0, ptr::null_mut(), 0, &mut bytes);
    }
    if bytes == 0 || bytes > 65_536 {
        return Err(invalid("Invalid service configuration size"));
    }
    let mut buffer = vec![0u128; (bytes as usize).div_ceil(16)];
    let config = buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    // SAFETY: The aligned buffer has at least the requested bytes.
    if unsafe { QueryServiceConfigW(service.0, config, bytes, &mut bytes) } == 0 {
        return Err(last_error("QueryServiceConfigW"));
    }
    // SAFETY: The complete configuration and its strings remain in buffer.
    let config = unsafe { &*config };
    if config.dwServiceType != SERVICE_WIN32_OWN_PROCESS || config.lpServiceStartName.is_null() {
        return Err(invalid("Unexpected service identity"));
    }
    let mut len = 0;
    // SAFETY: SCM returns a terminated account string within its output buffer.
    while len < 256 && unsafe { *config.lpServiceStartName.add(len) } != 0 {
        len += 1;
    }
    if len == 256 {
        return Err(invalid("Overlong service identity"));
    }
    // SAFETY: The preceding scan bounded this initialized string.
    let account =
        String::from_utf16(unsafe { std::slice::from_raw_parts(config.lpServiceStartName, len) })
            .map_err(|_| invalid("Invalid service identity"))?;
    if !account.eq_ignore_ascii_case("LocalSystem")
        && !account.eq_ignore_ascii_case(r"NT AUTHORITY\SYSTEM")
    {
        return Err(invalid("The installed service is not LocalSystem"));
    }
    Ok(())
}

pub fn transact(request: &[u8]) -> Result<Vec<u8>, NativeError> {
    if request.is_empty() || request.len() > MAX_FRAME {
        return Err(invalid("Invalid IPC request length"));
    }
    let name = wide(PIPE_NAME);
    // SAFETY: Fixed local pipe name, bounded wait, no remote endpoint.
    if unsafe { WaitNamedPipeW(name.as_ptr(), 5_000) } == 0 {
        return Err(last_error("WaitNamedPipeW"));
    }
    // SECURITY_IDENTIFICATION permits identity inspection without allowing a
    // malicious server to act with the client's privileges.
    // SAFETY: Name is terminated; desired rights exclude CREATE_PIPE_INSTANCE.
    let pipe = Handle::checked(
        unsafe {
            CreateFileW(
                name.as_ptr(),
                0x0012_0083,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                null_handle(),
            )
        },
        "Open local service pipe",
    )?;
    verify_server(pipe.0)?;
    let stop = AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(30);
    write_frame(pipe.0, request, &stop, deadline)?;
    let response = read_frame(pipe.0, &stop, deadline)?;
    write_all(pipe.0, &[0xa5], &stop, deadline)?;
    Ok(response)
}
