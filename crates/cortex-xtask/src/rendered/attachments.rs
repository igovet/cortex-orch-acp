//! The attachment stage: real OS transfer into the New task composer and delivery to the no-paid peer.
//!
//! A fixture file is dragged from a separate GTK window (the installed gate's `installed-drag-source`) onto the composer with the
//! real pointer; a PNG is put on the OS clipboard by an independent owner (`xclip` on X11, `wl-copy` on Wayland), read back byte
//! for byte and pasted with the real Ctrl+V; the Attach button opens the system file chooser, driven with the real keyboard. The
//! draft is saved, its files are read back on the task page, and Start must deliver every file to the engine: the peer records
//! each prompt it receives (`record-prompts`), and the project store must retain each file by name and SHA-256 while no event
//! carries its bytes. Only a host that reports attachments unavailable makes the stage `not_applicable`, with the page's own
//! words; a surface the page offers that does not work is a failure.

use super::journey::*;
use super::session::*;
use crate::installed::util::*;
use crate::json::{parse, Res, Value};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TITLE: &str = "Attachment delivery check";
const DROP_NAME: &str = "rendered-drop.txt";
const DROP_BODY: &str = "Rendered drop fixture body";
const PICK_NAME: &str = "rendered-pick.txt";
const PICK_BODY: &str = "Rendered picker fixture body";
const SOURCE_TITLE: &str = "Installed attachment fixture";
const COMPOSER: &str = "document.querySelector('.create .composer')";

/// The CRC-32 of a PNG chunk (ISO 3309, reflected 0xEDB88320).
pub(super) fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &byte in data {
        c ^= byte as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// A valid 4x4 RGB PNG: one stored zlib block, so no compressor is needed and the bytes are fixed.
pub(super) fn png_fixture() -> Vec<u8> {
    let (w, h) = (4u32, 4u32);
    let mut raw = Vec::new();
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            raw.extend_from_slice(&[(x * 60) as u8, (y * 60) as u8, 0x80]);
        }
    }
    let mut z = vec![0x78, 0x01, 0x01];
    let len = raw.len() as u16;
    z.extend(len.to_le_bytes());
    z.extend((!len).to_le_bytes());
    z.extend(&raw);
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [
        (&b"IHDR"[..], ihdr),
        (&b"IDAT"[..], z),
        (&b"IEND"[..], Vec::new()),
    ] {
        png.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(&data);
        png.extend(&body);
        png.extend(crc32(&body).to_be_bytes());
    }
    png
}

/// Standard base64 with padding, as the service and the page encode file bytes.
pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let v = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(v >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Whether one prompt block carries `body` as an attachment: an embedded text resource, or the service's framed text form.
fn block_carries(block: &Value, body: &str) -> bool {
    let resource = block
        .get("resource")
        .and_then(|r| r.get("text"))
        .and_then(Value::as_str);
    let framed = block
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|t| t.contains(&format!("\n{body}\n")));
    resource == Some(body) || framed
}

/// The first recorded prompt that carries the exact PNG as an image block, and which of `bodies` that same prompt carries. Each
/// log line is `{"session": .., "prompt": [blocks]}`; an unreadable line is named, never skipped.
pub(super) fn delivered(log: &str, png: &str, bodies: &[&str]) -> Res<Option<(usize, Vec<bool>)>> {
    for (index, line) in log
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let entry = parse(line).map_err(|e| format!("prompt_log_line_{index}_unreadable:{e}"))?;
        let blocks = entry.get("prompt").and_then(Value::as_arr).unwrap_or(&[]);
        let image = blocks.iter().any(|b| {
            b.get("type").and_then(Value::as_str) == Some("image")
                && b.get("mimeType").and_then(Value::as_str) == Some("image/png")
                && b.get("data").and_then(Value::as_str) == Some(png)
        });
        if image {
            return Ok(Some((
                index,
                bodies
                    .iter()
                    .map(|body| blocks.iter().any(|b| block_carries(b, body)))
                    .collect(),
            )));
        }
    }
    Ok(None)
}

