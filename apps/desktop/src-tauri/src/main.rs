#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use quad9_service::client::{ClientCoordinator, ObservedResponse};
use quad9_service::protocol::{Command, Request, Response, ResultBody};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
    Arc, Condvar, Mutex,
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

#[derive(Clone, serde::Serialize)]
struct MonitorFailure {
    generation: u64,
    message: String,
}

#[derive(Clone)]
struct Host {
    requests: Arc<ClientCoordinator>,
    restore: MenuItem<tauri::Wry>,
    status: MenuItem<tauri::Wry>,
    stop: Arc<AtomicBool>,
    monitor_error: Arc<Mutex<Option<MonitorFailure>>>,
    retention: Arc<AtomicU8>,
    retention_sequence: Arc<AtomicU64>,
    visibility_request: Arc<AtomicU64>,
    diagnostic_pending: Arc<AtomicBool>,
    window_transition: Arc<Mutex<()>>,
    destroyed: Arc<(Mutex<u64>, Condvar)>,
    next_view: Arc<Mutex<String>>,
    tray_result: Arc<Mutex<Option<Result<Response, String>>>>,
}
static NEXT: AtomicU64 = AtomicU64::new(0);
const IDLE: u8 = 0;
const RETAINED: u8 = 1;
const DESTROYING: u8 = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_diagnostic_keeps_a_mounting_document_alive() {
        let pending = AtomicBool::new(true);
        let retention = AtomicU8::new(IDLE);
        assert!(reserve_pending_diagnostic(&pending, &retention));
        assert_eq!(retention.load(Ordering::Acquire), RETAINED);
        assert!(!pending.load(Ordering::Acquire));
        assert!(retention
            .compare_exchange(IDLE, DESTROYING, Ordering::AcqRel, Ordering::Acquire)
            .is_err());
    }

    #[test]
    fn closing_document_leaves_diagnostic_for_its_replacement() {
        let pending = AtomicBool::new(true);
        let retention = AtomicU8::new(DESTROYING);
        assert!(!reserve_pending_diagnostic(&pending, &retention));
        assert!(pending.load(Ordering::Acquire));
        assert_eq!(retention.load(Ordering::Acquire), DESTROYING);
        retention.store(IDLE, Ordering::Release);
        assert!(reserve_pending_diagnostic(&pending, &retention));
        assert_eq!(retention.load(Ordering::Acquire), RETAINED);
    }

    #[test]
    fn navigation_without_a_diagnostic_does_not_retain_the_window() {
        let pending = AtomicBool::new(false);
        let retention = AtomicU8::new(IDLE);
        assert!(!reserve_pending_diagnostic(&pending, &retention));
        assert_eq!(retention.load(Ordering::Acquire), IDLE);
    }
}
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
    let (ready, started) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("quad9-network-observer".into())
        .spawn(move || {
            let mut ready = Some(ready);
            let generation = host.requests.generation.clone();
            let result = (|| {
                let watcher = quad9_native::network::NetworkWatcher::start(generation.clone())?;
                if let Some(ready) = ready.take() {
                    let _ = ready.send(());
                }
                while !host.stop.load(Ordering::Acquire) {
                    if !watcher.wait(std::time::Duration::from_millis(250))? {
                        continue;
                    }
                    unknown_tray(&app, "Network changed; checking DNS");
                    let _ = app.emit("network-changed", generation.load(Ordering::SeqCst));
                    // A burst gets one read after 250 ms quiet, or after two seconds at
                    // most. The wait consumes notifications, never polls adapter state.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    loop {
                        let remaining =
                            deadline.saturating_duration_since(std::time::Instant::now());
                        if remaining.is_zero()
                            || !watcher
                                .wait(remaining.min(std::time::Duration::from_millis(250)))?
                        {
                            break;
                        }
                    }
                    if host.stop.load(Ordering::Acquire) {
                        break;
                    }
                    // Refresh once more after the burst so a read started at the
                    // first signal cannot leave a view of an intermediate network.
                    let _ = app.emit("network-changed", generation.load(Ordering::SeqCst));
                    match host
                        .requests
                        .execute(&request(Command::GetStatus), quad9_service::host::send)
                    {
                        Ok(observed)
                            if matches!(observed.response.result, ResultBody::Status { .. }) =>
                        {
                            update_tray(&app, &observed.response)
                        }
                        _ => unknown_tray(&app, "Service unavailable"),
                    }
                }
                Ok::<(), quad9_native::NativeError>(())
            })();
            if let Err(error) = result {
                eprintln!("Network notifications unavailable: {error}");
                let failure = MonitorFailure {
                    generation: generation.load(Ordering::SeqCst),
                    message: error.to_string(),
                };
                if let Ok(mut saved) = host.monitor_error.lock() {
                    *saved = Some(failure.clone());
                }
                // Persist startup failure before setup exposes the webview.
                if let Some(ready) = ready.take() {
                    let _ = ready.send(());
                }
                unknown_tray(&app, "Automatic checks unavailable");
                let _ = app.emit("network-monitor-error", failure);
            }
        })?;
    // Initial webview reads must start after subscriptions are armed, or after
    // their failure has been saved for network_monitor_error to retrieve.
    started.recv().map_err(std::io::Error::other)
}

