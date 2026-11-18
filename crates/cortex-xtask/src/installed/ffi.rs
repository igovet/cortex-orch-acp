//! Late-bound access to the container's own GLib/GIO, AT-SPI and GTK 3 libraries.
//!
//! The libraries are opened with `dlopen` at run time, so the Rust tool has no link-time or
//! build-time dependency on them and nothing here runs on the host. Only the disposable
//! container (which already carries these libraries for the desktop application) uses it.

use crate::json::Res;
use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void, CStr, CString};
use std::sync::OnceLock;

pub type P = *mut c_void;

#[repr(C)]
pub struct GError {
    domain: u32,
    code: c_int,
    message: *mut c_char,
}

#[repr(C)]
struct GArray {
    data: *mut c_char,
    len: c_uint,
}

#[repr(C)]
struct AtRect {
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
}

fn open(names: &[&str]) -> Res<P> {
    for name in names {
        let c = CString::new(*name).unwrap();
        let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
        if !h.is_null() {
            return Ok(h);
        }
    }
    Err(format!("cannot load {}", names.join(" or ")))
}

fn sym<T: Copy>(handle: P, name: &str) -> Res<T> {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<P>());
    let c = CString::new(name).unwrap();
    let p = unsafe { libc::dlsym(handle, c.as_ptr()) };
    if p.is_null() {
        return Err(format!("missing library symbol {name}"));
    }
    Ok(unsafe { std::mem::transmute_copy::<P, T>(&p) })
}

fn take_error(err: *mut GError, free: unsafe extern "C" fn(*mut GError)) -> String {
    if err.is_null() {
        return "unknown error".into();
    }
    let message = unsafe { CStr::from_ptr((*err).message) }
        .to_string_lossy()
        .into_owned();
    unsafe { free(err) };
    message
}

fn take_string(p: *mut c_char, free: unsafe extern "C" fn(P)) -> String {
    if p.is_null() {
        return String::new();
    }
    let text = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { free(p as P) };
    text
}

// ---------------------------------------------------------------- GLib / GIO / D-Bus

pub struct Gio {
    g_free: unsafe extern "C" fn(P),
    g_error_free: unsafe extern "C" fn(*mut GError),
    g_object_unref: unsafe extern "C" fn(P),
    g_variant_unref: unsafe extern "C" fn(P),
    bus_get: unsafe extern "C" fn(c_int, P, *mut *mut GError) -> P,
    call_sync: unsafe extern "C" fn(
        P,
        *const c_char,
        *const c_char,
        *const c_char,
        *const c_char,
        P,
        P,
        c_int,
        c_int,
        P,
        *mut *mut GError,
    ) -> P,
    v_double: unsafe extern "C" fn(f64) -> P,
    v_int32: unsafe extern "C" fn(i32) -> P,
    v_uint32: unsafe extern "C" fn(u32) -> P,
    v_bool: unsafe extern "C" fn(c_int) -> P,
    v_string: unsafe extern "C" fn(*const c_char) -> P,
    v_tuple: unsafe extern "C" fn(*const P, usize) -> P,
    v_child: unsafe extern "C" fn(P, usize) -> P,
    v_get_bool: unsafe extern "C" fn(P) -> c_int,
    v_get_string: unsafe extern "C" fn(P, *mut usize) -> *const c_char,
}

pub fn gio() -> Res<&'static Gio> {
    static CELL: OnceLock<Result<Gio, String>> = OnceLock::new();
    CELL.get_or_init(|| {
        let h = open(&["libgio-2.0.so.0"])?;
        Ok(Gio {
            g_free: sym(h, "g_free")?,
            g_error_free: sym(h, "g_error_free")?,
            g_object_unref: sym(h, "g_object_unref")?,
            g_variant_unref: sym(h, "g_variant_unref")?,
            bus_get: sym(h, "g_bus_get_sync")?,
            call_sync: sym(h, "g_dbus_connection_call_sync")?,
            v_double: sym(h, "g_variant_new_double")?,
            v_int32: sym(h, "g_variant_new_int32")?,
            v_uint32: sym(h, "g_variant_new_uint32")?,
            v_bool: sym(h, "g_variant_new_boolean")?,
            v_string: sym(h, "g_variant_new_string")?,
            v_tuple: sym(h, "g_variant_new_tuple")?,
            v_child: sym(h, "g_variant_get_child_value")?,
            v_get_bool: sym(h, "g_variant_get_boolean")?,
            v_get_string: sym(h, "g_variant_get_string")?,
        })
    })
    .as_ref()
    .map_err(|e| e.clone())
}