/// Runs a reader with its standard output in `into`, bounded; the exit code.
fn read_into(command: &mut Command, into: &Path, seconds: u64) -> Res<i32> {
    let sink = std::fs::File::create(into).map_err(|e| e.to_string())?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(sink)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(status.code().unwrap_or(-1));
        }
        if Instant::now() > end {
            let _ = child.kill();
            let _ = child.wait();
            return Err("clipboard_readback_timed_out".into());
        }
        sleep_ms(30);
    }
}

fn end_child(mut child: Child) {
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let _ = child.wait();
}

impl J {
    fn composer(&mut self) -> Res<Value> {
        self.q(&format!("const c = {COMPOSER}; if (!c) return JSON.stringify(null); return JSON.stringify({{chips: [...c.querySelectorAll('.ws-att')].map(e => ({{name: e.querySelector('.trunc')?.textContent ?? '', image: !!e.querySelector('img.ws-att-thumb')}})), alert: c.querySelector('[role=alert]')?.textContent ?? '', busy: [...c.querySelectorAll('[role=status]')].some(s => s.textContent.includes('Reading attachments')), value: c.querySelector('textarea')?.value ?? ''}});"))
    }

    fn chip_names(&mut self) -> Res<Vec<String>> {
        let state = self.composer()?;
        Ok(at(&state, "chips")
            .and_then(Value::as_arr)
            .unwrap_or(&[])
            .iter()
            .map(|c| text(c, "name").to_string())
            .collect())
    }

    /// Waits until the composer shows a chip `wanted` accepts and is no longer reading; the composer's state either way.
    fn wait_chip(&mut self, seconds: u64, wanted: impl Fn(&Value) -> bool) -> (bool, Value) {
        let end = Instant::now() + Duration::from_secs(seconds);
        let mut last = Value::Null;
        while Instant::now() < end {
            if let Ok(state) = self.composer() {
                let found = at(&state, "chips")
                    .and_then(Value::as_arr)
                    .is_some_and(|c| c.iter().any(&wanted))
                    && !flag(&state, "busy");
                last = state;
                if found {
                    return (true, last);
                }
            }
            sleep_ms(100);
        }
        (false, last)
    }

    /// The centre of the first element `selector` matches, in page coordinates.
    fn page_centre(&mut self, selector: &str) -> Res<(f64, f64)> {
        let q = crate::json::quote(selector, true);
        let v = self.q(&format!("const e = document.querySelector({q}); if (!e) return JSON.stringify(null); const r = e.getBoundingClientRect(); return JSON.stringify([Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)]);"))?;
        require(v.as_arr().is_some(), &format!("element_missing:{selector}"))?;
        Ok((num(&v, "0"), num(&v, "1")))
    }

    /// The on-screen point of page point (`cx`, `cy`): estimated from the window's origin, then corrected by where the page
    /// reports the real pointer landed. A point the pointer cannot reach over the page is an error, never a guess.
    fn screen_point(&mut self, origin: (f64, f64), cx: f64, cy: f64) -> Res<(f64, f64)> {
        self.js("if (!window.__cwMoveArmed) { window.__cwMoveArmed = true; document.addEventListener('pointermove', e => { window.__cwMove = [Math.round(e.clientX), Math.round(e.clientY)]; }, true); } window.__cwMove = null; return 'armed';")?;
        let (mut px, mut py) = (origin.0 + cx, origin.1 + cy);
        for _ in 0..4 {
            self.js("window.__cwMove = null; return 'yes';")?;
            let input = self.st.input.as_ref().ok_or("no pointer")?;
            input.move_to(px - 3.0, py - 3.0)?;
            sleep_ms(120);
            input.move_to(px, py)?;
            sleep_ms(200);
            let landed = self.q("return JSON.stringify(window.__cwMove);")?;
            if landed.as_arr().is_none() {
                return Err(format!("pointer_not_over_page:{px},{py}"));
            }
            let (mx, my) = (num(&landed, "0"), num(&landed, "1"));
            if (mx - cx).abs() <= 1.0 && (my - cy).abs() <= 1.0 {
                return Ok((px, py));
            }
            px += cx - mx;
            py += cy - my;
        }
        Err("pointer_calibration_failed".into())
    }

