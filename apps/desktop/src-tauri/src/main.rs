#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use quad9_service::protocol::{Command, Request, Response, ResultBody};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

#[derive(Clone)]
struct Host {
    gate: Arc<Mutex<()>>,
    restore: MenuItem<tauri::Wry>,
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
        if let Some(tray) = app.tray_by_id("quad9") {
            let _ = tray.set_tooltip(Some(format!("Quad9 Connect · {label}")));
        }
    }
}
#[tauri::command]
async fn service_request(app: tauri::AppHandle, request: Request) -> Result<Response, String> {
    request.validate()?;
    let gate = app.state::<Host>().gate.clone();
    let response = tauri::async_runtime::spawn_blocking(move || {
        let _guard = gate
            .lock()
            .map_err(|_| "The request coordinator is unavailable".to_owned())?;
        quad9_service::host::send(&request)
    })
    .await
    .map_err(|e| e.to_string())??;
    update_tray(&app, &response);
    Ok(response)
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
        .invoke_handler(tauri::generate_handler![service_request])
        .setup(|app| {
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
            let menu = Menu::with_items(app, &[&open, &diagnostics, &settings, &restore, &quit])?;
            app.manage(Host {
                gate: Arc::new(Mutex::new(())),
                restore,
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
                            let result =
                                service_request(app.clone(), request(Command::RunDiagnostics))
                                    .await;
                            let _ = app.emit("diagnostic-result", result);
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
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("Could not start Quad9 Connect");
}
