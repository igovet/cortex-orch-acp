//! The standard close request to one exact X window: a `WM_DELETE_WINDOW` client message, sent through libX11 opened at run time.

use crate::json::Res;
use std::ffi::{c_char, c_int, c_long, c_ulong, c_void, CString};

#[repr(C)]
struct ClientMessage {
    kind: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut c_void,
    window: c_ulong,
    message_type: c_ulong,
    format: c_int,
    data: [c_long; 5],
}

/// An XEvent is a union of 24 longs; the client message occupies its start.
#[repr(C)]
union Event {
    client: std::mem::ManuallyDrop<ClientMessage>,
    pad: [c_long; 24],
}

pub fn delete_window(window: &str) -> Res<()> {
    let id: c_ulong = window
        .trim()
        .parse()
        .map_err(|_| "window id is not a number".to_string())?;
    let name = CString::new("libX11.so.6").unwrap();
    let lib = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW) };
    if lib.is_null() {
        return Err("libX11 is not available".into());
    }
    fn sym<T: Copy>(lib: *mut c_void, name: &str) -> Res<T> {
        let c = CString::new(name).unwrap();
        let p = unsafe { libc::dlsym(lib, c.as_ptr()) };
        if p.is_null() {
            return Err(format!("missing libX11 symbol {name}"));
        }
        Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
    }
    let open: unsafe extern "C" fn(*const c_char) -> *mut c_void = sym(lib, "XOpenDisplay")?;
    let intern: unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_ulong =
        sym(lib, "XInternAtom")?;
    let send: unsafe extern "C" fn(*mut c_void, c_ulong, c_int, c_long, *mut Event) -> c_int =
        sym(lib, "XSendEvent")?;
    let flush: unsafe extern "C" fn(*mut c_void) -> c_int = sym(lib, "XFlush")?;
    let close: unsafe extern "C" fn(*mut c_void) -> c_int = sym(lib, "XCloseDisplay")?;
    unsafe {
        let display = open(std::ptr::null());
        if display.is_null() {
            return Err("owned_display_unavailable".into());
        }
        let protocols = CString::new("WM_PROTOCOLS").unwrap();
        let delete = CString::new("WM_DELETE_WINDOW").unwrap();
        let mut event = Event { pad: [0; 24] };
        event.client = std::mem::ManuallyDrop::new(ClientMessage {
            kind: 33, // ClientMessage
            serial: 0,
            send_event: 1,
            display,
            window: id,
            message_type: intern(display, protocols.as_ptr(), 0),
            format: 32,
            data: [intern(display, delete.as_ptr(), 0) as c_long, 0, 0, 0, 0],
        });
        let sent = send(display, id, 0, 0, &mut event);
        flush(display);
        close(display);
        if sent == 0 {
            return Err("window_close_message_failed".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_message_fits_inside_an_xevent() {
        assert!(std::mem::size_of::<ClientMessage>() <= std::mem::size_of::<[c_long; 24]>());
        assert_eq!(std::mem::size_of::<ClientMessage>(), 96);
    }

    #[test]
    fn a_window_id_must_be_a_number() {
        assert!(delete_window("not-a-window").is_err());
    }
}