#[tauri::command]
fn network_monitor_error(app: tauri::AppHandle) -> Option<MonitorFailure> {
    app.state::<Host>()
        .monitor_error
        .lock()
        .ok()
        .and_then(|error| error.clone())
}

#[tauri::command]
async fn service_request(app: tauri::AppHandle, request: Request) -> Result<Response, String> {
    observed_service_request(app, request)
        .await
        .map(|observed| observed.response)
}

async fn observed_service_request(
    app: tauri::AppHandle,
    request: Request,
) -> Result<ObservedResponse, String> {
    let requests = app.state::<Host>().requests.clone();
    let status_request = matches!(request.command, Command::GetStatus);
    let result = tauri::async_runtime::spawn_blocking(move || {
        requests.execute(&request, quad9_service::host::send)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|result| result);
    match &result {
        Ok(observed)
            if status_request && !matches!(observed.response.result, ResultBody::Status { .. }) =>
        {
            unknown_tray(&app, "Service unavailable")
        }
        Ok(observed) => update_tray(&app, &observed.response),
        Err(_) => unknown_tray(&app, "Service unavailable"),
    }
    result
}
#[derive(serde::Serialize)]
struct InterfaceContext {
    view: String,
    diagnostic_pending: bool,
    tray_result: Option<Result<Response, String>>,
}

#[tauri::command]
fn interface_context(app: tauri::AppHandle, window: tauri::Window) -> InterfaceContext {
    let host = app.state::<Host>();
    let view = host.next_view.lock().unwrap().clone();
    let tray_result = host.tray_result.lock().unwrap().take();
    // A startup status read may still be pending when the user closes. Keep
    // that document alive before acknowledging its queued diagnostic.
    let diagnostic_pending = is_current_window(&app, &window)
        && reserve_pending_diagnostic(&host.diagnostic_pending, &host.retention);
    InterfaceContext {
        view,
        diagnostic_pending,
        tray_result,
    }
}

fn reserve_pending_diagnostic(pending: &AtomicBool, retention: &AtomicU8) -> bool {
    if !pending.load(Ordering::Acquire) {
        return false;
    }
    if retention
        .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current != DESTROYING).then_some(RETAINED)
        })
        .is_err()
    {
        return false;
    }
    pending.swap(false, Ordering::AcqRel)
}

#[tauri::command]
fn retain_interface(
    app: tauri::AppHandle,
    window: tauri::Window,
    retain: bool,
    sequence: Option<u64>,
) -> Result<(), String> {
    // Synchronous IPC records retention before a later CloseRequested can
    // dispatch a worker. Destruction atomically claims IDLE, so an accepted
    // busy reservation can never lose its document to a queued destroy.
    let host = app.state::<Host>();
    if !is_current_window(&app, &window) {
        return Err("The requesting interface has already closed".into());
    }
    if let Some(sequence) = sequence {
        if sequence <= host.retention_sequence.load(Ordering::Acquire) {
            return Ok(());
        }
        host.retention_sequence.store(sequence, Ordering::Release);
    }
    host.retention
        .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current != DESTROYING).then_some(if retain { RETAINED } else { IDLE })
        })
        .map_err(|_| "The requesting interface is already closing".to_owned())?;
    if !retain {
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let host = app.state::<Host>();
                let _transition = host.window_transition.lock().map_err(|e| e.to_string())?;
                if is_current_window(&app, &window) && matches!(window.is_visible(), Ok(false)) {
                    destroy_window(&app, &window)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("Could not release interface: {error}");
            }
        });
    }
    Ok(())
}

fn is_current_window(app: &tauri::AppHandle, window: &tauri::Window) -> bool {
    match (
        app.get_webview_window("main")
            .and_then(|current| current.hwnd().ok()),
        window.hwnd().ok(),
    ) {
        (Some(current), Some(candidate)) => current == candidate,
        _ => false,
    }
}

