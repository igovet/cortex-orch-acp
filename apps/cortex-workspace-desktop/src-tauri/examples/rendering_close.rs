// GTK/WebKit view lifetime must run on the process main thread, not libtest's thread.
#[path = "../src/rendering.rs"]
mod rendering;
use gtk::prelude::*;
use std::{cell::Cell, rc::Rc};
use webkit2gtk::{LoadEvent, SettingsExt, WebView, WebViewExt};

fn main() {
    gtk::init().expect("disposable GTK display");
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    let view = WebView::new();
    assert_ne!(WebViewExt::settings(&view).unwrap().hardware_acceleration_policy(), webkit2gtk::HardwareAccelerationPolicy::Never);
    window.add(&view);
    window.show_all();
    let finished = Rc::new(Cell::new(false));
    let seen = finished.clone();
    view.connect_load_changed(move |view, event| {
        if event == LoadEvent::Finished {
            let seen = seen.clone();
            rendering::close(view, move || { seen.set(true); gtk::main_quit(); });
        }
    });
    gtk::glib::timeout_add_local_once(std::time::Duration::from_secs(15), || { panic!("native close deadline"); });
    view.load_html("<p>Disposable close regression</p>", None);
    gtk::main();
    assert!(finished.get(), "TerminatedByApi was acknowledged");
    window.close();
    while gtk::events_pending() { gtk::main_iteration_do(false); }
    println!("PASS: main-thread WebKit close acknowledged before view destruction");
}
