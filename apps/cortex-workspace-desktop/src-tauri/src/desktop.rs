use crate::{client::Bridge, policy};
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
    bridge: Mutex<Option<Arc<Bridge>>>,
    drop: Arc<std::sync::Mutex<Option<(std::time::Instant, Result<Value, String>)>>>,
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
    if call.operation == "native.take_dropped_files" {
        let value = state.drop.lock().map_err(|_| "Drop is unavailable.")?.take();
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
        window.run_on_main_thread(move || {
            let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
            clipboard.request_contents(&gtk::gdk::Atom::intern("image/png"), move |clipboard, selection| {
                let bytes = selection.data();
                if !bytes.is_empty() {
                    let _ = send.send(crate::attachments::clipboard_png(&bytes));
                    return;
                }
                clipboard.request_image(move |_, image| {
                    let result = match image.map(|image| image.save_to_bufferv("png", &[])) {
                        None => Ok(serde_json::json!([])),
                        Some(Ok(bytes)) => crate::attachments::clipboard_png(&bytes),
                        Some(Err(_)) => Err("This clipboard image could not be read."),
                    };
                    let _ = send.send(result);
                });
            });
        }).map_err(|_| "Clipboard is unavailable.")?;
        return tokio::time::timeout(std::time::Duration::from_secs(5), receive).await
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
    window.run_on_main_thread(move || {
        use gtk::prelude::{FileChooserExt, NativeDialogExt};
        let parent = owner.gtk_window().ok();
        let dialog = gtk::FileChooserNative::new(Some("Choose a project folder"), parent.as_ref(), gtk::FileChooserAction::SelectFolder, Some("Choose"), Some("Cancel"));
        dialog.set_create_folders(true);
        dialog.set_modal(true);
        // Otherwise it opens on Recent, where Choose with nothing selected does nothing at all.
        if let Some(home) = std::env::var_os("HOME") {
            dialog.set_current_folder(home);
        }
        // GTK releases a native dialog with its last reference, so the response holds one until it runs.
        let held = std::rc::Rc::new(std::cell::RefCell::new(Some((dialog.clone(), send))));
        dialog.connect_response(move |chooser, response| {
            let chosen = (response == gtk::ResponseType::Accept).then(|| chooser.filename()).flatten();
            chooser.hide();
            if let Some((_, send)) = held.borrow_mut().take() {
                let _ = send.send(chosen);
            }
        });
        dialog.show();
    }).map_err(|_| "The folder chooser is unavailable.")?;
    let chosen = receive.await.map_err(|_| "The folder chooser closed unexpectedly.")?;
    Ok(serde_json::json!({"path":chosen.map(|path: std::path::PathBuf| path.to_string_lossy().into_owned())}))
}

/// Writes one saved attachment to the file the owner picks in the system's save dialog. The webview only
/// supplies the name and bytes, never a path; policy has already bounded both. Null means the owner cancelled.
async fn save_attachment(window: &WebviewWindow, message: Value) -> Result<Value, String> {
    use base64::Engine;
    let name = message["name"].as_str().ok_or("This file name is unavailable.")?.to_owned();
    let bytes = base64::engine::general_purpose::STANDARD.decode(message["data"].as_str().ok_or("This attachment could not be read.")?)
        .map_err(|_| "This attachment could not be read.")?;
    let (send, receive) = tokio::sync::oneshot::channel();
    let owner = window.clone();
    window.run_on_main_thread(move || {
        use gtk::prelude::{FileChooserExt, NativeDialogExt};
        let parent = owner.gtk_window().ok();
        let dialog = gtk::FileChooserNative::new(Some("Save a copy"), parent.as_ref(), gtk::FileChooserAction::Save, Some("Save"), Some("Cancel"));
        dialog.set_do_overwrite_confirmation(true);
        dialog.set_modal(true);
        dialog.set_current_name(&name);
        if let Some(home) = std::env::var_os("HOME") {
            dialog.set_current_folder(home);
        }
        let held = std::rc::Rc::new(std::cell::RefCell::new(Some((dialog.clone(), send))));
        dialog.connect_response(move |chooser, response| {
            let chosen = (response == gtk::ResponseType::Accept).then(|| chooser.filename()).flatten();
            chooser.hide();
            if let Some((_, send)) = held.borrow_mut().take() {
                let _ = send.send(chosen);
            }
        });
        dialog.show();
    }).map_err(|_| "The save dialog is unavailable.")?;
    let Some(path) = receive.await.map_err(|_| "The save dialog closed unexpectedly.")? else {
        return Ok(serde_json::json!({"path":null}));
    };
    std::fs::write(&path, bytes).map_err(|_| "The file could not be saved there.")?;
    Ok(serde_json::json!({"path":path.to_string_lossy()}))
}

#[tauri::command]
async fn workspace_notify(window: WebviewWindow, label: String) -> Result<(), String> {
    if window.label() != "main" || !policy::local_url(window.url().map_err(|_| "bridge_origin_refused")?.as_str()) {
        return Err("bridge_origin_refused".into());
    }
    if label.chars().count() > 1000 { return Err("notification_label_too_long".into()); }
    tauri::async_runtime::spawn_blocking(move || crate::notification::send(&label))
        .await.map_err(|_| "Desktop notification delivery failed.")?
}

/// Called only after the frontend unsaved guard and pending request barrier.
#[tauri::command]
fn workspace_close(window: WebviewWindow) -> Result<(), String> {
    if window.label() != "main" || !policy::local_url(window.url().map_err(|_| "bridge_origin_refused")?.as_str()) {
        return Err("bridge_origin_refused".into());
    }
    let owner = window.clone();
    window.with_webview(move |webview| {
        crate::rendering::close(&webview.inner(), move || { let _ = owner.destroy(); });
    }).map_err(|_| "Window close is unavailable.".into())
}

pub fn run() {
    tauri::Builder::default()
        .on_window_event(|window, event| {
            // Native/WM close must not bypass the frontend guard or WebKit teardown.
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event { api.prevent_close(); }
            }
        })
        .invoke_handler(tauri::generate_handler![workspace_request, workspace_notify, workspace_close])
        .setup(|app| {
            app.manage(Native::default());
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
                    .on_new_window(|_, _| NewWindowResponse::Deny)
                    .build()?;
            let dropped = app.state::<Native>().drop.clone();
            window.with_webview(move |webview| {
                use gtk::prelude::WidgetExt;
                webview.inner().connect_drag_data_received(move |_, _, _, _, selection, _, _| {
                    let uris = selection.uris();
                    if !uris.is_empty() {
                        let files = crate::attachments::snapshot(&uris);
                        if let Ok(mut slot) = dropped.lock() { *slot = Some((std::time::Instant::now(), files)); }
                    }
                });
            })?;
            window.set_background_color(Some(match window.theme() {
                Ok(Theme::Dark) => SURFACE_DARK,
                _ => SURFACE_LIGHT,
            }))?;
            window.show()?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    if let Ok(bridge) = handle.state::<Native>().bridge().await {
                        bridge
                            .subscribe(|event| {
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
