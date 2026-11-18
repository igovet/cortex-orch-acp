//! Read accessibility geometry; send input exclusively through XTEST or Mutter.

use super::ffi::{self, Acc, Arg, Bus};
use super::util::*;
use crate::json::{Res, Value};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

pub const WIDTH: i64 = 1920;
pub const HEIGHT: i64 = 1200;

#[derive(Clone, Debug)]
pub struct Node {
    pub role: String,
    pub name: String,
    pub text: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub enabled: bool,
    pub focused: bool,
    pub selected: bool,
}

impl Node {
    pub fn both(&self) -> String {
        format!("{} {}", self.name, self.text)
    }

    pub fn json(&self) -> Value {
        obj(vec![
            ("role", s(&self.role)),
            ("name", s(&self.name)),
            ("text", s(&self.text)),
            ("x", n(self.x)),
            ("y", n(self.y)),
            ("w", n(self.w)),
            ("h", n(self.h)),
            ("enabled", b(self.enabled)),
            ("focused", b(self.focused)),
            ("selected", b(self.selected)),
        ])
    }
}

/// The task composer's group: Start, Stop and Resume are its action button and nowhere else.
pub const TASK_COMPOSER: &str = "Task composer";

pub const DEFAULT_ROLES: [&str; 8] = [
    "push button",
    "button",
    "toggle button",
    "link",
    "page tab",
    "menu item",
    "radio button",
    "check box",
];

pub struct Input {
    pub backend: String,
    pub closing: Cell<bool>,
    position: Cell<Option<(f64, f64)>>,
    down: Cell<bool>,
    bus: Option<Bus>,
    remote: RefCell<String>,
}

impl Input {
    pub fn new(backend: &str) -> Res<Input> {
        let mut input = Input {
            backend: backend.into(),
            closing: Cell::new(false),
            position: Cell::new(None),
            down: Cell::new(false),
            bus: None,
            remote: RefCell::new(String::new()),
        };
        if backend == "wayland" {
            let bus = Bus::session()?;
            wait("Mutter remote input bus ready", || {
                Ok(bus
                    .call(
                        "org.freedesktop.DBus",
                        "/org/freedesktop/DBus",
                        "org.freedesktop.DBus",
                        "NameHasOwner",
                        Some(Arg::Str("org.gnome.Mutter.RemoteDesktop".into())),
                        1000,
                    )
                    .and_then(|v| v.child_bool(0))
                    .unwrap_or(false))
            })?;
            let session = bus
                .call(
                    "org.gnome.Mutter.RemoteDesktop",
                    "/org/gnome/Mutter/RemoteDesktop",
                    "org.gnome.Mutter.RemoteDesktop",
                    "CreateSession",
                    None,
                    5000,
                )?
                .child_string(0)?;
            let xml = bus
                .call(
                    "org.gnome.Mutter.RemoteDesktop",
                    &session,
                    "org.freedesktop.DBus.Introspectable",
                    "Introspect",
                    None,
                    5000,
                )?
                .child_string(0)?;
            for method in [
                "Start",
                "NotifyPointerMotionRelative",
                "NotifyPointerButton",
                "NotifyKeyboardKeysym",
            ] {
                require(
                    xml.contains(&format!("<method name=\"{method}\"")),
                    "Mutter RemoteDesktop session lacks a required method",
                )?;
            }
            bus.call(
                "org.gnome.Mutter.RemoteDesktop",
                &session,
                "org.gnome.Mutter.RemoteDesktop.Session",
                "Start",
                None,
                5000,
            )?;
            *input.remote.borrow_mut() = session;
            input.bus = Some(bus);
            // Create both virtual devices before GTK starts.
            input.move_to((WIDTH - 10) as f64, (HEIGHT - 10) as f64)?;
            input.key("Escape")?;
        }
        Ok(input)
    }

    fn notify(&self, method: &str, arg: Arg) -> Res<()> {
        let bus = self.bus.as_ref().ok_or("no Mutter session")?;
        bus.call(
            "org.gnome.Mutter.RemoteDesktop",
            &self.remote.borrow(),
            "org.gnome.Mutter.RemoteDesktop.Session",
            method,
            Some(arg),
            5000,
        )?;
        Ok(())
    }

    fn xdotool(args: &[&str]) -> Res<()> {
        let mut c = Command::new("xdotool");
        c.args(args).env("LC_ALL", "C.UTF-8");
        let done = capture(&mut c, std::time::Duration::from_secs(30))?;
        require(
            done.code == 0,
            &format!("xdotool {} failed", args.first().unwrap_or(&"")),
        )
    }

