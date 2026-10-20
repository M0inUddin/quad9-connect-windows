# Windows development installer

The v0.1 installer is an unsigned development build for Windows 11 x64. It installs the Tauri interface and the production LocalSystem service. It does not enable DNS management on a fresh installation. This community project is independent of Quad9.

## Install and upgrade

Install the Microsoft Edge WebView2 Evergreen Runtime **for all users** first. Setup checks the machine registration and stops with a download link if it is missing. Setup does not download or execute a runtime installer automatically.

Run `Quad9Connect-0.1.0-dev-<build>-unsigned-x64-setup.exe` and approve elevation. Select the local user who will control the app. Enrollment in `Quad9ConnectOperators` grants control over machine-wide DNS settings. A newly enrolled user needs to sign out and sign in again to refresh their Windows token. The Start menu shortcut launches the interface without elevation.

Run a newer installer to upgrade. Choose **Keep existing access**. Upgrades retain user preferences, operator membership, active DNS intent, receipts, and the original recovery snapshot. Setup stops the service before replacing its registration, so no DNS transaction can overlap the replacement. A protected maintenance marker blocks client mutations while setup checks the replacement. Setup starts the candidate and verifies the versioned status response before committing installation metadata and allowing mutations again.

Each payload lives in `%ProgramFiles%\Quad9Connect\versions\<SHA-256 build identifier>`. The previous payload remains available until removal. A failed replacement restores the previous service registration and starts that service without restoring or re-enabling committed DNS. The current v0.1 storage format is unchanged during upgrades. Unknown storage formats and pending recovery fail preflight instead of attempting a speculative migration. Future storage migrations require their own rollback design.

**Settings > Windows startup** controls only the current user's interface at Windows sign-in. Choose Off, Open window, or Tray; the default is Off. No Quad9 account or saved password is involved. Each opted-in user has a least-privilege interactive task. Setup updates its executable path during upgrades and rollback, including for users who are logged out, while preserving its mode, disabled state, and user access. A shared installation lock prevents preference changes from racing setup. The DNS service's automatic startup remains independent.

Setup records staging ownership before creating or copying a candidate directory. An interrupted copy is retained under the protected `retained-staging` directory on retry, without executing it or changing the registered service. Replacement recovery authenticates the previous recovery executable; missing or quarantined candidate UI files do not prevent rollback.

For silent installation, specify an enabled local user SID explicitly:

```powershell
Get-LocalUser | Select-Object Name, SID
.\Quad9Connect-<version>-<build>-unsigned-x64-setup.exe /S /OPERATORSID=S-1-5-21-<machine>-<user>
```

Silent upgrades preserve existing authorization and omit `/OPERATORSID`. For setup, exit code zero means success. Nonzero means installation stopped. Setup rejects an unrelated service, changed payloads, unsupported metadata, reparse points, or directories that ordinary users can modify.

## Safe uninstall and repair

Choose **Quit interface** from the tray menu before upgrading or removing the app. Setup checks for an open installed interface before changing service state or removing retry commands. Closing the window alone keeps the tray process running.

Use Windows Installed apps, or run the registered `Uninstall.exe`. Uninstall stops the service and takes the same exclusive recovery-store lock used by the service. The installed service executable then performs ownership-aware restoration offline. This prevents a client from enabling DNS between restoration and service removal.

For unattended removal, run the registered `QuietUninstallString` **as an administrator or SYSTEM**. It invokes the installed PowerShell lifecycle command synchronously and returns nonzero when recovery blocks removal. The interactive NSIS uninstaller copies itself to a child process; its bootstrap exit code alone is not a completion result. Automation should use `QuietUninstallString`, or invoke `Manage-Quad9Installation.ps1 -Operation Uninstall` from an elevated session.

If an adapter is missing, recovery data is unreadable, or another application changed DNS, removal stops and retains the service, binaries, and journals. Newer external settings remain untouched. Resolve the reported conflict before retrying. A generic DNS reset is never performed.

An elevated PowerShell session can repair the installation without the interface:

