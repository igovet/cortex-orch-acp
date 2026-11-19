use crate::phone::Phone;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{plugin::PluginHandle, AppHandle, Emitter, Manager, State, WebviewWindow, Wry};
use tokio::sync::OnceCell;

/// The phone is opened on first use, inside the async runtime, and then kept for the app's life.
#[derive(Default)]
struct Native(OnceCell<Arc<Phone>>);

/// Android reports the model through its property service; a phone without it still has a name.
fn device_name() -> String {
    std::process::Command::new("/system/bin/getprop")
        .arg("ro.product.model")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Android phone".into())
}

async fn phone(app: &AppHandle, native: &State<'_, Native>) -> Result<Arc<Phone>, String> {
    native
        .0
        .get_or_try_init(|| async {
            let folder = app
                .path()
                .app_data_dir()
                .map_err(|_| "phone_state_unavailable".to_string())?;
            std::fs::create_dir_all(&folder).map_err(|_| "phone_state_unavailable".to_string())?;
            let emitter = app.clone();
            Phone::open(
                &folder,
                &device_name(),
                Arc::new(move |event, payload| {
                    let _ = emitter.emit(event, payload);
                }),
            )
        })
        .await
        .cloned()
}

/// Only the bundled page of the main view may use the bridge.
fn bundled(window: &WebviewWindow) -> Result<(), String> {
    let url = window.url().map_err(|_| "bridge_origin_refused")?;
    let local = matches!(url.scheme(), "http" | "https" | "tauri")
        && matches!(url.host_str(), Some("tauri.localhost" | "localhost"))
        && url.port().is_none();
    if window.label() != "main" || !local {
        return Err("bridge_origin_refused".into());
    }
    Ok(())
}

#[tauri::command]
async fn workspace_request(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
    machine_id: String,
    operation: String,
    message: Value,
) -> Result<Value, String> {
    bundled(&window)?;
    phone(&app, &native)
        .await?
        .request(&machine_id, operation, message)
        .await
}

#[tauri::command]
async fn phone_info(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
) -> Result<Value, String> {
    bundled(&window)?;
    let phone = phone(&app, &native).await?;
    Ok(json!({"version": env!("CARGO_PKG_VERSION"), "deviceName": phone.device_name()}))
}

#[tauri::command]
async fn phone_state(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
) -> Result<Value, String> {
    bundled(&window)?;
    Ok(phone(&app, &native).await?.state())
}

#[tauri::command]
async fn phone_pair_begin(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
    link: String,
) -> Result<Value, String> {
    bundled(&window)?;
    phone(&app, &native).await?.pair_begin(&link).await
}

#[tauri::command]
async fn phone_pair_finish(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
    name: String,
) -> Result<Value, String> {
    bundled(&window)?;
    phone(&app, &native).await?.pair_finish(&name).await
}

#[tauri::command]
async fn phone_pair_cancel(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
) -> Result<(), String> {
    bundled(&window)?;
    phone(&app, &native).await?.pair_cancel().await;
    Ok(())
}

#[tauri::command]
async fn phone_select(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
    machine_id: String,
) -> Result<(), String> {
    bundled(&window)?;
    phone(&app, &native).await?.select(&machine_id)
}

#[tauri::command]
async fn phone_rename(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
    machine_id: String,
    name: String,
) -> Result<(), String> {
    bundled(&window)?;
    phone(&app, &native).await?.rename(&machine_id, &name)
}

#[tauri::command]
async fn phone_forget(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
    machine_id: String,
) -> Result<(), String> {
    bundled(&window)?;
    phone(&app, &native).await?.forget(&machine_id).await
}

#[tauri::command]
async fn phone_retry(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
    machine_id: String,
) -> Result<(), String> {
    bundled(&window)?;
    phone(&app, &native).await?.retry(&machine_id)
}

/// The phone's own speech recognition, reached through the native plugin.
struct Speech(PluginHandle<Wry>);

/// Opens the system's recognizer and returns what it heard: an empty text when the owner backed out
/// or nothing was recognised. The app itself records nothing.
#[tauri::command]
async fn phone_listen(window: WebviewWindow, speech: State<'_, Speech>) -> Result<Value, String> {
    bundled(&window)?;
    let heard: Value = speech
        .0
        .run_mobile_plugin_async("listen", ())
        .await
        .map_err(|_| "speech_unavailable".to_string())?;
    let text = heard["text"].as_str().unwrap_or_default();
    // Bounded like any text the interface accepts from outside.
    Ok(json!({"text": text.chars().take(16_000).collect::<String>()}))
}

#[tauri::command]
fn phone_exit(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    bundled(&window)?;
    app.exit(0);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_barcode_scanner::init())
        .plugin(
            tauri::plugin::Builder::<Wry>::new("speech")
                .setup(|app, api| {
                    let handle =
                        api.register_android_plugin("dev.cortex.workspace.speech", "SpeechPlugin")?;
                    app.manage(Speech(handle));
                    Ok(())
                })
                .build(),
        )
        .manage(Native::default())
        .invoke_handler(tauri::generate_handler![
            workspace_request,
            phone_info,
            phone_state,
            phone_pair_begin,
            phone_pair_finish,
            phone_pair_cancel,
            phone_select,
            phone_rename,
            phone_forget,
            phone_retry,
            phone_listen,
            phone_exit
        ])
        .run(tauri::generate_context!())
        .expect("Cortex Workspace phone shell failed");
}
