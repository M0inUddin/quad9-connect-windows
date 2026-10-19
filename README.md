# Quad9 Connect for Windows

An independent community Windows client for Quad9 DNS, inspired by Quad9 Connect for Android. This project is not affiliated with or endorsed by Quad9.

Version 0.1 is in development. The first increment provides a Rust transaction and recovery core plus a read-only Windows diagnostics tool. The desktop interface, privileged service, native writes, and installer are still pending. See the [implementation status](docs/implementation-status.md).

The [final project specification](docs/project-specification.md) defines the stack, release scope, architecture, interface behavior, recovery rules, privacy requirements, and acceptance criteria.

Query history and block notifications belong to a later forwarding-engine milestone. They are not capabilities of the initial native configuration release.

## Development

Install Rust with rustup, then run:

```text
cargo test --workspace --locked
cargo run --locked -p quad9-cli -- adapters
cargo run --locked -p quad9-cli -- probe
```

The inspection commands require Windows and do not change DNS. A probe result applies to that lookup only. See [CONTRIBUTING.md](CONTRIBUTING.md) for toolchain requirements, exit codes, checks, and the feature and bug-fix PR workflow.

See [LICENSE](LICENSE) for the GNU Affero General Public License, version 3.
