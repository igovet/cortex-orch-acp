use crate::{client::Bridge, policy, stall};
use serde_json::Value;
use std::sync::Arc;
use tauri::{
    ipc::{InvokeBody, Request},
    webview::NewWindowResponse,
    window::Color,
    Emitter, Manager, State, Theme, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tokio::sync::Mutex;

// --bg-canvas of the accepted stylesheet, painted before the page exists so a launch never flashes another colour.
const SURFACE_DARK: Color = Color(0x18, 0x1b, 0x1a, 0xff);
const SURFACE_LIGHT: Color = Color(0xff, 0xff, 0xff, 0xff);

#[derive(Default)]
struct Native {
    history: crate::history::History,
    extracted: bool,
    bridge: Mutex<Option<Arc<Bridge>>>,
    drop: Arc<std::sync::Mutex<Option<(std::time::Instant, Result<Value, String>)>>>,
    traffic: Arc<stall::Traffic>,
    /// The notifications this window raised that can still be clicked, and where each leads.
    notes: crate::notification::Shared,
}
impl Native {
    async fn bridge(&self) -> Result<Arc<Bridge>, String> {
        let mut current = self.bridge.lock().await;
        if current.is_none() {
            *current = Some(Arc::new(Bridge::for_user()?));
        }
        Ok(current.as_ref().unwrap().clone())
    }
}

#[tauri::command]
fn workspace_history_open(
    window: WebviewWindow,
    state: State<'_, Native>,
    id: String,
    params: Value,
) -> Result<(), String> {
    history_origin(&window)?;
    state.history.open(id, params)
}

#[tauri::command]
async fn workspace_history_next(
    window: WebviewWindow,
    state: State<'_, Native>,
    id: String,
) -> Result<Value, String> {
    history_origin(&window)?;
    let bridge = state.bridge().await?;
    state.history.next(&bridge, &id).await
}

fn history_origin(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main"
        && policy::local_url(window.url().map_err(|_| "bridge_origin_refused")?.as_str())
    {
        Ok(())
    } else {
        Err("bridge_origin_refused".into())
    }
}

#[tauri::command]
fn workspace_history_close(
    window: WebviewWindow,
    state: State<'_, Native>,
    id: String,
) -> Result<(), String> {
    history_origin(&window)?;
    state.history.close(&id);
    Ok(())
}

#[tauri::command]
async fn workspace_request(
    window: WebviewWindow,
    state: State<'_, Native>,
    request: Request<'_>,
) -> Result<Value, String> {
    let url = window.url().map_err(|_| "bridge_origin_refused")?;
    let body = match request.body() {
        InvokeBody::Json(body) => body.clone(),
        _ => return Err("bridge_request_refused".into()),
    };
    let call = policy::authorize(window.label(), url.as_str(), body)?;
    state.traffic.request(&call.operation, &call.message);
    if call.operation == "native.take_dropped_files" {
        let value = state
            .drop
            .lock()
            .map_err(|_| "Drop is unavailable.")?
            .take();
        return match value {
            Some((at, files)) if at.elapsed().as_secs() < 5 => files,
            _ => Ok(serde_json::json!([])),
        };
    }
    if call.operation == "native.save_attachment" {
        return save_attachment(&window, call.message).await;
    }
    if call.operation == "native.clipboard_image" {
        // WebKitGTK does not expose a clipboard image to the page. Ask for PNG, then
        // any other image GTK can decode, on GTK's main thread; never read text or files.
        let (send, receive) = tokio::sync::oneshot::channel();
        window
            .run_on_main_thread(move || {
                let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
                clipboard.request_contents(
                    &gtk::gdk::Atom::intern("image/png"),
                    move |clipboard, selection| {
                        let bytes = selection.data();
                        if !bytes.is_empty() {
                            let _ = send.send(crate::attachments::clipboard_png(&bytes));
                            return;
                        }
                        clipboard.request_image(move |_, image| {
                            let result = match image.map(|image| image.save_to_bufferv("png", &[]))
                            {
                                None => Ok(serde_json::json!([])),
                                Some(Ok(bytes)) => crate::attachments::clipboard_png(&bytes),
                                Some(Err(_)) => Err("This clipboard image could not be read."),
                            };
                            let _ = send.send(result);
                        });
                    },
                );
            })
            .map_err(|_| "Clipboard is unavailable.")?;
        return tokio::time::timeout(std::time::Duration::from_secs(5), receive)
            .await
            .map_err(|_| "Clipboard did not respond.")?
            .map_err(|_| "Clipboard is unavailable.")?
            .map_err(str::to_owned);
    }
    if call.operation == "native.pick_folder" {
        return pick_folder(&window).await;
    }
    let bridge = state.bridge().await?;
    bridge.request(call).await
}

/// The system's folder chooser, on GTK's main thread. The answer is only ever a path the owner
/// chose themselves; the service still decides whether it can hold a project.
async fn pick_folder(window: &WebviewWindow) -> Result<Value, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let owner = window.clone();
    window
        .run_on_main_thread(move || {
            use gtk::prelude::{FileChooserExt, NativeDialogExt};
            let parent = owner.gtk_window().ok();
            let dialog = gtk::FileChooserNative::new(
                Some("Choose a project folder"),
                parent.as_ref(),
                gtk::FileChooserAction::SelectFolder,
                Some("Choose"),
                Some("Cancel"),
            );
            dialog.set_create_folders(true);
            dialog.set_modal(true);
            // Otherwise it opens on Recent, where Choose with nothing selected does nothing at all.
            if let Some(home) = std::env::var_os("HOME") {
                dialog.set_current_folder(home);
            }
            // GTK releases a native dialog with its last reference, so the response holds one until it runs.
            let held = std::rc::Rc::new(std::cell::RefCell::new(Some((dialog.clone(), send))));
            dialog.connect_response(move |chooser, response| {
                let chosen = (response == gtk::ResponseType::Accept)
                    .then(|| chooser.filename())
                    .flatten();
                chooser.hide();
                if let Some((_, send)) = held.borrow_mut().take() {
                    let _ = send.send(chosen);
                }
            });
            dialog.show();
        })
        .map_err(|_| "The folder chooser is unavailable.")?;
    let chosen = receive
        .await
        .map_err(|_| "The folder chooser closed unexpectedly.")?;
    Ok(
        serde_json::json!({"path":chosen.map(|path: std::path::PathBuf| path.to_string_lossy().into_owned())}),
    )
}