```powershell
$root = Join-Path $env:ProgramFiles 'Quad9Connect'
$state = Get-Content (Join-Path $root 'installation.json') -Raw | ConvertFrom-Json
& (Join-Path $root "versions\$($state.current)\Manage-Quad9Installation.ps1") -Operation Repair
```

Repair first reconciles any interrupted installer transaction in `pending.json`, then restores still-owned DNS through the installed version and restarts the service. If setup failed before a current state was committed, rerun the same installer. Its embedded lifecycle code reads the protected pending record and attempts rollback before installing again. Retained payloads also contain the repair command.

For manual diagnosis, stop `Quad9Connect`, then run the retained service binary as an administrator with `maintenance check` or `maintenance restore`. `check` reads and validates the persisted formats without DNS writes. `restore` uses the transaction engine and returns nonzero on incomplete recovery. Both commands refuse concurrent access to an active service's recovery store. Restart the service after diagnosis or an incomplete restore.

Successful uninstall removes only inventoried files whose checksums still match, the service registration, the app shortcut, and operator membership created by setup. Preexisting membership and shared groups remain. Changed or unknown files remain for inspection. A later reinstall moves remaining version directories under `retained-removal` so the same build can be installed again without deleting those files. Protected `%ProgramData%\Quad9Connect` recovery records and small installation metadata remain. They are never recursively deleted by setup.

After DNS restoration and service removal, uninstall removes owned Windows startup tasks before deleting payloads. Ownership requires the task marker, SID-bound name, least-privilege interactive principal, matching logon trigger, and one known desktop action with supported arguments. A changed or unrelated task is preserved. If task cleanup fails, payload removal stops and can be retried. Selecting Off in the app removes only that user's owned registration and does not terminate its current interface.

## Build and validation

Use Windows x64 with Rust 1.99.1 MSVC, the C++ build tools and Windows SDK, Node 24, and the committed npm and Cargo lockfiles:

```powershell
cargo build --release --locked -p quad9-service
Push-Location apps/desktop
npm ci --no-audit --no-fund
npm run tauri -- build --no-bundle
Pop-Location
$nsis = ./scripts/Get-InstallerToolchain.ps1
./scripts/Build-WindowsInstaller.ps1 -MakeNsis $nsis
```

The project owns its NSIS entry point so upgrades do not invoke an old uninstaller before preserving active service state. Setup creates a unique Program Files staging directory with administrator/SYSTEM-only permissions before extracting or executing lifecycle scripts. It loads only trusted Windows PowerShell system modules, including when launched from PowerShell 7. Failed setup retains an administrator-only `lifecycle-error.txt` in that staging directory. Successful setup removes staging files without recursive deletion.

The compiler is NSIS 3.13 from Tauri's binary release, pinned by SHA-256. The build emits a clearly named unsigned setup executable, `.sha256` checksum, and build metadata in ignored `artifacts/installer`. Build identifiers include the version and payload checksums. The GUI binary remains a normal, unelevated Tauri executable.

`Test-InstallerLifecycle.ps1` exercises payload integrity, path validation, failed restoration, and rollback without DNS changes. It runs under both PowerShell editions in CI. The Windows installer workflow builds the actual MSVC release binaries and NSIS package, then tests fresh installation, standard-user access, mutation gating, active upgrade, failed replacement, interrupted upgrade reconciliation, conflict-blocked uninstall, repair, active uninstall, and reinstall in a disposable Windows Server 2025 VM. A separate VM-only binary establishes adapter fixtures. It is never included in the distributable installer.

Public supported distribution still requires Authenticode signing, Windows 11 installer validation, the remaining compatibility matrix, and the v0.1 resource and accessibility gates. CI artifacts are development builds, not a supported release.

References: [Tauri Windows distribution](https://v2.tauri.app/distribute/windows-installer/), [NSIS scripting](https://nsis.sourceforge.io/Docs/Chapter4.html), [NSIS 3.13 notes](https://nsis.sourceforge.io/Docs/AppendixF.html), [Microsoft WebView2 detection](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution), and [Windows service configuration](https://learn.microsoft.com/en-us/windows/win32/services/changing-a-service-configuration).