    /// Where the owned window's page starts on screen: the X11 geometry, or on Wayland the right half the window was tiled to.
    fn page_origin(&mut self) -> Res<(f64, f64)> {
        if self.st.wayland {
            return Ok(((MONITOR.0 / 2) as f64, 0.0));
        }
        let window = self.st.window_id()?;
        let g = geometry(&window)?;
        Ok((g[0] as f64, g[1] as f64))
    }

    fn native_width(&mut self) -> Res<i64> {
        Ok(num(&self.q("@native")?, "top.0") as i64)
    }

    /// A real OS file drag from a separate GTK window onto the composer. Source and target are kept side by side: on Wayland the
    /// compositor's own tiling keys place them (no client positioning exists there), on X11 the source asks for its place.
    fn drop_file(&mut self, fixtures: &Path) -> Res<bool> {
        let fixture = fixtures.join(DROP_NAME);
        let ready = fixtures.join("drag-ready");
        write(&fixture, DROP_BODY)?;
        let backend = if self.st.wayland { "wayland" } else { "x11" };
        if self.st.wayland {
            self.key("super+Right")?;
            let half = MONITOR.0 / 2;
            self.wait("attachment_app_tiled_right", 15, |j| {
                Ok(j.native_width()? == half)
            })?;
            sleep_ms(400);
        }
        let log = std::fs::File::create(fixtures.join("drag.log")).map_err(|e| e.to_string())?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let drag = Command::new(me)
            .arg("installed-drag-source")
            .arg(&fixture)
            .arg(&ready)
            .arg(backend)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log2)
            .spawn()
            .map_err(|e| e.to_string())?;
        self.drag_from(drag, &fixture, &ready)
    }

    fn drag_from(&mut self, drag: Child, fixture: &Path, ready: &Path) -> Res<bool> {
        let outcome = (|| -> Res<(f64, f64, f64, f64)> {
            sleep_ms(1000);
            let (sx, sy) = if self.st.wayland {
                // The new window has focus: tile it to the left half.
                self.key("super+Left")?;
                ((MONITOR.0 / 4) as f64, (MONITOR.1 / 2) as f64)
            } else {
                let found = xdotool(&["search", "--name", &format!("^{SOURCE_TITLE}$")])?;
                let wid = found
                    .lines()
                    .last()
                    .ok_or("drag_source_window_missing")?
                    .to_string();
                let g = geometry(&wid)?;
                ((g[0] + g[2] / 2) as f64, (g[1] + g[3] / 2) as f64)
            };
            wait_secs("attachment_drag_source_visible", 20, || Ok(ready.exists()))?;
            let origin = self.page_origin()?;
            let (cx, cy) = self.page_centre("[aria-label=\"Describe the task\"]")?;
            let (tx, ty) = self.screen_point(origin, cx, cy)?;
            let input = self.st.input.as_ref().ok_or("no pointer")?;
            input.move_to(sx, sy)?;
            sleep_ms(150);
            input.button(true)?;
            sleep_ms(100);
            input.move_to(sx + 20.0, sy + 20.0)?;
            sleep_ms(200);
            for step in 1..=20 {
                let f = step as f64 / 20.0;
                input.move_to(sx + (tx - sx) * f, sy + (ty - sy) * f)?;
                sleep_ms(40);
            }
            sleep_ms(500);
            input.button(false)?;
            Ok((sx, sy, tx, ty))
        })();
        let (shown, state) = if outcome.is_ok() {
            self.wait_chip(15, |c| text(c, "name") == DROP_NAME)
        } else {
            (false, Value::Null)
        };
        end_child(drag);
        let markers: Vec<Value> = ["began", "requested", "ended"]
            .iter()
            .map(|m| b(ready.with_extension(m).exists()))
            .collect();
        let (sx, sy, tx, ty) = outcome?;
        // Real focus back on the composer, by the pointer, for what follows.
        self.st
            .input
            .as_ref()
            .ok_or("no pointer")?
            .click_at(tx, ty)?;
        sleep_ms(300);
        let untouched = text(&state, "value") == TITLE;
        let detail = vec![
            ("source", arr_i(&[sx as i64, sy as i64])),
            ("target", arr_i(&[tx as i64, ty as i64])),
            ("drag_began_requested_ended", arr(markers)),
            ("text_unchanged", b(untouched)),
            (
                "fixture",
                s(fixture
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()),
            ),
            ("composer", state),
        ];
        self.check("attachment_os_file_drop", shown && untouched, detail);
        Ok(shown && untouched)
    }

    /// A PNG on the OS clipboard from an independent owner, read back byte for byte, then the real Ctrl+V into the composer.
    fn paste_image(&mut self, fixtures: &Path, png: &[u8]) -> Res<bool> {
        let image = fixtures.join("clipboard-fixture.png");
        std::fs::write(&image, png).map_err(|e| e.to_string())?;
        let before = self.chip_names()?.len();
        let owner = if self.st.wayland {
            let source = std::fs::File::open(&image).map_err(|e| e.to_string())?;
            Command::new("wl-copy")
                .args(["--foreground", "--type", "image/png"])
                .stdin(source)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("wl-copy: {e}"))?
        } else {
            Command::new("xclip")
                .args([
                    "-quiet",
                    "-selection",
                    "clipboard",
                    "-target",
                    "image/png",
                    "-in",
                ])
                .arg(&image)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("xclip: {e}"))?
        };
        self.keep.push(owner);
        let back = fixtures.join("clipboard-readback.png");
        let mut matched = false;
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end && !matched {
            sleep_ms(200);
            let mut reader = if self.st.wayland {
                let mut c = Command::new("wl-paste");
                c.args(["--no-newline", "--type", "image/png"]);
                c
            } else {
                let mut c = Command::new("xclip");
                c.args(["-selection", "clipboard", "-target", "image/png", "-out"]);
                c
            };
            matched = read_into(&mut reader, &back, 5)? == 0
                && std::fs::read(&back).map_err(|e| e.to_string())? == png;
        }
        require(matched, "clipboard_image_readback_mismatch")?;
        let focused = self.yes("return document.activeElement?.getAttribute('aria-label') === 'Describe the task' ? 'yes' : '';")?;
        require(focused, "composer_not_focused_for_paste")?;
        self.key("ctrl+v")?;
        let (shown, state) = self.wait_chip(15, |c| {
            flag(c, "image") && text(c, "name").ends_with(".png")
        });
        self.release_clipboards();
        let added = count(&state, "chips") > before;
        let untouched = text(&state, "value") == TITLE;
        self.check(
            "attachment_clipboard_image",
            shown && added && untouched,
            vec![
                (
                    "clipboard_owner",
                    s(if self.st.wayland {
                        "wl-copy image/png"
                    } else {
                        "xclip image/png"
                    }),
                ),
                ("readback_bytes", n(png.len() as i64)),
                ("text_unchanged", b(untouched)),
                ("composer", state),
            ],
        );
        Ok(shown && added && untouched)
    }

    /// The Attach button with the real pointer, then the system file chooser it opens, driven with the real keyboard; whether the
    /// picked file was attached. The chooser is observable on both displays, so an enabled Attach that opens none is a failure.
    fn pick_file(&mut self, fixtures: &Path) -> Res<bool> {
        let fixture = fixtures.join(PICK_NAME);
        write(&fixture, PICK_BODY)?;
        let origin = self.page_origin()?;
        let (cx, cy) = self.page_centre(".create .composer [aria-label=\"Attach files\"]")?;
        let (px, py) = self.screen_point(origin, cx, cy)?;
        self.st
            .input
            .as_ref()
            .ok_or("no pointer")?
            .click_at(px, py)?;
        let group = self.group();
        let chooser = wait_with("attachment_file_chooser", 15, || {
            Ok(super::a11y::tree(group)
                .into_iter()
                .find(|n| n.role == "file chooser")
                .map(|n| n.name))
        })
        .ok();
        let Some(name) = chooser else {
            let roles: Vec<Value> = super::a11y::tree(group)
                .into_iter()
                .filter(|n| matches!(n.role.as_str(), "dialog" | "frame" | "window" | "alert"))
                .map(|n| s(format!("{}:{}", n.role, n.name)))
                .take(12)
                .collect();
            self.fail("attachment_file_picker", vec![("reason", s("no file chooser appeared in the owned app's accessibility tree after a real pointer press on an enabled Attach files")), ("toplevels", arr(roles))]);
            return Ok(false);
        };
        sleep_ms(500);
        self.key("ctrl+l")?;
        sleep_ms(300);
        self.type_text(&fixture.to_string_lossy())?;
        sleep_ms(400);
        self.key("Return")?;
        let (shown, state) = self.wait_chip(15, |c| text(c, "name") == PICK_NAME);
        let closed = wait_secs("attachment_file_chooser_closed", 10, || {
            Ok(!super::a11y::tree(group)
                .iter()
                .any(|n| n.role == "file chooser"))
        })
        .is_ok();
        let untouched = text(&state, "value") == TITLE;
        self.check(
            "attachment_file_picker",
            shown && closed && untouched,
            vec![
                ("chooser", s(name)),
                ("chooser_closed", b(closed)),
                ("text_unchanged", b(untouched)),
                ("composer", state),
            ],
        );
        Ok(shown && closed && untouched)
    }

    /// Attachments are offered only when the host supports them. When the page says they are unavailable the paste, picker and
    /// drop surfaces cannot be exercised: that is recorded as not applicable with the page's own words, never as a pass. When it
    /// offers them, every surface the harness can drive must attach its file, and Start must deliver each one to the engine.
    pub fn attachments_stage(&mut self) -> Res<()> {
        let project = self.home.join("attachments");
        let fixtures = self.home.join("attachment-fixtures");
        std::fs::create_dir_all(&fixtures).map_err(|e| e.to_string())?;
        // The no-paid peer appends every prompt it receives to `prompt-log.jsonl` in its working directory, the project folder.
        write(&project.join("record-prompts"), "")?;
        self.launch("dark")?;
        self.wait_found("attachments_window", 15, "Add project")?;
        self.loaded()?;
        self.add_project(&project)?;
        self.wait_found("attachments_registered", 15, "Activate")?;
        self.click("Activate")?;
        self.wait_gone("attachments_activated", 60, "Activate")?;
        self.click("New task")?;
        self.wait_found("attachments_form", 15, "Describe the task")?;
        let words = self.q("return JSON.stringify([...document.querySelectorAll('.create [aria-label=\"Attach files\"]')].map(b => { const id = b.getAttribute('aria-describedby'); const why = (id ? document.getElementById(id)?.textContent.trim() : '') || b.dataset.why || ''; return {disabled: b.getAttribute('aria-disabled') === 'true' || b.disabled, words: why}; }));")?;
        let controls = words.as_arr().unwrap_or(&[]).to_vec();
        let offered = controls.iter().any(|c| !flag(c, "disabled"));
        let unavailable = !controls.is_empty()
            && controls.iter().all(|c| {
                flag(c, "disabled") && text(c, "words").to_lowercase().contains("not available")
            });
        if unavailable {
            let page = text(&controls[0], "words").to_string();
            self.not_applicable("native_attachments", "the host reports attachments unavailable: the file picker, drag-and-drop and clipboard paste shapes cannot be exercised", s(page));
            return self.close_window();
        }
        if !offered {
            self.fail(
                "native_attachments",
                vec![
                    (
                        "reason",
                        s("no usable attachment control was found on the task form"),
                    ),
                    ("controls", arr(controls)),
                ],
            );
            return self.close_window();
        }
        self.fill("Describe the task", TITLE)?;
        let png = png_fixture();
        let dropped = self.drop_file(&fixtures)?;
        let pasted = self.paste_image(&fixtures, &png)?;
        let picker = self.pick_file(&fixtures)?;
        let names = self.chip_names()?;
        // Whatever the composer holds is what Start must deliver: a picked file that attached counts even if its step failed.
        let picked = names.iter().any(|n| n == PICK_NAME);
        let image_name = names
            .iter()
            .find(|n| n.ends_with(".png"))
            .cloned()
            .unwrap_or_default();
        let mut expected: Vec<(String, Vec<u8>)> = vec![
            (DROP_NAME.into(), DROP_BODY.as_bytes().to_vec()),
            (image_name.clone(), png.clone()),
        ];
        if picked {
            expected.push((PICK_NAME.into(), PICK_BODY.as_bytes().to_vec()));
        }
        let all_attached = expected
            .iter()
            .all(|(name, _)| !name.is_empty() && names.contains(name))
            && names.len() == expected.len();
        require(
            all_attached,
            &format!("attachments_not_all_attached:{}", names.join(",")),
        )?;
        self.click("Save draft")?;
        self.wait_text("attachments_draft_saved", 15, TITLE)?;
        self.open_task("attachments_task_link", TITLE)?;
        // The saved draft reads its files back from the service, each one openable.
        let wanted: Vec<String> = expected
            .iter()
            .map(|(name, _)| format!("Open {name}"))
            .collect();
        let list = crate::json::quote(&wanted.join("\n"), true);
        let read_back = self.wait_js("attachments_draft_read_back", 20, &format!("const want = {list}.split('\\n'); const have = [...document.querySelectorAll('.ws-att-open')].map(b => b.getAttribute('aria-label')); return want.every(w => have.includes(w)) ? 'yes' : '';")).is_ok();
        let shown = self.q("return JSON.stringify([...document.querySelectorAll('.ws-att button')].map(b => b.getAttribute('aria-label')));")?;
        self.check(
            "attachments_saved_with_draft",
            read_back,
            vec![
                ("expected", arr(wanted.iter().map(s).collect())),
                ("shown", shown),
            ],
        );
        require(read_back, "attachments_draft_read_back")?;
        self.click_task("Start")?;
        let log = project.join("prompt-log.jsonl");
        let png64 = base64(&png);
        let bodies: Vec<&str> = if picked {
            vec![DROP_BODY, PICK_BODY]
        } else {
            vec![DROP_BODY]
        };
        let mut found = None;
        let end = Instant::now() + Duration::from_secs(60);
        while Instant::now() < end {
            // Only complete lines: the peer may be appending one right now.
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let whole = &text[..text.rfind('\n').map_or(0, |i| i + 1)];
            if let Some(hit) = delivered(whole, &png64, &bodies)? {
                found = Some(hit);
                break;
            }
            sleep_ms(200);
        }
        // The peer's recorder appends to the project folder and treats a failed write as fatal; the reviewer that follows works
        // read-only there, so recording stops once the coordinator's prompt is in hand.
        std::fs::remove_file(project.join("record-prompts"))
            .map_err(|e| format!("record-prompts: {e}"))?;
        let prompts = std::fs::read_to_string(&log)
            .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0);
        let texts_ok = found.as_ref().is_some_and(|(_, t)| t.iter().all(|x| *x));
        let detail = vec![
            ("prompts_recorded", n(prompts as i64)),
            (
                "image_block_prompt",
                found.as_ref().map_or(Value::Null, |(i, _)| n(*i as i64)),
            ),
            (
                "text_files_in_same_prompt",
                found
                    .as_ref()
                    .map_or(Value::Null, |(_, t)| arr(t.iter().map(|x| b(*x)).collect())),
            ),
        ];
        self.check("attachments_delivered_to_engine", texts_ok, detail);
        // The store retains each file by name and digest; no event carries the bytes.
        let db = self.channel.store_db(&project);
        let rows = crate::installed::native::query(&db, "SELECT name||'|'||sha256||'|'||size_bytes FROM source_blocks WHERE availability='stored' ORDER BY block_index")?;
        let retained: Vec<&str> = rows.lines().collect();
        let all_retained = expected.iter().all(|(name, bytes)| {
            retained.contains(&format!("{name}|{}|{}", sha_bytes(bytes), bytes.len()).as_str())
        });
        let leak = self.store_count(
            &project,
            &format!(
                "SELECT count(*) FROM runtime_events WHERE instr(payload_json, '{}') > 0",
                &png64[..32]
            ),
        )?;
        self.check(
            "attachments_retained_in_store",
            all_retained && leak == 0,
            vec![
                (
                    "stored_rows",
                    arr(retained
                        .iter()
                        .map(|r| s(r.split('|').next().unwrap_or("")))
                        .collect()),
                ),
                ("expected", n(expected.len() as i64)),
                ("events_with_file_bytes", n(leak)),
            ],
        );
        let state = self
            .wait("attachments_task_completed", 120, |j| {
                Ok(j.task_state(&project)?.as_deref() == Some("COMPLETED"))
            })
            .map(|_| "COMPLETED".to_string())
            .unwrap_or_else(|_| self.task_state(&project).ok().flatten().unwrap_or_default());
        if state != "COMPLETED" {
            // Event types, retry classes and request operations only, never payloads: where the task stopped.
            let events = crate::installed::native::query(&db, "SELECT type||coalesce(' '||json_extract(payload_json,'$.failure_class'),'') FROM runtime_events ORDER BY rowid").unwrap_or_else(|e| e);
            let requests = crate::installed::native::query(
                &db,
                "SELECT operation FROM runtime_requests ORDER BY rowid",
            )
            .unwrap_or_else(|e| e);
            self.note(
                "attachments_task_trail",
                vec![
                    ("events", arr(events.lines().map(s).collect())),
                    ("requests", arr(requests.lines().map(s).collect())),
                ],
            );
        }
        self.check(
            "attachments_task_completed",
            state == "COMPLETED",
            vec![("state", s(state.clone()))],
        );
        let ok = dropped
            && pasted
            && picker
            && texts_ok
            && all_retained
            && leak == 0
            && state == "COMPLETED";
        self.check(
            "native_attachments",
            ok,
            vec![
                ("drop", b(dropped)),
                ("clipboard_image", b(pasted)),
                ("file_picker", b(picker)),
                ("draft_read_back", b(read_back)),
                ("delivered", b(texts_ok)),
                ("retained", b(all_retained)),
                ("completed", b(state == "COMPLETED")),
                ("image_name", s(image_name)),
            ],
        );
        self.close_window()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_chunks_carry_their_crc_and_the_signature() {
        // The well-known CRC of an empty IEND chunk.
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        let png = png_fixture();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]));
        // IHDR: 13 bytes, 4x4, 8-bit RGB.
        assert_eq!(&png[8..16], &[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        assert_eq!(&png[16..29], &[0, 0, 0, 4, 0, 0, 0, 4, 8, 2, 0, 0, 0]);
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b""), "");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn delivery_needs_the_exact_image_and_reports_each_text_file() {
        let log = concat!(
            r#"{"session":"a","prompt":[{"type":"text","text":"title only"}]}"#,
            "\n",
            r#"{"session":"b","prompt":[{"type":"text","text":"Task"},{"type":"image","mimeType":"image/png","data":"QUJD"},{"type":"resource","resource":{"uri":"cortex:attachment/x","mimeType":"text/plain","text":"drop body"}},{"type":"text","text":"[attachment: p.txt]\npick body\n[end of attachment]"}]}"#,
            "\n",
        );
        assert_eq!(
            delivered(log, "QUJD", &["drop body", "pick body", "absent"]).unwrap(),
            Some((1, vec![true, true, false]))
        );
        assert_eq!(delivered(log, "QUJE", &["drop body"]).unwrap(), None);
        let wrong_type =
            r#"{"session":"b","prompt":[{"type":"image","mimeType":"image/jpeg","data":"QUJD"}]}"#;
        assert_eq!(delivered(wrong_type, "QUJD", &[]).unwrap(), None);
        // A body that only appears inside another word is not an attachment.
        let loose = r#"{"session":"b","prompt":[{"type":"image","mimeType":"image/png","data":"QUJD"},{"type":"text","text":"xx drop body yy"}]}"#;
        assert_eq!(
            delivered(loose, "QUJD", &["drop body"]).unwrap(),
            Some((0, vec![false]))
        );
        assert!(delivered("{not json\n", "QUJD", &[]).is_err());
    }
}
