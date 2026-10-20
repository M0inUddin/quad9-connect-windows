# Startup phase profiling

Use phase measurements before changing startup ordering. The three-second release target remains open. The existing [packaged measurements](performance-accessibility.md#packaged-startup-after-refresh-fixes) include CDP observation and do not isolate WebView2 creation.

## Collect a diagnostic

Build the production frontend and native host, then run from `apps/desktop` on Windows without elevation:

```powershell
npm run build
cargo build --manifest-path src-tauri/Cargo.toml --features custom-protocol --release --locked
node scripts/native-startup.mjs src-tauri/target/release/quad9-desktop.exe 5
```

Close any existing Quad9 desktop instance first. Each launch uses a fresh WebView profile in ignored `output/playwright/native-startup/`. A Windows job owns the desktop and its descendants and removes them after each observation. PowerShell launcher startup is excluded from the timer. The driver sends no DNS mutation, installs no service, and changes no Windows settings. It can observe the real unavailable-service state without an installation.

For an already installed, authorized service with DNS management off, append `--require-service`. This requires the settled state to be "Ready when you are". Service installation and enrollment remain separate from this read-only command. Use an explicit output directory for a separate run:

```powershell
node scripts/native-startup.mjs path/to/quad9-desktop.exe 5 ../../artifacts/startup-phases/run-1 --require-service
```

Raw reports include the operating system, runtime version, binary hash, status, document navigation/paint timing, and fixed startup timestamps. Keep profiles and raw reports private; publish reviewed, sanitized evidence separately. The instrumented diagnostic leaves every release-budget decision unassessed.

## Read the phases

Native stamps use monotonic `Instant` elapsed time from entry into Rust `main`. The read-only `startup_timeline` IPC command returns the first occurrence of each fixed phase for the lifetime of that process. Reopening or reloading the window does not reset native stamps. There is no automatic file write, upload, sampling timer, or stored request payload. Concurrent reads cannot replace the first request's start or completion stamps.

| Interval | What it includes |
| --- | --- |
| Windows process creation to `main_entered` | Loader and runtime work before the timing module starts |
| `main_entered` to `builder_ready` | Tauri context, runtime, and plugin construction |
| `builder_ready` to `setup_entered` | Event-loop startup and Tauri's configured-window creation before the application setup hook |
| `setup_entered` to `tray_ready` | Menu, application state, and tray construction |
| `tray_ready` to `monitor_ready` | Native network subscription registration, or persistence of its startup failure |
| `status_started` to `status_finished` | First host status request, including its worker, coordinator wait, authenticated pipe operation, and response handling before tray updates |
| `adapters_started` to `adapters_finished` | Corresponding first adapter request, when reached |

The configured-window interval includes WebView2 work but does not measure browser or renderer process creation separately. Tauri's [page-load hook](https://docs.rs/tauri/2.12.4/tauri/struct.Builder.html#method.on_page_load) supplies separate first navigation started/finished stamps. Those events may overlap other phases and should not be added together.

Document marks identify module execution, transport selection, startup effect, native listener registration, interface-context completion, and the first committed settled interface. Marks are bounded to one entry per fixed label in each document. Reload creates a new document timeline. Native and document elapsed clocks have different origins. Their Unix origins permit approximate alignment with Windows process creation, assuming the system clock does not jump during collection. `Performance.timeOrigin` is a [document timing origin](https://www.w3.org/TR/hr-time-2/#sec-time-origin), not renderer process creation.

The final process-to-ready duration ends when Node observes the settled status through CDP. The later snapshot collection is excluded. A document's `settled` mark is collected afterward and may be slightly later than that observation because React effects run after the DOM commit. An unavailable service ends before adapter enumeration and cannot establish authenticated startup cost.

## Keep safety checks while optimizing

The first service read must remain behind native network registration and frontend event listeners. A failed monitor must remain visible. Current complete status/adapter reads, bounded replacement attempts, mutation receipts, and the idle-close lifecycle remain required. Profile those intervals rather than removing these checks to reduce a number.

CI collects a native phase diagnostic alongside startup race tests and the native accessibility/lifecycle checks. Local diagnostics direct investigation; qualification still requires an identified release MSVC build, 20 authenticated runs, and the documented Windows 11 reference machine.