    pub fn move_to(&self, x: f64, y: f64) -> Res<()> {
        require(
            0.0 <= x && x < WIDTH as f64 && 0.0 <= y && y < HEIGHT as f64,
            "target outside screen",
        )?;
        if self.backend == "x11" {
            Self::xdotool(&[
                "mousemove",
                "--sync",
                &(x as i64).to_string(),
                &(y as i64).to_string(),
            ])?;
        } else {
            if self.position.get().is_none() || !self.down.get() {
                self.notify(
                    "NotifyPointerMotionRelative",
                    Arg::Doubles(10000.0, 10000.0),
                )?;
                sleep_ms(30);
                self.position
                    .set(Some(((WIDTH - 1) as f64, (HEIGHT - 1) as f64)));
            }
            let (px, py) = self.position.get().unwrap();
            self.notify("NotifyPointerMotionRelative", Arg::Doubles(x - px, y - py))?;
            self.position.set(Some((x, y)));
        }
        Ok(())
    }

    /// A relative pointer movement, for a page that reports where the pointer really is (Wayland has no absolute query).
    pub fn nudge(&self, dx: f64, dy: f64) -> Res<()> {
        if self.backend == "x11" {
            return Self::xdotool(&[
                "mousemove_relative",
                "--",
                &(dx.round() as i64).to_string(),
                &(dy.round() as i64).to_string(),
            ]);
        }
        self.notify("NotifyPointerMotionRelative", Arg::Doubles(dx, dy))
    }

    pub fn button(&self, down: bool) -> Res<()> {
        if self.backend == "x11" {
            Self::xdotool(&[if down { "mousedown" } else { "mouseup" }, "1"])?;
        } else {
            self.notify("NotifyPointerButton", Arg::IntBool(0x110, down))?;
        }
        self.down.set(down);
        Ok(())
    }

    pub fn click_at(&self, x: f64, y: f64) -> Res<()> {
        self.move_to(x, y)?;
        sleep_ms(100);
        self.button(true)?;
        sleep_ms(60);
        self.button(false)
    }

    pub fn key(&self, name: &str) -> Res<()> {
        if self.backend == "x11" {
            return Self::xdotool(&["key", "--clearmodifiers", name]);
        }
        let mut keys = Vec::new();
        for part in name.split('+') {
            let sym = match part {
                "Escape" => 0xff1b,
                "Return" => 0xff0d,
                "Tab" => 0xff09,
                "ctrl" => 0xffe3,
                "shift" => 0xffe1,
                "Home" => 0xff50,
                "End" => 0xff57,
                "Delete" => 0xffff,
                "alt" => 0xffe9,
                "super" => 0xffeb,
                "F10" => 0xffc7,
                "Up" => 0xff52,
                "Down" => 0xff54,
                "Left" => 0xff51,
                "Right" => 0xff53,
                "BackSpace" => 0xff08,
                single if single.chars().count() == 1 => single.chars().next().unwrap() as u32,
                _ => 0,
            };
            require(sym != 0, "unsupported key")?;
            keys.push(sym);
        }
        for sym in &keys {
            self.notify("NotifyKeyboardKeysym", Arg::UintBool(*sym, true))?;
        }
        for sym in keys.iter().rev() {
            self.notify("NotifyKeyboardKeysym", Arg::UintBool(*sym, false))?;
        }
        Ok(())
    }

    pub fn type_text(&self, text: &str) -> Res<()> {
        if self.backend == "x11" {
            return Self::xdotool(&["type", "--clearmodifiers", "--delay", "3", "--", text]);
        }
        for ch in text.chars() {
            let sym = if ch == '\n' {
                0xff0d
            } else if (ch as u32) <= 0xff {
                ch as u32
            } else {
                0x0100_0000 | ch as u32
            };
            self.notify("NotifyKeyboardKeysym", Arg::UintBool(sym, true))?;
            self.notify("NotifyKeyboardKeysym", Arg::UintBool(sym, false))?;
        }
        Ok(())
    }