/// Writes one saved attachment to the file the owner picks in the system's save dialog. The webview only
/// supplies the name and bytes, never a path; policy has already bounded both. Null means the owner cancelled.
async fn save_attachment(window: &WebviewWindow, message: Value) -> Result<Value, String> {
    use base64::Engine;
    let name = message["name"]
        .as_str()
        .ok_or("This file name is unavailable.")?
        .to_owned();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(
            message["data"]
                .as_str()
                .ok_or("This attachment could not be read.")?,
        )
        .map_err(|_| "This attachment could not be read.")?;
    let (send, receive) = tokio::sync::oneshot::channel();
    let owner = window.clone();
    window
        .run_on_main_thread(move || {
            use gtk::prelude::{FileChooserExt, NativeDialogExt};
            let parent = owner.gtk_window().ok();
            let dialog = gtk::FileChooserNative::new(
                Some("Save a copy"),
                parent.as_ref(),
                gtk::FileChooserAction::Save,
                Some("Save"),
                Some("Cancel"),
            );
            dialog.set_do_overwrite_confirmation(true);
            dialog.set_modal(true);
            dialog.set_current_name(&name);
            if let Some(home) = std::env::var_os("HOME") {
                dialog.set_current_folder(home);
            }
            let held = std::rc::Rc::new(std::cell::RefCell::new(Some((dialog.clone(), send))));
            dialog.connect_response(move |chooser, response| {
                let chosen = (response == gtk::ResponseType::Accept)
                    .then(|| chooser.filename())
                    .flatten();
                chooser.hide();
                if let Some((_, send)) = held.borrow_mut().take() {
                    let _ = send.send(chosen);
                }
            });
            dialog.show();
        })
        .map_err(|_| "The save dialog is unavailable.")?;
    let Some(path) = receive
        .await
        .map_err(|_| "The save dialog closed unexpectedly.")?
    else {
        return Ok(serde_json::json!({"path":null}));
    };
    std::fs::write(&path, bytes).map_err(|_| "The file could not be saved there.")?;
    Ok(serde_json::json!({"path":path.to_string_lossy()}))
}

