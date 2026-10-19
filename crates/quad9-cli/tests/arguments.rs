use std::process::Command;

fn tool() -> Command {
    Command::new(env!("CARGO_BIN_EXE_quad9-inspect"))
}

#[test]
fn help_explains_read_only_scope() {
    let output = tool().arg("--help").output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("cannot enable, restore, or reset DNS"));
}

#[test]
fn mutation_commands_and_extra_parameters_are_rejected() {
    for args in [
        vec!["enable"],
        vec!["restore"],
        vec!["adapters", "--execute"],
        vec!["probe", "--json", "extra"],
    ] {
        let output = tool().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn empty_command_is_a_usage_error() {
    assert_eq!(tool().output().unwrap().status.code(), Some(2));
}

#[cfg(not(windows))]
#[test]
fn unsupported_platform_returns_a_structured_error() {
    let output = tool().args(["adapters", "--json"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["error"], "This command requires Windows");
}