// Called only off the event thread, with window_transition held. A queued
// destroy is not complete until Tauri has removed the old window from its
// manager. The run callback signals that removal before a reopen can proceed.
fn destroy_window(app: &tauri::AppHandle, window: &tauri::Window) -> Result<(), String> {
    let host = app.state::<Host>();
    if host
        .retention
        .compare_exchange(IDLE, DESTROYING, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(());
    }
    let result = (|| {
        let (generation, changed) = &*host.destroyed;
        let before = *generation.lock().map_err(|e| e.to_string())?;
        window.destroy().map_err(|e| e.to_string())?;
        let current = generation.lock().map_err(|e| e.to_string())?;
        let (_completed, timeout) = changed
            .wait_timeout_while(current, std::time::Duration::from_secs(10), |value| {
                *value == before
            })
            .map_err(|e| e.to_string())?;
        if timeout.timed_out() {
            return Err("Window destruction did not finish before its deadline".into());
        }
        Ok(())
    })();
    host.retention.store(IDLE, Ordering::Release);
    result
}

fn hide_interface(window: tauri::Window) {
    let app = window.app_handle().clone();
    let request = app
        .state::<Host>()
        .visibility_request
        .fetch_add(1, Ordering::AcqRel)
        + 1;
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let host = app.state::<Host>();
            let _transition = host.window_transition.lock().map_err(|e| e.to_string())?;
            if request != host.visibility_request.load(Ordering::Acquire)
                || !is_current_window(&app, &window)
            {
                return Ok(());
            }
            window.hide().map_err(|e| e.to_string())?;
            destroy_window(&app, &window)?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Could not hide interface: {error}");
        }
    });
}

fn show(app: &tauri::AppHandle, view: &str) {
    let host = app.state::<Host>();
    *host.next_view.lock().unwrap() = view.to_owned();
    if view == "diagnostics" {
        host.diagnostic_pending.store(true, Ordering::Release);
    }
    let request = host.visibility_request.fetch_add(1, Ordering::AcqRel) + 1;
    let app = app.clone();
    // WebView2 window creation must not block the Windows event thread.
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let host = app.state::<Host>();
            let _transition = host.window_transition.lock().map_err(|e| e.to_string())?;
            if request != host.visibility_request.load(Ordering::Acquire) {
                return Ok(());
            }
            show_window(&app).map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            eprintln!("Could not open interface: {error}");
        }
    });
}

fn show_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        window.unminimize()?;
        window.set_focus()?;
        let view = app.state::<Host>().next_view.lock().unwrap().clone();
        app.emit("navigate", view)?;
    } else {
        app.state::<Host>()
            .retention_sequence
            .store(0, Ordering::Release);
        tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?.build()?;
        // The new frontend reads interface_context after mounting.
    }
    Ok(())
}
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show(app, "connection")
        }))
        .invoke_handler(tauri::generate_handler![
            service_request,
            network_monitor_error,
            interface_context,
            retain_interface
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
                requests: Arc::new(ClientCoordinator::default()),
                restore,
                status,
                stop: Arc::new(AtomicBool::new(false)),
                monitor_error: Arc::new(Mutex::new(None)),
                retention: Arc::new(AtomicU8::new(IDLE)),
                retention_sequence: Arc::new(AtomicU64::new(0)),
                visibility_request: Arc::new(AtomicU64::new(0)),
                diagnostic_pending: Arc::new(AtomicBool::new(false)),
                window_transition: Arc::new(Mutex::new(())),
                destroyed: Arc::new((Mutex::new(0), Condvar::new())),
                next_view: Arc::new(Mutex::new("connection".into())),
                tray_result: Arc::new(Mutex::new(None)),
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
                    "diagnostics" => show(app, "diagnostics"),
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
                                *app.state::<Host>().tray_result.lock().unwrap() =
                                    Some(result.clone());
                                let _ = app.emit("tray-result", result);
                            } else {
                                *app.state::<Host>().tray_result.lock().unwrap() =
                                    Some(status.clone());
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
                // Keep uncertain mutation receipts and in-flight UI actions alive.
                // An idle window is rebuilt from embedded assets on the next open.
                hide_interface(window.clone());
            }
        })
        .build(tauri::generate_context!())
        .expect("Could not start Quad9 Connect")
        .run(|app, event| {
            if matches!(&event, tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } if label == "main") {
                let host = app.state::<Host>();
                let (generation, changed) = &*host.destroyed;
                if let Ok(mut value) = generation.lock() { *value += 1; changed.notify_all(); };
            }
            if let tauri::RunEvent::ExitRequested {
                api, code: None, ..
            } = &event
            {
                api.prevent_exit();
            }
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<Host>().stop.store(true, Ordering::Release);
            }
        });
}
