use std::process::ExitCode;

const HELP: &str = "Quad9 Connect development service

Usage:
  quad9-service --help
  quad9-service service       Start through the Windows Service Control Manager
  quad9-service request       Read one JSON request from stdin and write its response

Service installation requires an administrator. DNS changes require an
authorized local operator and an explicit versioned request.
";

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    #[cfg(not(windows))]
    {
        eprintln!("The Quad9 service requires Windows");
        ExitCode::FAILURE
    }
    #[cfg(windows)]
    {
        let result: Result<(), String> = match args.as_slice() {
            [command] if command == "service" => {
                quad9_native::service::run_service(quad9_service::host::entry)
                    .map_err(|e| e.to_string())
            }
            #[cfg(feature = "vm-validation")]
            [command] if command == "service-vm" => {
                quad9_native::service::run_service(quad9_service::host::validation_entry)
                    .map_err(|e| e.to_string())
            }
            [command] if command == "request" => request(),
            _ => {
                eprintln!("{HELP}");
                return ExitCode::from(2);
            }
        };
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(windows)]
fn request() -> Result<(), String> {
    use quad9_service::{host, protocol::*};
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_FRAME_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("Request exceeds the IPC size limit".into());
    }
    let request: Request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let response = host::send(&request)?;
    println!(
        "{}",
        serde_json::to_string(&response).map_err(|e| e.to_string())?
    );
    Ok(())
}
