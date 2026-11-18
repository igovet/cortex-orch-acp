//! Linux desktop notifications, using the session bus, never a shell command.
//!
//! Each notification names the installed desktop entry and offers the default action, so a click
//! on it reaches this process as `ActionInvoked(id, "default")`, preceded by `ActivationToken(id,
//! token)` where the server sends one. The server delivers both only to the connection that sent the
//! notification, so that one connection is held for the life of the process and listened to.
use crate::navigation::{NotificationTarget, Opened, Outstanding};
use glib::variant::ToVariant;
use gtk::{gio, glib};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

pub(crate) type Shared = Arc<Mutex<Outstanding>>;

static LISTENING: AtomicBool = AtomicBool::new(false);

fn parameters(label: &str, replaces: u32, target: &NotificationTarget) -> glib::Variant {
    let label: String = label
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    // Notification bodies can be markup. Titles from a project are plain text.
    let body = glib::markup_escape_text(&label);
    let channel = workspace_protocol::channel::current();
    let summary = match target {
        NotificationTarget::Question { .. } => "Question waiting",
        NotificationTarget::Planning { .. } => "Planning question waiting",
        // The window raises questions only; the other kinds come from the service.
        NotificationTarget::Result { .. } => "Result ready for review",
        NotificationTarget::Task { .. } => "Task failed",
        NotificationTarget::Sprint { .. } => "Sprint paused",
    };
    // The installed entry is `<package>.desktop`: the server finds the app, its icon and its
    // activation token by it, and withdraws the notification when this process leaves the bus.
    let mut hints = std::collections::HashMap::<String, glib::Variant>::new();
    hints.insert("desktop-entry".into(), channel.package.to_variant());
    (
        channel.title.as_str(),
        replaces,
        "dialog-question",
        summary,
        body.as_str(),
        vec!["default".to_owned(), "Open".to_owned()],
        hints,
        -1i32,
    )
        .to_variant()
}

/// The process's own session-bus connection. GIO keeps its shared connection only while someone
/// holds it, and a notification's click is sent to the connection that raised it, so it is held here.
fn bus() -> Result<&'static gio::DBusConnection, String> {
    static BUS: OnceLock<gio::DBusConnection> = OnceLock::new();
    if let Some(bus) = BUS.get() {
        return Ok(bus);
    }
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|_| "Desktop notification service unavailable.")?;
    Ok(BUS.get_or_init(|| bus))
}