    /// Put `text` on the OS clipboard through an independent owner, verify it, then paste.
    pub fn paste(&self, text: &str) -> Res<Child> {
        let home = PathBuf::from(std::env::var("HOME").map_err(|_| "HOME unset")?);
        let clipboard = if self.backend == "x11" {
            let ready = home.join(format!("clipboard-ready-{}", mono_ns()));
            let me = std::env::current_exe().map_err(|e| e.to_string())?;
            let child = Command::new(me)
                .args(["installed-clipboard-owner"])
                .arg(&ready)
                .arg(text)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?;
            wait("GTK OS clipboard owner", || Ok(ready.exists()))?;
            child
        } else {
            let mut child = Command::new("wl-copy")
                .args(["--foreground", "--type", "text/plain;charset=utf-8"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?;
            use std::io::Write;
            let mut stdin = child.stdin.take().unwrap();
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| e.to_string())?;
            drop(stdin);
            sleep_ms(400);
            child
        };
        let mut read = if self.backend == "x11" {
            let mut c = Command::new("xclip");
            c.args(["-selection", "clipboard", "-o"]);
            c
        } else {
            let mut c = Command::new("wl-paste");
            c.arg("--no-newline");
            c
        };
        let got = capture(&mut read, std::time::Duration::from_secs(5))?;
        require(got.code == 0 && got.stdout == text, "OS clipboard readback")?;
        self.key("ctrl+v")?;
        Ok(clipboard)
    }

    pub fn nodes_checked(&self, check_errors: bool) -> Res<Vec<Node>> {
        let desktop = Acc::desktop()?;
        let mut found: Vec<Node> = Vec::new();
        let mut stack: Vec<Acc> = Vec::new();
        let count = desktop.child_count()?;
        for i in 0..count {
            if let Some(app) = desktop.child(i)? {
                if app.name()?.contains("cortex-workspace") {
                    stack.push(app);
                }
            }
        }
        let mut seen = 0;
        while let Some(node) = stack.pop() {
            seen += 1;
            require(seen < 12000, "accessibility tree truncated")?;
            let step = (|| -> Res<()> {
                let st = node.states()?;
                if st[0] {
                    return Ok(());
                }
                if st[1] {
                    let coord = if self.backend == "x11" {
                        ffi::COORD_SCREEN
                    } else {
                        ffi::COORD_WINDOW
                    };
                    let interfaces = node.interfaces()?;
                    let (x, y, w, h) = if interfaces.iter().any(|i| i == "Component") {
                        node.extents(coord)?
                    } else {
                        (0, 0, 0, 0)
                    };
                    let text = if interfaces.iter().any(|i| i == "Text") {
                        node.text().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    found.push(Node {
                        role: node.role_name()?,
                        name: node.name()?,
                        text,
                        x: x as i64,
                        y: y as i64,
                        w: w as i64,
                        h: h as i64,
                        enabled: st[2],
                        focused: st[3],
                        selected: st[4],
                    });
                }
                let k = node.child_count()?;
                for i in (0..k).rev() {
                    if let Some(c) = node.child(i)? {
                        stack.push(c);
                    }
                }
                Ok(())
            })();
            if let Err(error) = step {
                // A page update may invalidate one read, never an input action.
                let low = error.to_lowercase();
                let stale = low.contains("does not exist")
                    || low.contains("application no longer exists")
                    || (low.contains("no such interface") && error.contains("org.a11y.atspi."));
                let closed = self.closing.get() && low.contains("disconnected from message bus");
                if !(stale || closed) {
                    return Err(error);
                }
            }
        }
        if check_errors {
            require(
                !found
                    .iter()
                    .any(|n| n.role == "alert" && (!n.name.is_empty() || !n.text.is_empty())),
                "visible error alert",
            )?;
            require(
                !found.iter().any(|n| {
                    n.both().contains(
                        "This task changed on the machine. Refresh it before trying again.",
                    )
                }),
                "visible stale-action error",
            )?;
        }
        Ok(found)
    }

    pub fn nodes(&self) -> Res<Vec<Node>> {
        self.nodes_checked(true)
    }

    pub fn find(&self, name: &str, roles: &[&str], contains: bool) -> Res<Option<Node>> {
        Ok(self.nodes()?.into_iter().find(|n| {
            (if contains {
                n.name.contains(name)
            } else {
                n.name == name
            }) && (roles.is_empty() || roles.contains(&n.role.as_str()))
                && n.w > 0
                && n.h > 0
        }))
    }

    /// The button named exactly `name` inside the region named `region` (by geometry: the flat node list keeps no
    /// parents), and only if no other showing button anywhere has that name, so a stale match elsewhere never counts.
    pub fn find_in(&self, region: &str, name: &str) -> Res<Option<Node>> {
        let nodes = self.nodes()?;
        let Some(area) = nodes
            .iter()
            .find(|n| n.name == region && n.w > 0 && n.h > 0)
        else {
            return Ok(None);
        };
        let buttons: Vec<&Node> = nodes
            .iter()
            .filter(|n| {
                n.name == name && DEFAULT_ROLES.contains(&n.role.as_str()) && n.w > 0 && n.h > 0
            })
            .collect();
        let inside = |n: &Node| {
            n.x >= area.x
                && n.y >= area.y
                && n.x + n.w <= area.x + area.w
                && n.y + n.h <= area.y + area.h
        };
        Ok(match buttons.as_slice() {
            [only] if inside(only) => Some((*only).clone()),
            _ => None,
        })
    }

    pub fn has_in(&self, region: &str, name: &str) -> Res<bool> {
        Ok(self.find_in(region, name)?.is_some())
    }

    /// Whether a showing button named exactly `name` exists anywhere.
    pub fn any_button(&self, name: &str) -> Res<bool> {
        Ok(self.nodes()?.iter().any(|n| {
            n.name == name && DEFAULT_ROLES.contains(&n.role.as_str()) && n.w > 0 && n.h > 0
        }))
    }

    pub fn click_in(&self, region: &str, name: &str) -> Res<()> {
        let node = wait_with(&format!("enabled control {name} in {region}"), 30, || {
            Ok(self.find_in(region, name)?.filter(|n| n.enabled))
        })?;
        self.click_at(
            node.x as f64 + node.w as f64 / 2.0,
            node.y as f64 + node.h as f64 / 2.0,
        )
    }

    pub fn has(&self, name: &str, roles: &[&str]) -> Res<bool> {
        Ok(self.find(name, roles, false)?.is_some())
    }

    pub fn seen(&self, text: &str) -> Res<bool> {
        Ok(self.nodes()?.iter().any(|n| n.both().contains(text)))
    }

    pub fn click(&self, name: &str, roles: &[&str], contains: bool) -> Res<()> {
        let roles: &[&str] = if roles.is_empty() {
            &DEFAULT_ROLES
        } else {
            roles
        };
        let node = wait_with(&format!("enabled control {name}"), 30, || {
            Ok(self.find(name, roles, contains)?.filter(|n| n.enabled))
        })?;
        self.click_at(
            node.x as f64 + node.w as f64 / 2.0,
            node.y as f64 + node.h as f64 / 2.0,
        )
    }

    pub fn fill(&self, name: &str, text: &str) -> Res<()> {
        self.click(name, &["entry", "text", "combo box"], false)?;
        wait(&format!("field focus {name}"), || {
            Ok(self.nodes()?.iter().any(|n| n.name == name && n.focused))
        })?;
        self.key("ctrl+a")?;
        self.type_text(text)
    }

    /// Repeated geometry/text changes of a hovered stationary control are a regression;
    /// paint-only flicker is not observable through accessibility.
    pub fn stable(&self, name: &str) -> Res<()> {
        let node = wait_with(&format!("stable {name}"), 30, || {
            self.find(name, &[], false)
        })?;
        self.move_to(
            node.x as f64 + node.w as f64 / 2.0,
            node.y as f64 + node.h as f64 / 2.0,
        )?;
        let tuple = |q: &Node| arr(vec![s(&q.name), n(q.x), n(q.y), n(q.w), n(q.h)]);
        let key = |n: &Node| (n.name.clone(), n.x, n.y, n.w, n.h);
        let expected = tuple(&node);
        let started = mono_ns();
        let notices: Vec<Value> = self
            .nodes_checked(false)?
            .iter()
            .filter(|n| {
                n.role == "status"
                    || n.role == "alert"
                    || n.both().contains("Updating confirmed state")
            })
            .map(Node::json)
            .collect();
        emit_row(
            "stability_baseline",
            vec![
                ("backend", s(&self.backend)),
                ("expected", expected.clone()),
                ("notices", arr(notices)),
                ("monotonic_ns", n(started)),
            ],
        );
        for sample in 0..8 {
            sleep_ms(150);
            let current = self.find(name, &[], false)?;
            let observed = mono_ns();
            let actual = current.as_ref().map(tuple).unwrap_or(Value::Null);
            emit_row(
                "stability_sample",
                vec![
                    ("backend", s(&self.backend)),
                    ("expected", expected.clone()),
                    ("actual", actual.clone()),
                    ("monotonic_ns", n(observed)),
                    ("elapsed_ns", n(observed - started)),
                    ("sample", n(sample)),
                ],
            );
            if current.as_ref().map(key) != Some(key(&node)) {
                let nearby: Vec<Value> = self
                    .nodes_checked(false)?
                    .iter()
                    .filter(|n| {
                        ["link", "frame", "alert", "status", "push button", "button"]
                            .contains(&n.role.as_str())
                    })
                    .map(Node::json)
                    .collect();
                emit_row(
                    "stability_failure_context",
                    vec![("monotonic_ns", n(mono_ns())), ("nodes", arr(nearby))],
                );
                return Err(format!(
                    "blink/shift: {name}: expected={}, actual={}",
                    line(&expected),
                    line(&actual)
                ));
            }
        }
        Ok(())
    }
}

fn emit_row(step: &str, pairs: Vec<(&str, Value)>) {
    emit(step, pairs);
}
