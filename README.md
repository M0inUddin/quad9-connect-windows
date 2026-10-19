# Quad9 Connect for Windows

An independent community Windows client for Quad9 DNS, inspired by Quad9 Connect for Android. This project is not affiliated with or endorsed by Quad9.

Version 0.1 is in development. The Rust workspace includes the transaction core, native DNS backend, protected recovery storage, a privileged Windows service, and read-only inspection commands. The [desktop client](apps/desktop/README.md) adds connection controls, adapter selection, diagnostics, themes, and a native tray. An [unsigned development installer](docs/windows-installer.md) adds service installation, upgrades with rollback, administrator repair, and restoration before uninstall. Signed distribution and the remaining compatibility and resource checks are release gates. See the [implementation status](docs/implementation-status.md), [service development guide](docs/service-development.md), and [UI screenshots](docs/screenshots/README.md).

The [final project specification](docs/project-specification.md) defines the stack, release scope, architecture, interface behavior, recovery rules, privacy requirements, and acceptance criteria.

Query history and block notifications belong to a later forwarding-engine milestone. They are not capabilities of the initial native configuration release.

## Development

Install Rust with rustup, then run:

```text
cargo test --workspace --locked
cargo run --locked -p quad9-cli -- adapters
cargo run --locked -p quad9-cli -- probe
cargo run --locked -p quad9-cli -- upstream --json
```

The inspection commands require Windows and do not change DNS. A probe result applies to that lookup only. The separate [direct HTTPS check](docs/direct-upstream-diagnostics.md) reports reachability without claiming system DNS protection. See [CONTRIBUTING.md](CONTRIBUTING.md) for toolchain requirements, exit codes, checks, and the feature and bug-fix PR workflow.

See [LICENSE](LICENSE) for the GNU Affero General Public License, version 3.