#[tauri::command]
async fn workspace_notify(
    window: WebviewWindow,
    state: State<'_, Native>,
    label: String,
    target: Value,
) -> Result<(), String> {
    if window.label() != "main"
        || !policy::local_url(window.url().map_err(|_| "bridge_origin_refused")?.as_str())
    {
        return Err("bridge_origin_refused".into());
    }
    let target = policy::notification_target(&label, target)?;
    if state.extracted {
        return Err("Desktop notifications are disabled for extracted Dev runs.".into());
    }
    let notes = state.notes.clone();
    tauri::async_runtime::spawn_blocking(move || crate::notification::send(&notes, &label, target))
        .await
        .map_err(|_| "Desktop notification delivery failed.")?
}

/// A click on one of the service's notifications. Its target must have the exact shape the
/// window opens; the window then claims the click from the service, which hands over the
/// server's activation token (the token is spent by the process that raises the window), and
/// raises itself and opens the place. A click another window claimed first does nothing here.
fn claim(window: &WebviewWindow, bridge: Arc<Bridge>, event: &Value) {
    let Ok(target) = policy::notification_target("", event["target"].clone()) else {
        return;
    };
    let Some(nonce) = event["nonce"].as_str().map(str::to_owned) else {
        return;
    };
    let window = window.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(reply) = bridge
            .rpc(
                "notifications.opened",
                serde_json::json!({ "nonce": nonce }),
            )
            .await
        else {
            return;
        };
        if reply["claimed"] != true {
            return;
        }
        let token = reply["token"].as_str().map(str::to_owned);
        let shown = window.clone();
        let _ = window.run_on_main_thread(move || {
            if let Ok(gtk) = shown.gtk_window() {
                crate::notification::raise(&gtk, token.as_deref());
            }
            let _ = shown.emit("workspace-navigate", &target);
        });
    });
}

/// A window that stops answering leaves a line on stderr naming how long and what it was handling,
/// so a freeze is diagnosable afterwards. It never changes what the window does.
fn watch_main_thread(window: WebviewWindow, traffic: Arc<stall::Traffic>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(stall::PROBE_EVERY).await;
            let before = traffic.take();
            let (send, mut answered) = tokio::sync::oneshot::channel();
            let started = std::time::Instant::now();
            if window
                .run_on_main_thread(move || {
                    let _ = send.send(());
                })
                .is_err()
            {
                return;
            }
            if tokio::time::timeout(stall::STALL_AFTER, &mut answered)
                .await
                .is_ok()
            {
                continue;
            }
            eprintln!(
                "{}",
                stall::line("main_thread_stall", started.elapsed(), before)
            );
            if answered.await.is_err() {
                return;
            }
            eprintln!(
                "{}",
                stall::line("main_thread_recovered", started.elapsed(), traffic.take())
            );
        }
    });
}

/// Local window only; the renderer invokes this on an explicit link or file-menu activation.
#[tauri::command]
async fn workspace_open_result_target(
    window: WebviewWindow,
    target: crate::opener::ResultTarget,
) -> Result<(), String> {
    if window.label() != "main"
        || !policy::local_url(window.url().map_err(|_| "bridge_origin_refused")?.as_str())
    {
        return Err("bridge_origin_refused".into());
    }
    crate::opener::open_result_target(target).await
}

/// Called only after the frontend unsaved guard and pending request barrier.
#[tauri::command]
fn workspace_close(window: WebviewWindow, state: State<'_, Native>) -> Result<(), String> {
    if window.label() != "main"
        || !policy::local_url(window.url().map_err(|_| "bridge_origin_refused")?.as_str())
    {
        return Err("bridge_origin_refused".into());
    }
    crate::notification::withdraw(&state.notes);
    let owner = window.clone();
    window
        .with_webview(move |webview| {
            crate::rendering::close(&webview.inner(), move || {
                let _ = owner.destroy();
            });
        })
        .map_err(|_| "Window close is unavailable.".into())
}

