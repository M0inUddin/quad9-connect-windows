#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use quad9_service::protocol::{Command, Request, Response, ResultBody};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

#[derive(Clone)]
struct Host {
    gate: Arc<Mutex<()>>,
    restore: MenuItem<tauri::Wry>,
    status: MenuItem<tauri::Wry>,
    generation: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    monitor_error: Arc<Mutex<Option<String>>>,
}
static NEXT: AtomicU64 = AtomicU64::new(0);
fn request(command: Command) -> Request {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Request {
        version: 1,
        id: format!("tray-{time}-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
        command,
    }
}
fn update_tray(app: &tauri::AppHandle, response: &Response) {
    if let ResultBody::Status {
        journal,
        recovery_blocked,
        observed_matches,
        ..
    } = &response.result
    {
        let owned = journal
            .as_ref()
            .is_some_and(|j| j.phase != quad9_core::TransactionPhase::Restored);
        let label = if *recovery_blocked {
            "Recovery required"
        } else if owned && *observed_matches {
            "Quad9 configured"
        } else if owned {
            "Settings need attention"
        } else {
            "DNS management is off"
        };
        let _ = app.state::<Host>().restore.set_enabled(owned);
        let _ = app.state::<Host>().status.set_text(label);
        if let Some(tray) = app.tray_by_id("quad9") {
            let _ = tray.set_tooltip(Some(format!("Quad9 Connect · {label}")));
        }
    }
}
fn unknown_tray(app: &tauri::AppHandle, label: &str) {
    let host = app.state::<Host>();
    let _ = host.restore.set_enabled(false);
    let _ = host.status.set_text(label);
    if let Some(tray) = app.tray_by_id("quad9") {
        let _ = tray.set_tooltip(Some(format!("Quad9 Connect · {label}")));
    }
}

fn observe_network(app: tauri::AppHandle) -> std::io::Result<()> {
    let host = app.state::<Host>().inner().clone();
    std::thread::Builder::new()
        .name("quad9-network-observer".into())
        .spawn(move || {
            let watcher =
                match quad9_native::network::NetworkWatcher::start(host.generation.clone()) {
                    Ok(watcher) => watcher,
                    Err(error) => {
                        eprintln!("Network notifications unavailable: {error}");
                        if let Ok(mut saved) = host.monitor_error.lock() {
                            *saved = Some(error.to_string());
                        }
                        let _ = app.emit("network-monitor-error", error.to_string());
                        return;
                    }
                };
            while !host.stop.load(Ordering::Acquire) {
                if !watcher.wait(std::time::Duration::from_millis(250)) {
                    continue;
                }
                unknown_tray(&app, "Network changed; checking DNS");
                let _ = app.emit("network-changed", host.generation.load(Ordering::SeqCst));
                // A burst gets one read after 250 ms quiet, or after two seconds at
                // most. The wait consumes notifications, never polls adapter state.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                loop {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero()
                        || !watcher.wait(remaining.min(std::time::Duration::from_millis(250)))
                    {
                        break;
                    }
                }
                if host.stop.load(Ordering::Acquire) {
                    break;
                }
                // Refresh once more after the burst so a read started at the
                // first signal cannot leave a view of an intermediate network.
                let _ = app.emit("network-changed", host.generation.load(Ordering::SeqCst));
                let Ok(_guard) = host.gate.lock() else {
                    break;
                };
                let generation = host.generation.load(Ordering::SeqCst);
                let response = quad9_service::host::send(&request(Command::GetStatus));
                if generation == host.generation.load(Ordering::SeqCst) {
                    match response {
                        Ok(response) if matches!(response.result, ResultBody::Status { .. }) => {
                            update_tray(&app, &response)
                        }
                        _ => unknown_tray(&app, "Service unavailable"),
                    }
                }
            }
        })
        .map(|_| ())
}