pub struct Variant(P);

impl Drop for Variant {
    fn drop(&mut self) {
        if !self.0.is_null() {
            if let Ok(g) = gio() {
                unsafe { (g.g_variant_unref)(self.0) };
            }
        }
    }
}

/// A floating GVariant argument, consumed by the call that receives it.
pub enum Arg {
    Str(String),
    Doubles(f64, f64),
    IntBool(i32, bool),
    UintBool(u32, bool),
}

impl Arg {
    fn build(&self, g: &Gio) -> P {
        unsafe {
            match self {
                Arg::Str(text) => {
                    let c = CString::new(text.as_str()).unwrap();
                    let v = (g.v_string)(c.as_ptr());
                    (g.v_tuple)([v].as_ptr(), 1)
                }
                Arg::Doubles(a, b) => (g.v_tuple)([(g.v_double)(*a), (g.v_double)(*b)].as_ptr(), 2),
                Arg::IntBool(a, b2) => {
                    (g.v_tuple)([(g.v_int32)(*a), (g.v_bool)(*b2 as c_int)].as_ptr(), 2)
                }
                Arg::UintBool(a, b2) => {
                    (g.v_tuple)([(g.v_uint32)(*a), (g.v_bool)(*b2 as c_int)].as_ptr(), 2)
                }
            }
        }
    }
}

impl Variant {
    /// Child `index` of a tuple reply as a string (object path or plain string).
    pub fn child_string(&self, index: usize) -> Res<String> {
        let g = gio()?;
        unsafe {
            let child = Variant((g.v_child)(self.0, index));
            let text = (g.v_get_string)(child.0, std::ptr::null_mut());
            if text.is_null() {
                return Err("reply child is not a string".into());
            }
            Ok(CStr::from_ptr(text).to_string_lossy().into_owned())
        }
    }

    pub fn child_bool(&self, index: usize) -> Res<bool> {
        let g = gio()?;
        unsafe {
            let child = Variant((g.v_child)(self.0, index));
            Ok((g.v_get_bool)(child.0) != 0)
        }
    }
}

pub struct Bus(P);

impl Bus {
    pub fn session() -> Res<Bus> {
        let g = gio()?;
        let mut err: *mut GError = std::ptr::null_mut();
        let conn = unsafe { (g.bus_get)(2, std::ptr::null_mut(), &mut err) };
        if conn.is_null() {
            return Err(take_error(err, g.g_error_free));
        }
        Ok(Bus(conn))
    }

    pub fn call(
        &self,
        dest: &str,
        path: &str,
        interface: &str,
        method: &str,
        arg: Option<Arg>,
        timeout_ms: i32,
    ) -> Res<Variant> {
        let g = gio()?;
        let (d, p, i, m) = (
            CString::new(dest).unwrap(),
            CString::new(path).unwrap(),
            CString::new(interface).unwrap(),
            CString::new(method).unwrap(),
        );
        let params = arg.map(|a| a.build(g)).unwrap_or(std::ptr::null_mut());
        let mut err: *mut GError = std::ptr::null_mut();
        let reply = unsafe {
            (g.call_sync)(
                self.0,
                d.as_ptr(),
                p.as_ptr(),
                i.as_ptr(),
                m.as_ptr(),
                params,
                std::ptr::null_mut(),
                0,
                timeout_ms,
                std::ptr::null_mut(),
                &mut err,
            )
        };
        if reply.is_null() {
            return Err(take_error(err, g.g_error_free));
        }
        Ok(Variant(reply))
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        if let Ok(g) = gio() {
            unsafe { (g.g_object_unref)(self.0) };
        }
    }
}

// ---------------------------------------------------------------- AT-SPI