pub fn run() {
    // Reject a bad extraction before GTK/WebKit can use the ordinary profile.
    let extracted = match workspace_service::dev_extraction::DevExtraction::from_env() {
        Ok(extraction) => extraction.is_some(),
        Err(_) => {
            eprintln!("cortex-workspace-desktop: dev_extraction_invalid");
            std::process::exit(1);
        }
    };
    // Probe and retain the same native Bridge that later handles both View PR
    // actions. Fail before creating a window if the requested capture cannot write.
    let qualified_bridge =
        if extracted && std::env::var_os("CORTEX_WORKSPACE_QUALIFICATION_OPENER_DIR").is_some() {
            match Bridge::for_user().and_then(|bridge| {
                bridge.probe_qualification_opener()?;
                Ok(Arc::new(bridge))
            }) {
                Ok(bridge) => Some(bridge),
                Err(_) => {
                    eprintln!("cortex-workspace-desktop: qualification_opener_failed");
                    std::process::exit(1);
                }
            }
        } else {
            None
        };
    tauri::Builder::default()
        .on_window_event(|window, event| {
            // Native/WM close must not bypass the frontend guard or WebKit teardown.
            if window.label() == "main" {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    if let Some(state) = window.try_state::<Native>() {
                        state.history.clear();
                    }
                }
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            workspace_request,
            workspace_history_open,
            workspace_history_next,
            workspace_history_close,
            workspace_notify,
            workspace_open_result_target,
            workspace_close
        ])
        .setup(move |app| {
            app.manage(Native {
                extracted,
                bridge: Mutex::new(qualified_bridge),
                ..Native::default()
            });
            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title(&workspace_protocol::channel::current().title)
                    .inner_size(1440.0, 900.0)
                    .min_inner_size(960.0, 540.0)
                    // The title band is the frame: its own buttons, drag region and edge resize.
                    .decorations(false)
                    .visible(false)
                    .disable_drag_drop_handler()
                    .devtools(false)
                    .on_navigation(|url| policy::local_url(url.as_str()))
                    .on_page_load(|window, page| {
                        if matches!(page.event(), tauri::webview::PageLoadEvent::Started) {
                            window.state::<Native>().history.clear();
                        }
                    })
                    .on_new_window(|_, _| NewWindowResponse::Deny)
                    .build()?;
            let dropped = app.state::<Native>().drop.clone();
            window.with_webview(move |webview| {
                use gtk::prelude::WidgetExt;
                webview
                    .inner()
                    .connect_drag_data_received(move |_, _, _, _, selection, _, _| {
                        let uris = selection.uris();
                        if !uris.is_empty() {
                            let files = crate::attachments::snapshot(&uris);
                            if let Ok(mut slot) = dropped.lock() {
                                *slot = Some((std::time::Instant::now(), files));
                            }
                        }
                    });
            })?;
            window.set_background_color(Some(match window.theme() {
                Ok(Theme::Dark) => SURFACE_DARK,
                _ => SURFACE_LIGHT,
            }))?;
            window.show()?;
            let handle = app.handle().clone();
            let traffic = app.state::<Native>().traffic.clone();
            watch_main_thread(window.clone(), traffic.clone());
            // A click on one of this window's notifications raises the window and opens its place.
            // Without a listener the window raises none (see notification::send).
            if !extracted {
                let clicked = window.clone();
                let _ = crate::notification::listen(
                    app.state::<Native>().notes.clone(),
                    move |opened| {
                        let window = clicked.clone();
                        let _ = clicked.run_on_main_thread(move || {
                            if let Ok(gtk) = window.gtk_window() {
                                crate::notification::raise(&gtk, opened.token.as_deref());
                            }
                            let _ = window.emit("workspace-navigate", &opened.target);
                        });
                    },
                );
            }
            tauri::async_runtime::spawn(async move {
                loop {
                    if let Ok(bridge) = handle.state::<Native>().bridge().await {
                        bridge
                            .subscribe(|event| {
                                traffic.event();
                                if event["type"] == "notification_open" {
                                    claim(&window, bridge.clone(), &event);
                                    return;
                                }
                                let _ = window.emit("workspace-transport", event);
                            })
                            .await;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Cortex Workspace window failed");
}