#[tauri::command]
fn network_monitor_error(app: tauri::AppHandle) -> Option<String> {
    app.state::<Host>()
        .monitor_error
        .lock()
        .ok()
        .and_then(|error| error.clone())
}

#[tauri::command]
async fn service_request(app: tauri::AppHandle, request: Request) -> Result<Response, String> {
    request.validate()?;
    let gate = app.state::<Host>().gate.clone();
    let generation = app.state::<Host>().generation.clone();
    let status_request = matches!(request.command, Command::GetStatus);
    let observation = matches!(
        request.command,
        Command::GetStatus | Command::ListAdapters | Command::RunDiagnostics
    );
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _guard = gate
            .lock()
            .map_err(|_| "The request coordinator is unavailable".to_owned())?;
        let before = generation.load(Ordering::SeqCst);
        let result = quad9_service::host::send(&request);
        if observation && before != generation.load(Ordering::SeqCst) {
            return Err("The network changed during this check. Read current state and run diagnostics again.".into());
        }
        result
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|result| result);
    match &result {
        Ok(response) if status_request && !matches!(response.result, ResultBody::Status { .. }) => {
            unknown_tray(&app, "Service unavailable")
        }
        Ok(response) => update_tray(&app, response),
        Err(_) => unknown_tray(&app, "Service unavailable"),
    }
    result
}
fn show(app: &tauri::AppHandle, view: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let _ = app.emit("navigate", view);
}
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show(app, "connection")
        }))
        .invoke_handler(tauri::generate_handler![
            service_request,
            network_monitor_error
        ])
        .setup(|app| {
            let title = MenuItem::new(app, "Quad9 Connect for Windows", false, None::<&str>)?;
            let status = MenuItem::new(app, "Reading service state", false, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let open = MenuItem::with_id(app, "open", "Open application", true, None::<&str>)?;
            let diagnostics =
                MenuItem::with_id(app, "diagnostics", "Run diagnostics", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let restore = MenuItem::with_id(
                app,
                "restore",
                "Turn off and restore DNS",
                false,
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, "quit", "Quit interface", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &title,
                    &status,
                    &separator,
                    &open,
                    &diagnostics,
                    &settings,
                    &restore,
                    &quit,
                ],
            )?;
            app.manage(Host {
                gate: Arc::new(Mutex::new(())),
                restore,
                status,
                generation: Arc::new(AtomicU64::new(0)),
                stop: Arc::new(AtomicBool::new(false)),
                monitor_error: Arc::new(Mutex::new(None)),
            });
            TrayIconBuilder::with_id("quad9")
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/128x128.png"
                ))?)
                .tooltip("Quad9 Connect · Open to read service state")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        show(tray.app_handle(), "connection");
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show(app, "connection"),
                    "settings" => show(app, "settings"),
                    "diagnostics" => {
                        show(app, "diagnostics");
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let generation = app.state::<Host>().generation.load(Ordering::SeqCst);
                            let result =
                                service_request(app.clone(), request(Command::RunDiagnostics))
                                    .await;
                            let _ = app.emit(
                                "diagnostic-result",
                                serde_json::json!({
                                    "generation": generation,
                                    "result": result,
                                }),
                            );
                        });
                    }
                    "restore" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let status =
                                service_request(app.clone(), request(Command::GetStatus)).await;
                            if let Ok(Response {
                                result: ResultBody::Status { revision, .. },
                                ..
                            }) = status
                            {
                                let result = service_request(
                                    app.clone(),
                                    request(Command::RestorePreviousSettings {
                                        expected_revision: revision,
                                    }),
                                )
                                .await;
                                let _ = app.emit("tray-result", result);
                            } else {
                                let _ = app.emit("tray-result", status);
                            }
                            let _ = app.emit("service-changed", ());
                            show(&app, "connection");
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            observe_network(app.handle().clone())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("Could not start Quad9 Connect")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<Host>().stop.store(true, Ordering::Release);
            }
        });
}