pub struct Atspi {
    g_free: unsafe extern "C" fn(P),
    g_error_free: unsafe extern "C" fn(*mut GError),
    g_object_unref: unsafe extern "C" fn(P),
    g_array_free: unsafe extern "C" fn(*mut GArray, c_int) -> *mut c_char,
    get_desktop: unsafe extern "C" fn(c_int) -> P,
    child_count: unsafe extern "C" fn(P, *mut *mut GError) -> c_int,
    child_at: unsafe extern "C" fn(P, c_int, *mut *mut GError) -> P,
    name: unsafe extern "C" fn(P, *mut *mut GError) -> *mut c_char,
    role_name: unsafe extern "C" fn(P, *mut *mut GError) -> *mut c_char,
    state_set: unsafe extern "C" fn(P) -> P,
    state_contains: unsafe extern "C" fn(P, c_int) -> c_int,
    interfaces: unsafe extern "C" fn(P) -> *mut GArray,
    extents: unsafe extern "C" fn(P, c_int, *mut *mut GError) -> *mut AtRect,
    text: unsafe extern "C" fn(P, c_int, c_int, *mut *mut GError) -> *mut c_char,
    process_id: unsafe extern "C" fn(P, *mut *mut GError) -> c_int,
    do_action: unsafe extern "C" fn(P, c_int, *mut *mut GError) -> c_int,
    grab_focus: unsafe extern "C" fn(P, *mut *mut GError) -> c_int,
}

fn atspi() -> Res<&'static Atspi> {
    static CELL: OnceLock<Result<Atspi, String>> = OnceLock::new();
    CELL.get_or_init(|| {
        gio()?;
        let h = open(&["libatspi.so.0"])?;
        let init: unsafe extern "C" fn() -> c_int = sym(h, "atspi_init")?;
        unsafe { init() };
        Ok(Atspi {
            g_free: sym(h, "g_free")?,
            g_error_free: sym(h, "g_error_free")?,
            g_object_unref: sym(h, "g_object_unref")?,
            g_array_free: sym(h, "g_array_free")?,
            get_desktop: sym(h, "atspi_get_desktop")?,
            child_count: sym(h, "atspi_accessible_get_child_count")?,
            child_at: sym(h, "atspi_accessible_get_child_at_index")?,
            name: sym(h, "atspi_accessible_get_name")?,
            role_name: sym(h, "atspi_accessible_get_role_name")?,
            state_set: sym(h, "atspi_accessible_get_state_set")?,
            state_contains: sym(h, "atspi_state_set_contains")?,
            interfaces: sym(h, "atspi_accessible_get_interfaces")?,
            extents: sym(h, "atspi_component_get_extents")?,
            text: sym(h, "atspi_text_get_text")?,
            process_id: sym(h, "atspi_accessible_get_process_id")?,
            do_action: sym(h, "atspi_action_do_action")?,
            grab_focus: sym(h, "atspi_component_grab_focus")?,
        })
    })
    .as_ref()
    .map_err(|e| e.clone())
}

// AtspiStateType values from atspi-constants.h.
pub const STATE_DEFUNCT: c_int = 6;
pub const STATE_ENABLED: c_int = 8;
pub const STATE_FOCUSED: c_int = 12;
pub const STATE_SELECTED: c_int = 23;
pub const STATE_SHOWING: c_int = 25;
pub const COORD_SCREEN: c_int = 0;
pub const COORD_WINDOW: c_int = 1;

/// One owned accessible object reference.
pub struct Acc(P);

impl Drop for Acc {
    fn drop(&mut self) {
        if !self.0.is_null() {
            if let Ok(a) = atspi() {
                unsafe { (a.g_object_unref)(self.0) };
            }
        }
    }
}

impl Acc {
    pub fn desktop() -> Res<Acc> {
        let a = atspi()?;
        let p = unsafe { (a.get_desktop)(0) };
        if p.is_null() {
            return Err("accessibility desktop unavailable".into());
        }
        Ok(Acc(p))
    }

    pub fn child_count(&self) -> Res<i32> {
        let a = atspi()?;
        let mut err = std::ptr::null_mut();
        let k = unsafe { (a.child_count)(self.0, &mut err) };
        if !err.is_null() {
            return Err(take_error(err, a.g_error_free));
        }
        Ok(k)
    }

