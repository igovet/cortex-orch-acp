//! The few D-Bus calls the service makes on the owner's session bus, over the bus socket
//! itself. No D-Bus library is in the locked dependency set except GIO, which belongs to the
//! GTK stack the service does not link, so this speaks the wire protocol directly: EXTERNAL
//! authentication, `Hello`, method calls with simple bodies, and the replies and signals it
//! needs to read back. Messages are written little-endian; replies in either byte order are
//! read. Nothing else on the bus is answered.
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// The largest message read back. Notification servers send small replies and signals.
const MAX_MESSAGE: usize = 1 << 20;

/// One value of a simple body: what notification and bus replies carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Arg {
    U32(u32),
    Str(String),
}
impl Arg {
    pub(crate) fn u32(&self) -> Option<u32> {
        match self {
            Arg::U32(v) => Some(*v),
            _ => None,
        }
    }
    pub(crate) fn str(&self) -> Option<&str> {
        match self {
            Arg::Str(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Reply {
    pub sender: String,
    pub error: Option<String>,
    pub body: Vec<Arg>,
}

/// A body under construction. Offsets are relative to the body start, which the header pads
/// to 8, so alignment here is the message's alignment.
#[derive(Default)]
pub(crate) struct Body {
    pub(crate) bytes: Vec<u8>,
}
impl Body {
    fn pad(&mut self, align: usize) {
        while self.bytes.len() % align != 0 {
            self.bytes.push(0);
        }
    }
    pub(crate) fn u32(&mut self, v: u32) -> &mut Self {
        self.pad(4);
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub(crate) fn i32(&mut self, v: i32) -> &mut Self {
        self.u32(v as u32)
    }
    pub(crate) fn str(&mut self, v: &str) -> &mut Self {
        self.u32(v.len() as u32);
        self.bytes.extend_from_slice(v.as_bytes());
        self.bytes.push(0);
        self
    }
    fn signature(&mut self, v: &str) -> &mut Self {
        self.bytes.push(v.len() as u8);
        self.bytes.extend_from_slice(v.as_bytes());
        self.bytes.push(0);
        self
    }
    /// An array whose elements align to `align`; `items` writes them.
    pub(crate) fn array(&mut self, align: usize, items: impl FnOnce(&mut Self)) -> &mut Self {
        self.pad(4);
        let at = self.bytes.len();
        self.bytes.extend_from_slice(&[0; 4]);
        self.pad(align);
        let start = self.bytes.len();
        items(self);
        let length = (self.bytes.len() - start) as u32;
        self.bytes[at..at + 4].copy_from_slice(&length.to_le_bytes());
        self
    }
    pub(crate) fn strings(&mut self, items: &[&str]) -> &mut Self {
        self.array(4, |b| {
            for item in items {
                b.str(item);
            }
        })
    }
    /// `a{sv}` whose values are all strings.
    pub(crate) fn string_hints(&mut self, hints: &[(&str, &str)]) -> &mut Self {
        self.array(8, |b| {
            for (key, value) in hints {
                b.pad(8);
                b.str(key);
                b.signature("s");
                b.str(value);
            }
        })
    }
}

/// One header field: its code and its value (`o`, `s` and `g` as text, `u` as a number).
enum Field<'a> {
    Text(u8, char, &'a str),
    #[cfg_attr(not(test), allow(dead_code))]
    Number(u8, u32),
}

fn header(
    kind: u8,
    flags: u8,
    serial: u32,
    fields: &[Field<'_>],
    signature: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut h = Body::default();
    h.bytes.extend_from_slice(&[b'l', kind, flags, 1]);
    h.u32(body.len() as u32);
    h.u32(serial);
    h.array(8, |b| {
        let signed = (!signature.is_empty()).then_some(Field::Text(8, 'g', signature));
        for field in fields.iter().chain(signed.as_ref()) {
            b.pad(8);
            match field {
                Field::Text(code, kind, value) => {
                    b.bytes.push(*code);
                    b.signature(&kind.to_string());
                    if *kind == 'g' {
                        b.signature(value);
                    } else {
                        b.str(value);
                    }
                }
                Field::Number(code, value) => {
                    b.bytes.push(*code);
                    b.signature("u");
                    b.u32(*value);
                }
            }
        }
    });
    h.pad(8);
    h.bytes.extend_from_slice(body);
    h.bytes
}

/// A method call message. `signature` describes `body`.
pub(crate) fn method_call(
    serial: u32,
    destination: &str,
    path: &str,
    interface: &str,
    member: &str,
    signature: &str,
    body: &[u8],
    no_reply: bool,
) -> Vec<u8> {
    let fields = [
        Field::Text(1, 'o', path),
        Field::Text(2, 's', interface),
        Field::Text(3, 's', member),
        Field::Text(6, 's', destination),
    ];
    // NO_AUTO_START always: the service never starts a bus service by calling it.
    header(
        1,
        if no_reply { 0x1 | 0x2 } else { 0x2 },
        serial,
        &fields,
        signature,
        body,
    )
}

/// A method return, as the tests' stand-in server sends one.
#[cfg(test)]
pub(crate) fn method_return(
    serial: u32,
    destination: &str,
    reply_serial: u32,
    signature: &str,
    body: &[u8],
) -> Vec<u8> {
    header(
        2,
        0,
        serial,
        &[
            Field::Number(5, reply_serial),
            Field::Text(6, 's', destination),
        ],
        signature,
        body,
    )
}

/// A signal message, as a notification server sends one (used by the tests' stand-in server).
#[cfg(test)]
pub(crate) fn signal(
    serial: u32,
    destination: &str,
    path: &str,
    interface: &str,
    member: &str,
    signature: &str,
    body: &[u8],
) -> Vec<u8> {
    let fields = [
        Field::Text(1, 'o', path),
        Field::Text(2, 's', interface),
        Field::Text(3, 's', member),
        Field::Text(6, 's', destination),
    ];
    header(4, 0, serial, &fields, signature, body)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    little: bool,
}
impl Reader<'_> {
    fn align(&mut self, n: usize) -> Option<()> {
        while self.at % n != 0 {
            self.at += 1;
        }
        (self.at <= self.bytes.len()).then_some(())
    }
    fn u8(&mut self) -> Option<u8> {
        let v = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        self.align(4)?;
        let raw: [u8; 4] = self.bytes.get(self.at..self.at + 4)?.try_into().ok()?;
        self.at += 4;
        Some(if self.little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        })
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        let raw = self.bytes.get(self.at..self.at + n)?;
        self.at += n + 1;
        String::from_utf8(raw.to_vec()).ok()
    }
    fn signature(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        let raw = self.bytes.get(self.at..self.at + n)?;
        self.at += n + 1;
        String::from_utf8(raw.to_vec()).ok()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Message {
    pub kind: u8,
    pub serial: u32,
    pub reply_serial: Option<u32>,
    pub path: String,
    pub interface: String,
    pub member: String,
    pub error: Option<String>,
    pub sender: String,
    pub destination: String,
    pub body: Vec<Arg>,
}

/// Reads one complete message, or `None` for anything malformed.
pub(crate) fn parse(bytes: &[u8]) -> Option<Message> {
    let little = match bytes.first()? {
        b'l' => true,
        b'B' => false,
        _ => return None,
    };
    let mut r = Reader {
        bytes,
        at: 1,
        little,
    };
    let kind = r.u8()?;
    r.at = 4;
    let body_len = r.u32()? as usize;
    let serial = r.u32()?;
    let fields_len = r.u32()? as usize;
    let end = 16usize.checked_add(fields_len)?;
    let mut m = Message {
        kind,
        serial,
        ..Default::default()
    };
    let mut signature = String::new();
    while r.at < end {
        r.align(8)?;
        if r.at >= end {
            break;
        }
        let code = r.u8()?;
        let kind = r.signature()?;
        match kind.as_str() {
            "s" | "o" => {
                let v = r.str()?;
                match code {
                    1 => m.path = v,
                    2 => m.interface = v,
                    3 => m.member = v,
                    4 => m.error = Some(v),
                    6 => m.destination = v,
                    7 => m.sender = v,
                    _ => {}
                }
            }
            "g" => {
                let v = r.signature()?;
                if code == 8 {
                    signature = v;
                }
            }
            "u" => {
                let v = r.u32()?;
                if code == 5 {
                    m.reply_serial = Some(v);
                }
            }
            _ => return None,
        }
    }
    r.at = end;
    r.align(8)?;
    let start = r.at;
    let body = bytes.get(start..start.checked_add(body_len)?)?;
    let mut b = Reader {
        bytes: body,
        at: 0,
        little,
    };
    for code in signature.chars() {
        match code {
            'u' | 'i' | 'b' => m.body.push(Arg::U32(b.u32()?)),
            's' | 'o' => m.body.push(Arg::Str(b.str()?)),
            // Anything richer is not read; what came before it is kept.
            _ => break,
        }
    }
    Some(m)
}

fn read_message(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut fixed = [0u8; 16];
    stream.read_exact(&mut fixed)?;
    let n = |at: usize| {
        let raw: [u8; 4] = fixed[at..at + 4].try_into().unwrap();
        if fixed[0] == b'l' {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        }
    };
    let (body, fields) = (n(4) as usize, n(12) as usize);
    let total = (16 + fields).div_ceil(8) * 8 + body;
    if total > MAX_MESSAGE {
        return Err(io::Error::other("bus_message_too_large"));
    }
    let mut message = fixed.to_vec();
    message.resize(total, 0);
    stream.read_exact(&mut message[16..])?;
    Ok(message)
}

/// The session bus address the service may use: `DBUS_SESSION_BUS_ADDRESS` (its `unix:path=`
/// form), else the per-user `$XDG_RUNTIME_DIR/bus`. `None` when there is no session bus.
pub(crate) fn session_address() -> Option<PathBuf> {
    if let Ok(address) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        for entry in address.split(';') {
            let Some(rest) = entry.strip_prefix("unix:") else {
                continue;
            };
            for pair in rest.split(',') {
                if let Some(path) = pair.strip_prefix("path=") {
                    return Some(PathBuf::from(unescape(path)));
                }
            }
        }
        return None;
    }
    let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("bus");
    path.exists().then_some(path)
}
fn unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(v) = value
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One connection to the session bus. Calls wait for their reply; signals go to the
/// receiver given at connection.
pub(crate) struct Bus {
    writer: Mutex<UnixStream>,
    serial: AtomicU32,
    pending: Arc<Mutex<HashMap<u32, mpsc::Sender<Reply>>>>,
    pub(crate) unique: String,
}
impl Bus {
    /// `incoming` receives every signal (kind 4) and method call (kind 1) addressed to or
    /// broadcast past this connection; the service only reads signals from it.
    pub(crate) fn connect(
        path: &std::path::Path,
        incoming: impl Fn(Message) + Send + 'static,
    ) -> io::Result<Arc<Bus>> {
        let mut stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let uid = unsafe { libc::geteuid() }.to_string();
        let hex: String = uid.bytes().map(|b| format!("{b:02x}")).collect();
        stream.write_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes())?;
        let mut line = String::new();
        BufReader::new(&mut stream).take(512).read_line(&mut line)?;
        if !line.starts_with("OK ") {
            return Err(io::Error::other("bus_auth_refused"));
        }
        stream.write_all(b"BEGIN\r\n")?;
        stream.set_read_timeout(None)?;
        let mut reader = stream.try_clone()?;
        let pending: Arc<Mutex<HashMap<u32, mpsc::Sender<Reply>>>> = Default::default();
        let replies = pending.clone();
        std::thread::Builder::new()
            .name("session-bus".into())
            .spawn(move || {
                while let Ok(bytes) = read_message(&mut reader) {
                    let Some(m) = parse(&bytes) else { continue };
                    match m.kind {
                        2 | 3 => {
                            let waiting =
                                m.reply_serial.and_then(|s| replies.lock().ok()?.remove(&s));
                            if let Some(waiting) = waiting {
                                let _ = waiting.send(Reply {
                                    sender: m.sender,
                                    error: m.error,
                                    body: m.body,
                                });
                            }
                        }
                        1 | 4 => incoming(m),
                        _ => {}
                    }
                }
                // The connection ended: every waiting call fails now rather than at its deadline.
                if let Ok(mut held) = replies.lock() {
                    held.clear();
                }
            })?;
        let mut bus = Bus {
            writer: Mutex::new(stream),
            serial: AtomicU32::new(1),
            pending,
            unique: String::new(),
        };
        let hello = bus.call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "Hello",
            "",
            &[],
        )?;
        bus.unique = hello
            .body
            .first()
            .and_then(Arg::str)
            .ok_or_else(|| io::Error::other("bus_hello_failed"))?
            .to_owned();
        Ok(Arc::new(bus))
    }
    fn send(&self, message: &[u8]) -> io::Result<()> {
        self.writer
            .lock()
            .map_err(|_| io::Error::other("bus_closed"))?
            .write_all(message)
    }
    pub(crate) fn call(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        signature: &str,
        body: &[u8],
    ) -> io::Result<Reply> {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst);
        let (send, receive) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| io::Error::other("bus_closed"))?
            .insert(serial, send);
        let sent = self.send(&method_call(
            serial,
            destination,
            path,
            interface,
            member,
            signature,
            body,
            false,
        ));
        if let Err(e) = sent {
            if let Ok(mut held) = self.pending.lock() {
                held.remove(&serial);
            }
            return Err(e);
        }
        let reply = receive.recv_timeout(Duration::from_secs(3));
        if let Ok(mut held) = self.pending.lock() {
            held.remove(&serial);
        }
        let reply = reply.map_err(|_| io::Error::other("bus_no_reply"))?;
        match &reply.error {
            Some(_) => Err(io::Error::other("bus_call_refused")),
            None => Ok(reply),
        }
    }
    pub(crate) fn notify_only(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        signature: &str,
        body: &[u8],
    ) -> io::Result<()> {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst);
        self.send(&method_call(
            serial,
            destination,
            path,
            interface,
            member,
            signature,
            body,
            true,
        ))
    }
    /// For the tests' stand-in server: a signal sent to one peer.
    #[cfg(test)]
    pub(crate) fn emit(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        signature: &str,
        body: &[u8],
    ) -> io::Result<()> {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst);
        self.send(&signal(
            serial,
            destination,
            path,
            interface,
            member,
            signature,
            body,
        ))
    }
    /// For the tests' stand-in server: a reply to a method call.
    #[cfg(test)]
    pub(crate) fn reply(
        &self,
        destination: &str,
        reply_serial: u32,
        signature: &str,
        body: &[u8],
    ) -> io::Result<()> {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst);
        self.send(&method_return(
            serial,
            destination,
            reply_serial,
            signature,
            body,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notify_call_round_trips_through_the_parser() {
        let mut body = Body::default();
        body.str("Cortex Workspace")
            .u32(3)
            .str("dialog-question")
            .str("Question waiting")
            .str("Fix &amp; ship")
            .strings(&["default", "Open"])
            .string_hints(&[("desktop-entry", "cortex-workspace")])
            .i32(-1);
        let message = method_call(
            9,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
            "susssasa{sv}i",
            &body.bytes,
            false,
        );
        let m = parse(&message).unwrap();
        assert_eq!((m.kind, m.serial), (1, 9));
        assert_eq!(m.member, "Notify");
        assert_eq!(m.destination, "org.freedesktop.Notifications");
        // The parser reads the leading simple values and stops at the first array.
        assert_eq!(
            m.body,
            vec![
                Arg::Str("Cortex Workspace".into()),
                Arg::U32(3),
                Arg::Str("dialog-question".into()),
                Arg::Str("Question waiting".into()),
                Arg::Str("Fix &amp; ship".into())
            ]
        );
    }

    #[test]
    fn the_string_array_and_hint_layout_match_the_wire_format() {
        let mut body = Body::default();
        body.u32(1).strings(&["a"]).string_hints(&[("k", "v")]);
        // u32 1 | len 6 | "a\0" pad to 4 | len 16 at 16 | pad to 24... | "k" | sig "s" | "v"
        assert_eq!(&body.bytes[0..4], &1u32.to_le_bytes());
        assert_eq!(&body.bytes[4..8], &6u32.to_le_bytes());
        assert_eq!(&body.bytes[8..12], &1u32.to_le_bytes());
        assert_eq!(&body.bytes[12..14], b"a\0");
        // The dict's length sits at 16; its first entry is 8-aligned at 24.
        let dict_len = u32::from_le_bytes(body.bytes[16..20].try_into().unwrap()) as usize;
        assert_eq!(body.bytes.len(), 24 + dict_len);
        assert_eq!(&body.bytes[24..28], &1u32.to_le_bytes());
        assert_eq!(&body.bytes[28..30], b"k\0");
        assert_eq!(&body.bytes[30..33], b"\x01s\0");
    }

    #[test]
    fn signals_and_big_endian_replies_are_read() {
        let mut body = Body::default();
        body.u32(41).str("default");
        let message = signal(
            4,
            ":1.7",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "ActionInvoked",
            "us",
            &body.bytes,
        );
        let m = parse(&message).unwrap();
        assert_eq!((m.kind, m.member.as_str()), (4, "ActionInvoked"));
        assert_eq!(m.body, vec![Arg::U32(41), Arg::Str("default".into())]);
        // A big-endian method return carrying (u) 7, reply serial 3.
        let mut be = vec![
            b'B', 2, 0, 1, 0, 0, 0, 4, 0, 0, 0, 9, 0, 0, 0, 16, 5, 1, b'u', 0, 0, 0, 0, 3, 8, 1,
            b'g', 0, 1, b'u', 0, 0,
        ];
        be.extend_from_slice(&[0, 0, 0, 7]);
        let m = parse(&be).unwrap();
        assert_eq!((m.kind, m.reply_serial), (2, Some(3)));
        assert_eq!(m.body, vec![Arg::U32(7)]);
        assert!(parse(b"x").is_none());
        // A truncated message is refused, not read past its end.
        assert!(parse(&be[..20]).is_none());
    }

    #[test]
    fn the_session_address_is_the_unix_path_form_only() {
        assert_eq!(unescape("/run/user/1000/bus"), "/run/user/1000/bus");
        assert_eq!(unescape("/tmp/a%2db"), "/tmp/a-b");
    }
}
