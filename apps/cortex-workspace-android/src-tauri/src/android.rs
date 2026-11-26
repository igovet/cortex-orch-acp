use crate::{moments::Delivery, phone::Phone};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
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

/// This phone's own notifications, reached through the native plugin (`NotifyService.kt`): the
/// foreground service that keeps the connection while the app is in the background, the
/// notifications it raises for owner moments, and the taps on them.
struct Notify(Arc<PluginHandle<Wry>>);

/// Hands each raise or withdrawal to the plugin. A plugin that does not answer in time loses
/// that one notification, never the subscription.
fn deliver(handle: Arc<PluginHandle<Wry>>) -> crate::phone::Deliver {
    Arc::new(move |delivery| {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            let (command, payload) = match delivery {
                Delivery::Show {
                    tag,
                    channel,
                    title,
                    text,
                    machine_id,
                    target,
                } => (
                    "show",
                    json!({"tag":tag,"channel":channel,"title":title,"text":text,
                        "machineId":machine_id,"target":target.to_string()}),
                ),
                Delivery::Withdraw { tag } => ("withdraw", json!({"tag":tag})),
            };
            let _ = tokio::time::timeout(
                Duration::from_secs(10),
                handle.run_mobile_plugin_async::<Value>(command, payload),
            )
            .await;
        });
    })
}

/// Turns this phone's notifications on (the foreground service starts and every paired
/// machine's owner moments are followed) or off. On asks Android for the notification
/// permission when it is not granted yet. `words` are the texts Android shows, in the
/// interface's language; each is checked (`moments::words`) and English stays where one is not.
#[tauri::command]
async fn phone_notifications(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
    notify: State<'_, Notify>,
    enabled: bool,
    words: Value,
) -> Result<Value, String> {
    bundled(&window)?;
    let phone = phone(&app, &native).await?;
    let words = crate::moments::words(&words);
    let reply: Value = notify
        .0
        .run_mobile_plugin_async(if enabled { "start" } else { "stop" }, words.android)
        .await
        .map_err(|_| "notifications_unavailable".to_string())?;
    phone.set_notifications(enabled.then(|| deliver(notify.0.clone())), words.titles);
    let permission = match reply["permission"].as_str() {
        Some("granted") => "granted",
        _ => "denied",
    };
    Ok(
        json!({"enabled":enabled,"permission":permission,"batteryExempt":reply["batteryExempt"] == true}),
    )
}

/// The next tap on one of this phone's notifications. The plugin answers when one arrives;
/// what it hands back is checked again here (a paired machine, a target of the closed
/// shapes), since any app can start this activity with extras of its own. `null` leads nowhere.
#[tauri::command]
async fn phone_notification_next(
    window: WebviewWindow,
    app: AppHandle,
    native: State<'_, Native>,
    notify: State<'_, Notify>,
) -> Result<Value, String> {
    bundled(&window)?;
    let phone = phone(&app, &native).await?;
    let tap: Value = notify
        .0
        .run_mobile_plugin_async("next", ())
        .await
        .map_err(|_| "notifications_unavailable".to_string())?;
    let machine = tap["machineId"].as_str().unwrap_or_default();
    let target = tap["target"]
        .as_str()
        .filter(|t| t.len() <= 2048)
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
        .unwrap_or(Value::Null);
    Ok(phone
        .notification_tap(machine, &target)
        .unwrap_or(Value::Null))
}

/// Opens Android's battery optimisation list, where the owner can exempt the app so the
/// connection is kept while the phone sleeps.
#[tauri::command]
async fn phone_battery_settings(
    window: WebviewWindow,
    notify: State<'_, Notify>,
) -> Result<(), String> {
    bundled(&window)?;
    notify
        .0
        .run_mobile_plugin_async::<Value>("battery", ())
        .await
        .map(|_| ())
        .map_err(|_| "notifications_unavailable".to_string())
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
        .plugin(
            tauri::plugin::Builder::<Wry>::new("notify")
                .setup(|app, api| {
                    let handle =
                        api.register_android_plugin("dev.cortex.workspace.notify", "NotifyPlugin")?;
                    app.manage(Notify(Arc::new(handle)));
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
            phone_notifications,
            phone_notification_next,
            phone_battery_settings,
            phone_exit
        ])
        .run(tauri::generate_context!())
        .expect("Cortex Workspace phone shell failed");
}