    pub fn child(&self, index: i32) -> Res<Option<Acc>> {
        let a = atspi()?;
        let mut err = std::ptr::null_mut();
        let p = unsafe { (a.child_at)(self.0, index, &mut err) };
        if !err.is_null() {
            if !p.is_null() {
                unsafe { (a.g_object_unref)(p) };
            }
            return Err(take_error(err, a.g_error_free));
        }
        Ok((!p.is_null()).then_some(Acc(p)))
    }

    fn string(&self, f: unsafe extern "C" fn(P, *mut *mut GError) -> *mut c_char) -> Res<String> {
        let a = atspi()?;
        let mut err = std::ptr::null_mut();
        let p = unsafe { f(self.0, &mut err) };
        if !err.is_null() {
            if !p.is_null() {
                unsafe { (a.g_free)(p as P) };
            }
            return Err(take_error(err, a.g_error_free));
        }
        Ok(take_string(p, a.g_free))
    }

    pub fn name(&self) -> Res<String> {
        self.string(atspi()?.name)
    }

    pub fn role_name(&self) -> Res<String> {
        self.string(atspi()?.role_name)
    }

    /// State flags `(defunct, showing, enabled, focused, selected)`.
    pub fn states(&self) -> Res<[bool; 5]> {
        let a = atspi()?;
        let set = unsafe { (a.state_set)(self.0) };
        if set.is_null() {
            return Err("state set unavailable".into());
        }
        let has = |s: c_int| unsafe { (a.state_contains)(set, s) != 0 };
        let out = [
            has(STATE_DEFUNCT),
            has(STATE_SHOWING),
            has(STATE_ENABLED),
            has(STATE_FOCUSED),
            has(STATE_SELECTED),
        ];
        unsafe { (a.g_object_unref)(set) };
        Ok(out)
    }

    /// Short interface names such as `Component` and `Text`.
    pub fn interfaces(&self) -> Res<Vec<String>> {
        let a = atspi()?;
        let array = unsafe { (a.interfaces)(self.0) };
        if array.is_null() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        unsafe {
            let items = (*array).data as *const *const c_char;
            for i in 0..(*array).len as usize {
                let item = *items.add(i);
                if !item.is_null() {
                    names.push(
                        CStr::from_ptr(item)
                            .to_string_lossy()
                            .rsplit('.')
                            .next()
                            .unwrap_or("")
                            .to_string(),
                    );
                    (a.g_free)(item as P);
                }
            }
            (a.g_array_free)(array, 1);
        }
        Ok(names)
    }

    pub fn extents(&self, coord: c_int) -> Res<(i32, i32, i32, i32)> {
        let a = atspi()?;
        let mut err = std::ptr::null_mut();
        let p = unsafe { (a.extents)(self.0, coord, &mut err) };
        if !err.is_null() {
            if !p.is_null() {
                unsafe { (a.g_free)(p as P) };
            }
            return Err(take_error(err, a.g_error_free));
        }
        if p.is_null() {
            return Ok((0, 0, 0, 0));
        }
        let r = unsafe { (p as *const AtRect).read() };
        unsafe { (a.g_free)(p as P) };
        Ok((r.x, r.y, r.width, r.height))
    }

    /// The owning process id the application reported.
    pub fn process_id(&self) -> Res<i32> {
        let a = atspi()?;
        let mut err = std::ptr::null_mut();
        let pid = unsafe { (a.process_id)(self.0, &mut err) };
        if !err.is_null() {
            return Err(take_error(err, a.g_error_free));
        }
        Ok(pid)
    }

    /// The object's first action (`do_action(0)`), as a click or press; false when it declined.
    pub fn do_action(&self, index: i32) -> Res<bool> {
        let a = atspi()?;
        let mut err = std::ptr::null_mut();
        let done = unsafe { (a.do_action)(self.0, index, &mut err) };
        if !err.is_null() {
            return Err(take_error(err, a.g_error_free));
        }
        Ok(done != 0)
    }

    /// Asks the component to take keyboard focus; false when it declined.
    pub fn grab_focus(&self) -> Res<bool> {
        let a = atspi()?;
        let mut err = std::ptr::null_mut();
        let done = unsafe { (a.grab_focus)(self.0, &mut err) };
        if !err.is_null() {
            return Err(take_error(err, a.g_error_free));
        }
        Ok(done != 0)
    }