/// Listens, once, for clicks on this process's notifications. `opened` runs on the thread whose
/// main context was current when this was called (the window's, at setup).
pub(crate) fn listen(outstanding: Shared, opened: impl Fn(Opened) + 'static) -> Result<(), String> {
    let bus = bus()?;
    if LISTENING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    bus.signal_subscribe(
        Some(NAME),
        Some(NAME),
        None,
        Some(PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |_, _, _, _, signal, params| {
            let Ok(mut held) = outstanding.lock() else {
                return;
            };
            match signal {
                "ActivationToken" => {
                    if let Some((id, token)) = params.get::<(u32, String)>() {
                        held.token(id, &token);
                    }
                }
                "ActionInvoked" => {
                    if let Some(click) = params
                        .get::<(u32, String)>()
                        .and_then(|(id, action)| held.invoked(id, &action))
                    {
                        drop(held);
                        opened(click);
                    }
                }
                "NotificationClosed" => {
                    if let Some((id, _)) = params.get::<(u32, u32)>() {
                        held.closed(id);
                    }
                }
                _ => {}
            }
        },
    );
    Ok(())
}

/// Raises one notification and remembers where its click leads. Refused while nothing listens for
/// that click, so a notification never offers an action that would do nothing.
pub(crate) fn send(
    outstanding: &Shared,
    label: &str,
    target: NotificationTarget,
) -> Result<(), String> {
    if !LISTENING.load(Ordering::SeqCst) {
        return Err("Desktop notification service unavailable.".into());
    }
    let bus = bus()?;
    let replaces = outstanding
        .lock()
        .map_err(|_| "Desktop notification delivery failed.")?
        .replaces(&target);
    let reply = bus
        .call_sync(
            Some(NAME),
            PATH,
            NAME,
            "Notify",
            Some(&parameters(label, replaces, &target)),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        )
        .map_err(|_| "Desktop notification delivery failed.")?;
    let Some((id,)) = reply.get::<(u32,)>() else {
        return Err("Desktop notification reply invalid.".into());
    };
    outstanding
        .lock()
        .map_err(|_| "Desktop notification delivery failed.")?
        .sent(id, target);
    Ok(())
}

/// Withdraws this window's notifications before it closes, so none is left offering a click that
/// no process would receive. Sent without waiting for replies; the connection is flushed once.
pub(crate) fn withdraw(outstanding: &Shared) {
    let ids = match outstanding.lock() {
        Ok(mut held) => held.drain(),
        Err(_) => return,
    };
    if ids.is_empty() {
        return;
    }
    let Ok(bus) = bus() else { return };
    for id in ids {
        let message =
            gio::DBusMessage::new_method_call(Some(NAME), PATH, Some(NAME), "CloseNotification");
        message.set_body(&(id,).to_variant());
        message.set_flags(
            gio::DBusMessageFlags::NO_REPLY_EXPECTED | gio::DBusMessageFlags::NO_AUTO_START,
        );
        let _ = bus.send_message(&message, gio::DBusSendMessageFlags::NONE);
    }
    let _ = bus.flush_sync(gio::Cancellable::NONE);
}

#[link(name = "gdk-3")]
extern "C" {
    // GTK 3's public Wayland API: the token is used, and consumed, by the next focus request,
    // which GTK makes through xdg-activation.
    fn gdk_wayland_display_set_startup_notification_id(
        display: *mut gtk::gdk::ffi::GdkDisplay,
        startup_id: *const std::ffi::c_char,
    );
}

/// Brings the window forward for a click, on GTK's main thread. Under Wayland focus-stealing
/// prevention only the server's activation token lets it take focus; without one the compositor
/// may only mark the window as wanting attention.
pub(crate) fn raise(window: &gtk::ApplicationWindow, token: Option<&str>) {
    use glib::prelude::{ObjectExt, ObjectType};
    use gtk::prelude::{GtkWindowExt, WidgetExt};
    if let Some(token) = token.and_then(|t| std::ffi::CString::new(t).ok()) {
        let display = WidgetExt::display(window);
        if display.type_().name() == "GdkWaylandDisplay" {
            unsafe {
                gdk_wayland_display_set_startup_notification_id(display.as_ptr(), token.as_ptr())
            };
        } else if let Ok(id) = token.to_str() {
            // X11: the token is a startup-notification id carrying the click's timestamp.
            window.set_startup_id(id);
        }
    }
    window.deiconify();
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop_entry(hints: &glib::Variant) -> Option<String> {
        hints
            .get::<std::collections::HashMap<String, glib::Variant>>()?
            .get("desktop-entry")?
            .get::<String>()
    }

    fn target() -> NotificationTarget {
        NotificationTarget::Question {
            project: "p".into(),
            task: "t".into(),
            decision: "d".into(),
        }
    }

    /// The private bus's stand-in for the desktop's notification server: it answers `Notify` with
    /// id 41, then reports a click on it, unicast to the sender, as GNOME Shell does.
    #[test]
    #[ignore = "run only in a disposable container under dbus-run-session"]
    fn delivery_and_click_on_a_private_bus() {
        assert_eq!(
            std::env::var("CORTEX_NOTIFICATION_TEST_BUS").as_deref(),
            Ok("private-container")
        );
        let (ready, wait) = std::sync::mpsc::channel();
        let (observed, received) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let context = glib::MainContext::new();
            context.with_thread_default(|| {
                // Its own connection, so its signals reach the client's as from another peer.
                let address = gio::dbus_address_get_for_bus_sync(gio::BusType::Session, gio::Cancellable::NONE).unwrap();
                let bus = gio::DBusConnection::for_address_sync(&address,
                    gio::DBusConnectionFlags::AUTHENTICATION_CLIENT | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
                    None, gio::Cancellable::NONE).unwrap();
                bus.call_sync(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
                    "org.freedesktop.DBus", "RequestName",
                    Some(&(NAME, 0u32).to_variant()), None,
                    gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE).unwrap();
                let info = gio::DBusNodeInfo::for_xml(r#"<node><interface name="org.freedesktop.Notifications"><method name="Notify"><arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/><arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/></method><method name="CloseNotification"><arg type="u" direction="in"/></method><signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal><signal name="ActivationToken"><arg type="u"/><arg type="s"/></signal></interface></node>"#).unwrap();
                let event_loop = glib::MainLoop::new(Some(&context), false);
                let stop = event_loop.clone();
                let registration = bus.register_object(PATH,
                    &info.lookup_interface(NAME).unwrap(),
                    move |connection, sender, _, _, method, params, invocation| {
                        assert_eq!(method, "Notify");
                        let actions = params.child_value(5).get::<Vec<String>>().unwrap();
                        let hints = params.child_value(6);
                        let entry = desktop_entry(&hints);
                        observed.send((params.child_value(4).get::<String>().unwrap(), actions, entry)).unwrap();
                        invocation.return_value(Some(&(41u32,).to_variant()));
                        connection.emit_signal(Some(sender), PATH, NAME, "ActivationToken", Some(&(41u32, "fixture-token").to_variant())).unwrap();
                        connection.emit_signal(Some(sender), PATH, NAME, "ActionInvoked", Some(&(40u32, "default").to_variant())).unwrap();
                        connection.emit_signal(Some(sender), PATH, NAME, "ActionInvoked", Some(&(41u32, "default").to_variant())).unwrap();
                        connection.flush_sync(gio::Cancellable::NONE).unwrap();
                        stop.quit();
                    }, |_, _, _, _, _| unreachable!(), |_, _, _, _, _, _| false).unwrap();
                ready.send(()).unwrap();
                event_loop.run();
                bus.unregister_object(registration).unwrap();
                bus.close_sync(gio::Cancellable::NONE).unwrap();
            }).unwrap();
        });
        wait.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let outstanding: Shared = Default::default();
        assert!(send(&outstanding, "Before listening", target()).is_err());
        let clicks = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = clicks.clone();
        listen(outstanding.clone(), move |opened| {
            seen.borrow_mut().push(opened)
        })
        .unwrap();
        send(&outstanding, "Fixture question", target()).unwrap();
        let (body, actions, entry) = received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(body, "Fixture question");
        assert_eq!(actions, ["default", "Open"]);
        assert_eq!(
            entry.as_deref(),
            Some(workspace_protocol::channel::current().package.as_str())
        );
        server.join().unwrap();
        // The signals are dispatched by the main context that was current when listening began.
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while clicks.borrow().is_empty() && std::time::Instant::now() < deadline {
            context.iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            *clicks.borrow(),
            vec![Opened {
                target: target(),
                token: Some("fixture-token".into())
            }]
        );
        assert!(outstanding.lock().unwrap().is_empty());
        assert!(send(&outstanding, "Service unavailable", target()).is_err());
    }

    #[test]
    fn bounded_plain_text_notification_has_the_native_signature() {
        let params = parameters("<task>\n & description", 0, &target());
        assert_eq!(params.type_().as_str(), "(susssasa{sv}i)");
        assert_eq!(
            params.child_value(4).get::<String>().unwrap(),
            "&lt;task&gt; &amp; description"
        );
        assert_eq!(
            params.child_value(5).get::<Vec<String>>().unwrap(),
            ["default", "Open"]
        );
        assert_eq!(
            desktop_entry(&params.child_value(6)).as_deref(),
            Some(workspace_protocol::channel::current().package.as_str())
        );
        assert_eq!(
            parameters("x", 9, &target()).child_value(1).get::<u32>(),
            Some(9)
        );
        assert_eq!(
            parameters(&"x".repeat(500), 0, &target())
                .child_value(4)
                .get::<String>()
                .unwrap()
                .len(),
            200
        );
    }
}
