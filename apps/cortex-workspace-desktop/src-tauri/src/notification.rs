//! Linux desktop notifications, using the session bus, never a shell command.
use gtk::{gio, glib};
use glib::variant::ToVariant;

fn parameters(label: &str) -> glib::Variant {
    let label: String = label.chars().filter(|c| !c.is_control()).take(200).collect();
    // Notification bodies can be markup. Titles from a project are plain text.
    let body = glib::markup_escape_text(&label);
    ("Cortex Workspace", 0u32, "dialog-question", "Question waiting",
     body.as_str(), Vec::<String>::new(),
     std::collections::HashMap::<String, glib::Variant>::new(), -1i32).to_variant()
}

pub(crate) fn send(label: &str) -> Result<(), String> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|_| "Desktop notification service unavailable.")?;
    let reply = bus.call_sync(Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications", "org.freedesktop.Notifications", "Notify",
        Some(&parameters(label)), None, gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE)
        .map_err(|_| "Desktop notification delivery failed.")?;
    if reply.get::<(u32,)>().is_none() { return Err("Desktop notification reply invalid.".into()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "run only in a disposable container under dbus-run-session"]
    fn delivery_to_private_bus() {
        assert_eq!(std::env::var("CORTEX_NOTIFICATION_TEST_BUS").as_deref(), Ok("private-container"));
        let (ready, wait) = std::sync::mpsc::channel();
        let (observed, received) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let context = glib::MainContext::new();
            context.with_thread_default(|| {
                let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).unwrap();
                bus.call_sync(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
                    "org.freedesktop.DBus", "RequestName",
                    Some(&("org.freedesktop.Notifications", 0u32).to_variant()), None,
                    gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE).unwrap();
                let info = gio::DBusNodeInfo::for_xml(r#"<node><interface name="org.freedesktop.Notifications"><method name="Notify"><arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/><arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/></method></interface></node>"#).unwrap();
                let event_loop = glib::MainLoop::new(Some(&context), false);
                let stop = event_loop.clone();
                let registration = bus.register_object("/org/freedesktop/Notifications",
                    &info.lookup_interface("org.freedesktop.Notifications").unwrap(),
                    move |_, _, _, _, method, params, invocation| {
                        assert_eq!(method, "Notify");
                        observed.send(params.child_value(4).get::<String>().unwrap()).unwrap();
                        invocation.return_value(Some(&(1u32,).to_variant()));
                        stop.quit();
                    }, |_, _, _, _, _| unreachable!(), |_, _, _, _, _, _| false).unwrap();
                ready.send(()).unwrap();
                event_loop.run();
                bus.unregister_object(registration).unwrap();
                bus.call_sync(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus",
                    "org.freedesktop.DBus", "ReleaseName",
                    Some(&("org.freedesktop.Notifications",).to_variant()), None,
                    gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE).unwrap();
            }).unwrap();
        });
        wait.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        send("Fixture question").unwrap();
        assert_eq!(received.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), "Fixture question");
        server.join().unwrap();
        assert!(send("Service unavailable").is_err());
    }

    #[test]
    fn bounded_plain_text_notification_has_the_native_signature() {
        let params = parameters("<task>\n & description");
        assert_eq!(params.type_().as_str(), "(susssasa{sv}i)");
        assert_eq!(params.child_value(4).get::<String>().unwrap(), "&lt;task&gt; &amp; description");
        assert_eq!(parameters(&"x".repeat(500)).child_value(4).get::<String>().unwrap().len(), 200);
    }
}