    pub fn text(&self) -> Res<String> {
        let a = atspi()?;
        let mut err = std::ptr::null_mut();
        let p = unsafe { (a.text)(self.0, 0, -1, &mut err) };
        if !err.is_null() {
            if !p.is_null() {
                unsafe { (a.g_free)(p as P) };
            }
            return Err(take_error(err, a.g_error_free));
        }
        Ok(take_string(p, a.g_free))
    }
}

// ---------------------------------------------------------------- GTK 3 (external drag source, clipboard owner)

unsafe impl Send for Gtk {}
unsafe impl Sync for Gtk {}

struct Gtk {
    init_check: unsafe extern "C" fn(P, P) -> c_int,
    window_new: unsafe extern "C" fn(c_int) -> P,
    set_title: unsafe extern "C" fn(P, *const c_char),
    set_default_size: unsafe extern "C" fn(P, c_int, c_int),
    window_move: unsafe extern "C" fn(P, c_int, c_int),
    event_box_new: unsafe extern "C" fn() -> P,
    label_new: unsafe extern "C" fn(*const c_char) -> P,
    container_add: unsafe extern "C" fn(P, P),
    show_all: unsafe extern "C" fn(P),
    drag_source_set: unsafe extern "C" fn(P, c_int, *const TargetEntry, c_int, c_int),
    set_uris: unsafe extern "C" fn(P, *const *const c_char) -> c_int,
    signal_connect: unsafe extern "C" fn(P, *const c_char, P, P, P, c_int) -> c_ulong,
    main: unsafe extern "C" fn(),
    main_quit: P,
    alloc_w: unsafe extern "C" fn(P) -> c_int,
    alloc_h: unsafe extern "C" fn(P) -> c_int,
    timeout_add: unsafe extern "C" fn(c_uint, P, P) -> c_uint,
    filename_to_uri:
        unsafe extern "C" fn(*const c_char, *const c_char, *mut *mut GError) -> *mut c_char,
    clipboard_get: unsafe extern "C" fn(P) -> P,
    clipboard_set_text: unsafe extern "C" fn(P, *const c_char, c_int),
}

#[repr(C)]
struct TargetEntry {
    target: *const c_char,
    flags: c_uint,
    info: c_uint,
}

fn gtk() -> Res<&'static Gtk> {
    static CELL: OnceLock<Result<Gtk, String>> = OnceLock::new();
    CELL.get_or_init(|| {
        gio()?;
        let h = open(&["libgtk-3.so.0"])?;
        let g = open(&["libgio-2.0.so.0"])?;
        let init_check: unsafe extern "C" fn(P, P) -> c_int = sym(h, "gtk_init_check")?;
        if unsafe { init_check(std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
            return Err("GTK could not open the display".into());
        }
        Ok(Gtk {
            init_check,
            window_new: sym(h, "gtk_window_new")?,
            set_title: sym(h, "gtk_window_set_title")?,
            set_default_size: sym(h, "gtk_window_set_default_size")?,
            window_move: sym(h, "gtk_window_move")?,
            event_box_new: sym(h, "gtk_event_box_new")?,
            label_new: sym(h, "gtk_label_new")?,
            container_add: sym(h, "gtk_container_add")?,
            show_all: sym(h, "gtk_widget_show_all")?,
            drag_source_set: sym(h, "gtk_drag_source_set")?,
            set_uris: sym(h, "gtk_selection_data_set_uris")?,
            signal_connect: sym(g, "g_signal_connect_data")?,
            main: sym(h, "gtk_main")?,
            main_quit: sym(h, "gtk_main_quit")?,
            alloc_w: sym(h, "gtk_widget_get_allocated_width")?,
            alloc_h: sym(h, "gtk_widget_get_allocated_height")?,
            timeout_add: sym(g, "g_timeout_add")?,
            filename_to_uri: sym(g, "g_filename_to_uri")?,
            clipboard_get: sym(h, "gtk_clipboard_get")?,
            clipboard_set_text: sym(h, "gtk_clipboard_set_text")?,
        })
    })
    .as_ref()
    .map_err(|e| e.clone())
}

struct DragState {
    uri: CString,
    ready: std::path::PathBuf,
    backend_x11: bool,
    window: P,
}

fn touch(path: &std::path::Path) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
}

