# Service development and protocol

Use disposable Windows VMs with snapshots for development service installation and DNS mutations. The intended support platform is Windows 11 x64; hosted Windows Server CI provides additional API evidence.

An explicitly authorized Windows 11 host run can use `scripts/validation/Test-Windows11Host.ps1`. Launch it in a dedicated 64-bit administrator PowerShell process with `-File`, an explicit physical adapter GUID, the production service and inspection binaries, the chosen operator SID, and a fresh `ProgramData\Quad9ConnectHostValidation-<32 hex digits>` directory. It requires an automatic DNS baseline, crosschecked against native API and static registry settings, without per-server or profile properties. It refuses existing installations, recovery data, managed DNS policy, and uncovered resolver scopes. It uses production adapter checks without the VM bypass. Other baseline variants belong in disposable VM fixtures.

Before any DNS write, the runner saves the native baseline in the protected run directory and registers a SYSTEM recovery task for three minutes later and the next Windows startup. The task can stop the specific test process if it has exceeded its deadline. Recovery writes only settings that still exactly match the test profile; it preserves external changes and retains the service and journal when recovery cannot finish. A successful run removes its task and temporary service, reverses only its new operator enrollment, and archives recovery records in the protected run directory. It tests the original-to-strict-profile roundtrip, service restart while active, exact restoration, and historical retries after restart. It does not create test accounts or alter policy. The existing disposable-VM fixtures cover additional static-DNS cases.

Run `scripts/validation/Test-HostValidationHelpers.ps1` without elevation to test recovery decisions with synthetic state. Host-specific results identify tested address families and remaining compatibility gaps. Authorization for one host test does not make these scripts an automatic installation or startup action.

## Build and install

```powershell
cargo build --locked -p quad9-service
# In an elevated PowerShell inside the VM:
./scripts/Install-DevelopmentService.ps1 -BinaryPath ./target/debug/quad9-service.exe -OperatorAccount "$env:COMPUTERNAME\YourUser"
```

The script copies the binary into protected Program Files storage and starts the automatic LocalSystem `Quad9Connect` service. It creates `Quad9ConnectOperators` and enrolls the selected local user. Sign in again after enrollment so the token contains the new group. Installation leaves DNS off. The script refuses an existing service or install directory so it cannot reuse planted executables or libraries. For a repeat development install, inspect and remove the prior Program Files directory after successful service removal. Keep the separate ProgramData recovery records.

The service uses the Windows known ProgramData folder for `Quad9Connect`, with protected administrator/SYSTEM access. Its recovery journal, request receipts, and lock remain outside the checkout. A preexisting directory or file with unsafe permissions causes startup to fail. Do not delete recovery files to dismiss an error.

## Local requests

The unprivileged client verifies the pipe server against the installed running LocalSystem service. The server rejects remote pipe clients and authorizes the connected token after reading a bounded request. It reverts impersonation before executing commands. One serialized controller owns mutations.

```powershell
'{"version":1,"id":"inspect-1","command":{"type":"get_status"}}' | & 'C:\Program Files\Quad9Connect\quad9-service.exe' request
'{"version":1,"id":"adapters-1","command":{"type":"list_adapters"}}' | & 'C:\Program Files\Quad9Connect\quad9-service.exe' request
```

Commands are `get_status`, `list_adapters`, `run_diagnostics`, `enable_quad9`, and `restore_previous_settings`. Mutations require the latest `expected_revision`. Enable also requires distinct explicit `{ "adapter_id": "<GUID>", "family": "ipv4" }` targets. Select supported IPv6 scopes where usable, and account for every active resolver scope. The server decides eligibility independently of the client.

Every request has protocol `version: 1` and a caller-chosen ASCII `id` of at most 64 characters. Reuse the same ID and exact payload to retrieve an uncertain mutation's receipt. Use a fresh ID for a new action. A historical receipt describes that request's outcome; read current status to determine the present configuration.

The service durably reserves a receipt and advances the revision before invoking the DNS engine. It keeps 128 receipts per machine, scoped to caller SID and ID. Stale revisions prevent replay after eviction. Pending receipts become `interrupted` after restart, with uncertain writes reconciled from the DNS journal. A missing expected journal blocks mutations and never triggers a DHCP reset.

Frames contain a little-endian four-byte length and UTF-8 JSON, capped at 64 KiB. Each connection handles one request and response; the client acknowledges the complete response with byte `0xa5`. Both sides enforce deadlines. The command contract accepts no executable, shell, registry, resolver-template, or storage path parameters.

## Restore and remove

```powershell
# In an elevated PowerShell inside the VM:
./scripts/Uninstall-DevelopmentService.ps1
```

Removal starts the service if needed, requests ownership-aware restoration, and removes the service only after complete restoration. It retains protected recovery records and the operators group for inspection. External settings or missing adapters can block removal. Resolve those conditions through the service rather than resetting all adapters.

## Validation

The hosted workflow first saves the VM's actual DNS baseline outside the repository and restores it in a final cleanup step. Native contract fixtures cover both families, static order, empty encryption properties, strict DoH, and existing fallback flags. Separate standard users exercise operator authorization and unauthorized denial. Unit tests inject persistence failures and interruptions at transaction boundaries.

`cargo build -p quad9-service --features vm-validation` adds the `service-vm` entry point, allowing virtual Ethernet in the disposable CI VM. The development installer accepts that entry point only when both GitHub-hosted environment guards are present. The ordinary `service` entry point always uses production scope checks.
