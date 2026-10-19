use quad9_native::{inspect_adapters, probe_system_resolver, ProbeOutcome};
use std::process::ExitCode;

const HELP: &str = "Quad9 Connect for Windows 0.1 development diagnostics

Usage: quad9-inspect <adapters|probe> [--json]

  adapters   Inspect Windows adapter candidates and raw DNS settings
  probe      Make one fresh system-resolver lookup to proto.on.quad9.net.
  --json     Write a versioned JSON result to stdout

This tool reads configuration. It cannot enable, restore, or reset DNS.
A probe describes that lookup only, not every adapter or application.
Exit codes: 0 success, 1 error, 2 invalid arguments, 3 DoH not observed.
";

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let valid = (args.len() == 1 || (args.len() == 2 && args[1] == "--json"))
        && args
            .first()
            .is_some_and(|s| s == "adapters" || s == "probe");
    if !valid {
        eprintln!("{HELP}");
        return ExitCode::from(2);
    }
    let json = args.len() == 2;
    match run(&args[0], json) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"schema_version": 1, "error": error.to_string()})
                );
            } else {
                eprintln!("{error}");
            }
            ExitCode::FAILURE
        }
    }
}

fn run(command: &str, json: bool) -> Result<u8, Box<dyn std::error::Error>> {
    if command == "adapters" {
        let adapters = inspect_adapters()?;
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"schema_version": 1, "adapters": adapters})
                )?
            );
        } else {
            if adapters.is_empty() {
                println!("No network adapters found.");
            }
            for adapter in adapters {
                println!(
                    "{} ({:?})\n  GUID: {}",
                    adapter.name,
                    adapter.kind,
                    adapter.id.as_str()
                );
                println!(
                    "  {}",
                    adapter.exclusion_reason.as_deref().unwrap_or(
                        "Physical adapter candidate; policy and restore validation still required"
                    )
                );
                println!(
                    "  IPv4 enabled: {}, IPv6 enabled: {}",
                    adapter.ipv4_enabled, adapter.ipv6_enabled
                );
                if let Some(error) = &adapter.dns_readback_error {
                    println!("  DNS readback unavailable: {error}");
                }
                if let Some(readback) = adapter.dns_readback {
                    println!(
                        "  API name servers: {}",
                        readback.name_servers.as_deref().unwrap_or("<not returned>")
                    );
                }
            }
        }
        return Ok(0);
    }
    let report = probe_system_resolver()?;
    let code = match report.outcome {
        ProbeOutcome::DohObserved => 0,
        ProbeOutcome::Failed | ProbeOutcome::TimedOut => 1,
        _ => 3,
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"schema_version": 1, "probe": report})
            )?
        );
    } else {
        println!(
            "System DNS probe: {:?}\nObserved transport: {:?}\nDuration: {} ms",
            report.outcome, report.transport, report.elapsed_ms
        );
        if let Some(error) = report.error {
            println!("Details: {error}");
        }
        println!("{}", report.scope);
    }
    Ok(code)
}