fn marker(ready: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut name = ready
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    // Path.with_suffix: replace an existing suffix, otherwise append.
    let text = name.to_string_lossy().into_owned();
    let stem = text
        .rsplit_once('.')
        .filter(|(s, _)| !s.is_empty())
        .map(|(s, _)| s.to_string())
        .unwrap_or(text);
    name = format!("{stem}.{suffix}").into();
    ready.with_file_name(name)
}

unsafe extern "C" fn on_data_get(_w: P, _ctx: P, data: P, _info: c_uint, _time: c_uint, user: P) {
    let st = &*(user as *const DragState);
    touch(&marker(&st.ready, "requested"));
    let uris = [st.uri.as_ptr(), std::ptr::null()];
    if let Ok(g) = gtk() {
        (g.set_uris)(data, uris.as_ptr());
    }
}

unsafe extern "C" fn on_begin(_w: P, _ctx: P, user: P) {
    touch(&marker(&(*(user as *const DragState)).ready, "began"));
}

unsafe extern "C" fn on_end(_w: P, _ctx: P, user: P) {
    touch(&marker(&(*(user as *const DragState)).ready, "ended"));
}

unsafe extern "C" fn on_tick(user: P) -> c_int {
    let st = &*(user as *const DragState);
    let g = match gtk() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    if st.backend_x11 || ((g.alloc_w)(st.window) >= 950 && (g.alloc_h)(st.window) >= 1100) {
        touch(&st.ready);
        return 0;
    }
    1
}

/// External GTK drag source; data is offered only after real compositor input.
pub fn drag_source(fixture: &std::path::Path, ready: &std::path::Path, x11: bool) -> Res<()> {
    let g = gtk()?;
    unsafe {
        let window = (g.window_new)(0);
        let title = CString::new("Installed attachment fixture").unwrap();
        (g.set_title)(window, title.as_ptr());
        let area = (g.event_box_new)();
        let text = CString::new("Drag this fixture").unwrap();
        (g.container_add)(area, (g.label_new)(text.as_ptr()));
        (g.container_add)(window, area);
        let target = CString::new("text/uri-list").unwrap();
        let entry = TargetEntry {
            target: target.as_ptr(),
            flags: 0,
            info: 0,
        };
        (g.drag_source_set)(area, 1 << 8, &entry, 1, 2);
        let path = CString::new(fixture.to_string_lossy().as_bytes()).unwrap();
        let mut err: *mut GError = std::ptr::null_mut();
        let uri = (g.filename_to_uri)(path.as_ptr(), std::ptr::null(), &mut err);
        if uri.is_null() {
            return Err(take_error(err, gio()?.g_error_free));
        }
        let uri_text = CStr::from_ptr(uri).to_owned();
        (gio()?.g_free)(uri as P);
        let state = Box::leak(Box::new(DragState {
            uri: uri_text,
            ready: ready.to_path_buf(),
            backend_x11: x11,
            window,
        }));
        let user = state as *mut DragState as P;
        let connect = |obj: P, sig: &str, cb: P, data: P| {
            let c = CString::new(sig).unwrap();
            (g.signal_connect)(obj, c.as_ptr(), cb, data, std::ptr::null_mut(), 0);
        };
        connect(area, "drag-data-get", on_data_get as P, user);
        connect(area, "drag-begin", on_begin as P, user);
        connect(area, "drag-end", on_end as P, user);
        connect(window, "destroy", g.main_quit, std::ptr::null_mut());
        (g.set_default_size)(window, 180, 100);
        if x11 {
            (g.window_move)(window, 20, 70);
        }
        (g.show_all)(window);
        (g.timeout_add)(100, on_tick as P, user);
        (g.main)();
    }
    let _ = g.init_check;
    Ok(())
}

/// Own the CLIPBOARD selection through GTK until killed; `ready` is touched once offered.
pub fn clipboard_owner(ready: &std::path::Path, text: &str) -> Res<()> {
    let g = gtk()?;
    unsafe {
        let clipboard = (g.clipboard_get)(69usize as P); // GDK_SELECTION_CLIPBOARD
        let c = CString::new(text).map_err(|_| "clipboard text has NUL")?;
        (g.clipboard_set_text)(clipboard, c.as_ptr(), -1);
        touch(ready);
        (g.main)();
    }
    Ok(())
}
