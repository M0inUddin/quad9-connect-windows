// SPDX-License-Identifier: AGPL-3.0-or-later
// On-demand, per-user Windows settings. Never runs during normal/tray startup.
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::windows::{ffi::OsStringExt, process::CommandExt};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
static OPERATION: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Off,
    Window,
    Tray,
}

#[derive(Deserialize, Serialize)]
pub struct Preference {
    available: bool,
    mode: Option<Mode>,
    message: Option<String>,
}

#[derive(Serialize)]
struct Input {
    module: &'static str,
    executable: std::path::PathBuf,
    mode: Option<Mode>,
}

#[derive(Deserialize)]
struct Reply {
    ok: bool,
    value: Option<Preference>,
    error: Option<String>,
}

pub fn preference(mode: Option<Mode>) -> Result<Preference, String> {
    // A read from a newly opened Settings document must not overtake a save
    // that is still running for the document the user just closed.
    let _operation = OPERATION
        .lock()
        .map_err(|_| "Windows startup settings are unavailable.")?;
    let mut system = [0_u16; 260];
    // SAFETY: The writable buffer and its declared length agree. Only the
    // initialized prefix is used after checking the returned required length.
    let count = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            system.as_mut_ptr(),
            system.len() as u32,
        )
    } as usize;
    if count == 0 || count >= system.len() {
        return Err("Cannot locate Windows PowerShell.".into());
    }
    let powershell = std::path::PathBuf::from(std::ffi::OsString::from_wide(&system[..count]))
        .join("WindowsPowerShell/v1.0/powershell.exe");
    let input = serde_json::to_vec(&Input {
        module: include_str!("../../../../installer/windows/InstallerLifecycle.psm1"),
        executable: std::env::current_exe().map_err(|e| e.to_string())?,
        mode,
    })
    .map_err(|e| e.to_string())?;
    let mut child = Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            include_str!("windows-startup-driver.ps1"),
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW; never request elevation
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot open Windows startup settings: {e}"))?;
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(32768).read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(8192).read_to_end(&mut bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(match result {
                    Err(error) => error.to_string(),
                    _ => {
                        "Windows startup settings timed out. Try reading the setting again.".into()
                    }
                });
            }
        }
    };
    let written = writer
        .join()
        .map_err(|_| "Startup settings writer failed.")?;
    let output = reader
        .join()
        .map_err(|_| "Startup settings reader failed.")?;
    let _ = errors.join();
    let status = status?;
    written.map_err(|e| e.to_string())?;
    let output = output.map_err(|e| e.to_string())?;
    if !status.success() || output.len() == 32768 {
        return Err("Windows startup settings could not be read. Try again.".into());
    }
    let reply: Reply = serde_json::from_slice(&output)
        .map_err(|_| "Windows startup settings returned an invalid response.")?;
    if reply.ok {
        reply
            .value
            .ok_or_else(|| "Windows startup setting was missing.".into())
    } else {
        Err(reply
            .error
            .unwrap_or_else(|| "Windows startup settings failed.".into()))
    }
}
