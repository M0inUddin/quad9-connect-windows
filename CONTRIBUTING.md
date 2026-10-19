# Contributing

Use `feat/<description>` branches for features and `fix/<description>` branches for bug fixes. Open a pull request for each reviewable change. Keep `main` for reviewed changes, and do not merge a pull request without the maintainer's instruction. Include a failing regression case with a bug fix when it demonstrates meaningful behavior.

The [project specification](docs/project-specification.md) defines scope. The [implementation status](docs/implementation-status.md) records progress and release gates. Version 0.1 is in development and is not a supported desktop release.

For the unsigned NSIS package, build prerequisites, upgrade behavior, and recovery commands, see [Windows development installer](docs/windows-installer.md). Generated installers, toolchains, payload staging, and validation reports belong in ignored `artifacts/`.

## Build the Rust workspace

Install [Rust with rustup](https://rust-lang.org/tools/install/). The repository pins the toolchain and formatting tools. Windows developers should use the default MSVC toolchain and install the C++ build tools and Windows SDK required by Rust. The platform-independent transaction tests also run on Linux.

```text
cargo build --workspace --locked
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

Run the read-only Windows inspection tool without elevation:

```text
cargo run --locked -p quad9-cli -- adapters
cargo run --locked -p quad9-cli -- probe
cargo run --locked -p quad9-cli -- adapters --json
```

`adapters` reports local adapter identifiers, names, and DNS settings. Review its output before publishing it. `probe` sends one diagnostic query through the system resolver and does not save a report. Exit code 3 means the probe did not observe DoH. A failed or timed-out lookup returns exit code 1. Invalid arguments return exit code 2.

The inspection tool does not install a service, request administrator rights, or modify DNS. The separate service has a narrow mutation contract. Use disposable Windows VMs with snapshots for native write and restore experiments by default. An explicitly authorized Windows 11 host run can use the guarded host validation runner described in the [service guide](docs/service-development.md).

## Review expectations

PR descriptions must explain the user-visible behavior, the relevant checks, and any remaining release gates. Keep implementation claims distinct from test evidence. CI validates compilation, deterministic recovery, adapter inspection, and native write/restore contracts in disposable Windows Server VMs. It does not establish Windows 11, VPN, physical hardware, or distribution installer compatibility.

Keep unsafe code within the Windows integration crate, explain each unsafe operation, and copy API-owned data before releasing its allocation. Changes to the journal format must reject incompatible versions and preserve a recoverable migration path.
