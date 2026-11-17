//! The owner journey: the shipped desktop binary driven through its accessibility tree, the real keyboard and pointer, and the
//! no-paid native peer, one disposable account per run.
//!
//! Everything is a step that prints one JSON line. A step that fails is recorded and the journey carries on when it can; a step
//! that cannot proceed ends the journey as `journey_incomplete`, with how each process really ended. Nothing here passes a stage
//! it could not run: a capability the host does not offer is `not_applicable` with the page's own words, never a pass.

use super::a11y;
use super::session::*;
use crate::installed::channel::Channel;
use crate::installed::native;
use crate::installed::util::*;
use crate::json::{Res, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Which window-close requests the harness made and how the product ended each web process.
pub(super) fn terminations_unexpected(terminations: &Value, closes: u32) -> Value {
    let all: Vec<&str> = terminations.as_arr().unwrap_or(&[]).iter().filter_map(Value::as_str).collect();
    let by_api = all.iter().filter(|r| **r == "terminated_by_api").count();
    // Exactly one `terminated_by_api` per requested close, nothing looser: any other reason, a missing one or an extra one is a failure.
    if all.iter().all(|r| *r == "terminated_by_api") && by_api == closes as usize {
        return arr(vec![]);
    }
    if all.iter().all(|r| *r == "terminated_by_api") {
        let mut found = vec![s(format!("terminated_by_api_count_mismatch:{by_api}_for_{closes}_requested_closes"))];
        found.extend(all.iter().map(|r| s(*r)));
        return arr(found);
    }
    arr(all.iter().map(|r| s(*r)).collect())
}

pub(super) struct J {
    pub run: Run,
    pub st: Stack,
    pub home: PathBuf,
    pub channel: Channel,
    pub keep: Vec<Child>,
    pub only: Option<String>,
}

pub(super) fn outcome_pairs(value: &Value) -> Vec<(&str, Value)> {
    value.as_obj().unwrap_or(&[]).iter().map(|(k, v)| (k.as_str(), v.clone())).collect()
}

impl J {
    // ------------------------------------------------------------ the page

    pub fn group(&self) -> i32 {
        self.st.app.as_ref().map_or(-1, |a| a.id() as i32)
    }

    pub fn js(&mut self, body: &str) -> Res<String> {
        self.st.js(body, 10)
    }

    pub fn jsn(&mut self, body: &str, seconds: u64) -> Res<String> {
        self.st.js(body, seconds)
    }

    pub fn q(&mut self, body: &str) -> Res<Value> {
        self.st.query(body, 10)
    }

    pub fn yes(&mut self, body: &str) -> Res<bool> {
        Ok(self.js(body)? == "yes")
    }

    pub fn wait(&mut self, step: &str, seconds: u64, mut check: impl FnMut(&mut J) -> Res<bool>) -> Res<()> {
        let end = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < end {
            if check(self)? {
                return Ok(());
            }
            sleep_ms(50);
        }
        Err(step.to_string())
    }

    /// Waits for a script that answers `yes`.
    pub fn wait_js(&mut self, step: &str, seconds: u64, body: &str) -> Res<()> {
        self.wait(step, seconds, |j| j.yes(body))
    }

    pub fn path(&mut self) -> Res<String> {
        self.js("return location.pathname;")
    }

    pub fn go(&mut self, href: &str) -> Res<()> {
        let q = crate::json::quote(href, true);
        self.js(&format!("history.pushState(null,'',{q}); dispatchEvent(new PopStateEvent('popstate')); return 'yes';"))?;
        Ok(())
    }

    pub fn open_link(&mut self, href: &str) -> Res<()> {
        let q = crate::json::quote(href, true);
        self.js(&format!("document.querySelector('a[href=' + JSON.stringify({q}) + ']').click(); return 'yes';"))?;
        Ok(())
    }

    pub fn follow(&mut self, selector: &str) -> Res<Vec<String>> {
        let q = crate::json::quote(selector, true);
        let v = self.q(&format!("return JSON.stringify([...document.querySelectorAll({q})].map(a => a.getAttribute('href')).filter(Boolean));"))?;
        let links: Vec<String> = v.as_arr().unwrap_or(&[]).iter().filter_map(|h| h.as_str().map(String::from)).collect();
        // A selector that finds nothing would audit nothing and still pass: it is a failure of the journey itself.
        if links.is_empty() {
            return Err(format!("no_links_for:{selector}"));
        }
        Ok(links)
    }

    pub fn press(&mut self, label: &str) -> Res<()> {
        let wanted = crate::json::quote(label, true);
        let body = format!(
            "const scope = document.querySelector('dialog[open]') ?? document; const buttons = [...scope.querySelectorAll('button')].filter(e => !e.disabled && e.getAttribute('aria-disabled') !== 'true'); const b = buttons.find(e => e.textContent.trim() === {wanted}) ?? buttons.find(e => e.textContent.trim().startsWith({wanted})); if (!b) return 'no'; b.click(); return 'yes';"
        );
        if self.js(&body)? != "yes" {
            let seen = self.jsn("const scope = document.querySelector('dialog[open]') ?? document; return JSON.stringify([...scope.querySelectorAll('button')].map(b => [b.textContent.trim().slice(0, 30), b.disabled, b.getAttribute('aria-disabled'), b.title.slice(0, 120), b.dataset.why ?? '']));", 3).unwrap_or_default();
            return Err(format!("button_missing:{label}:{}", seen.chars().take(700).collect::<String>()));
        }
        sleep_ms(200);
        Ok(())
    }

    pub fn nav(&mut self, label: &str) -> Res<()> {
        nav(&mut self.st, label)
    }

    pub fn audit(&mut self, label: &str, theme: &str) -> Res<Value> {
        audit(&mut self.run, &mut self.st, label, theme)
    }

    pub fn popovers(&mut self, label: &str) -> Res<()> {
        popovers(&mut self.run, &mut self.st, label)
    }

    pub fn step(&mut self, name: &str, details: Vec<(&str, Value)>) {
        outcome(&mut self.run, name, "passed", details);
    }

    pub fn note(&mut self, name: &str, details: Vec<(&str, Value)>) {
        outcome(&mut self.run, name, "diagnostic", details);
    }

    pub fn fail(&mut self, name: &str, details: Vec<(&str, Value)>) {
        fault(&mut self.run, name, details);
    }

    pub fn not_applicable(&mut self, name: &str, reason: &str, page: Value) {
        outcome(&mut self.run, name, "not_applicable", vec![("reason", s(reason)), ("page_words", page)]);
    }

    pub fn check(&mut self, name: &str, ok: bool, details: Vec<(&str, Value)>) {
        if ok {
            self.step(name, details);
        } else {
            self.fail(name, details);
        }
    }

    // ------------------------------------------------------------ processes and evidence

    pub fn page_answers_now(&mut self) -> bool {
        self.st.js("return 'yes';", 3).is_ok_and(|a| a == "yes")
    }

    pub fn unexpected_terminations(&self) -> Value {
        terminations_unexpected(&web_terminations(&self.st.probe_dir), self.st.closes)
    }

    pub fn compositor_gone(&mut self) -> Option<std::process::ExitStatus> {
        self.st.compositor.as_mut().and_then(|c| c.try_wait().ok().flatten())
    }

    /// The one tolerated case: the page replaced a node the harness held, and every process is alive and answering. The same
    /// libatspi text also follows a real death, so the text alone never decides.
    pub fn node_went_stale(&mut self, error: &str) -> bool {
        if !a11y::is_stale_text(error) {
            return false;
        }
        let app = self.st.app_status();
        let answers = app.is_none() && self.page_answers_now();
        let unexpected = self.unexpected_terminations();
        let compositor = self.compositor_gone();
        let class = failure_class(app.is_some(), unexpected.as_arr().is_some_and(|a| !a.is_empty()), compositor.is_some(), error);
        let stale = class.as_str() == Some("journey_step_failed") && answers;
        if stale {
            let after = self.run.progress_step.clone();
            self.note("accessibility_node_stale", vec![("after_step", s(after)), ("app", exit_status(app)), ("page_answers", b(answers))]);
        }
        stale
    }

    /// Repeats an accessibility action whose node the page replaced. libatspi refuses a stale node before anything is sent, so
    /// nothing runs twice.
    pub fn retry_stale<T>(&mut self, mut action: impl FnMut(&mut J) -> Res<T>) -> Res<T> {
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            match action(self) {
                Ok(value) => return Ok(value),
                Err(error) => {
                    if Instant::now() >= end || !self.node_went_stale(&error) {
                        return Err(error);
                    }
                    sleep_ms(50);
                }
            }
        }
    }

    pub fn find(&self, name: &str) -> Option<a11y::Node> {
        a11y::find(self.group(), name, None)
    }

    pub fn has(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    pub fn text_seen(&self, text: &str) -> bool {
        a11y::text_seen(self.group(), text)
    }

    pub fn wait_node(&mut self, name: &str) -> Res<a11y::Node> {
        let end = Instant::now() + Duration::from_secs(15);
        while Instant::now() < end {
            if let Some(node) = self.find(name) {
                return Ok(node);
            }
            if let Some(status) = self.st.app_status() {
                return Err(format!("app_exited_during_request:{}", line(&exit_status(Some(status)))));
            }
            sleep_ms(50);
        }
        // Retained diagnostic: what the window showed instead (the startup screen, an alert ...).
        let shown = self.js("return (document.body?.innerText ?? '').replace(/\\s+/g, ' ').trim().slice(0, 240);").unwrap_or_default();
        let socket = self.st.socket();
        let timed = ["overview", "config.read"].iter().map(|op| {
            let (send, recv) = std::sync::mpsc::channel();
            let began = Instant::now();
            let path = socket.clone();
            let call = obj(vec![("op", s(*op)), ("params", obj(vec![]))]);
            std::thread::spawn(move || { let _ = send.send(crate::installed::seed_large::rpc(&path, "machine.call", call)); });
            match recv.recv_timeout(Duration::from_secs(60)) {
                Ok(Ok(_)) => format!("{op} ok {} ms", began.elapsed().as_millis()),
                Ok(Err(e)) => format!("{op} error {} ms: {}", began.elapsed().as_millis(), e.chars().take(120).collect::<String>()),
                Err(_) => format!("{op} no reply in 60 s"),
            }
        }).collect::<Vec<_>>().join("; ");
        let _ = self.js("window.__cwProbe = {}; for (const [k, m] of [['overview', {op: 'overview'}], ['config', {op: 'config.read'}], ['caps', null]]) { const began = performance.now(); const call = m ? {operation: 'machine.call', message: m} : {operation: 'service.capabilities', message: {}}; window.__TAURI_INTERNALS__.invoke('workspace_request', call).then(v => { window.__cwProbe[k] = 'ok ' + Math.round(performance.now() - began) + 'ms ' + JSON.stringify(v).slice(0, 80); }, e => { window.__cwProbe[k] = 'err ' + Math.round(performance.now() - began) + 'ms ' + String(e).slice(0, 120); }); } return 'started';");
        sleep_ms(3000);
        let bridge = self.js("return JSON.stringify(window.__cwProbe ?? null);").unwrap_or_default();
        let late = Instant::now();
        let mut appeared = String::from("not within a further 45 s");
        while late.elapsed() < Duration::from_secs(45) {
            if self.find(name).is_some() { appeared = format!("after a further {} ms", late.elapsed().as_millis()); sleep_ms(500); let after = self.js("return [...document.querySelectorAll('[role=alert],[role=status]')].map(e => e.textContent.trim() + (e.dataset.reason ? ' [' + e.dataset.reason + ']' : '')).join(' | ').slice(0, 300);").unwrap_or_default(); appeared.push_str(&format!("; alerts: {after}")); break; }
            sleep_ms(100);
        }
        Err(format!("control_missing:{name}; page: {shown}; direct: {timed}; appeared: {appeared}; bridge: {bridge}"))
    }

    pub fn wait_found(&mut self, step: &str, seconds: u64, name: &str) -> Res<()> {
        let g = self.group();
        let name = name.to_string();
        self.wait(step, seconds, move |_| Ok(a11y::find(g, &name, None).is_some()))
    }

    pub fn wait_gone(&mut self, step: &str, seconds: u64, name: &str) -> Res<()> {
        let g = self.group();
        let name = name.to_string();
        self.wait(step, seconds, move |_| Ok(a11y::find(g, &name, None).is_none()))
    }

    pub fn wait_text(&mut self, step: &str, seconds: u64, text: &str) -> Res<()> {
        let g = self.group();
        let text = text.to_string();
        self.wait(step, seconds, move |_| Ok(a11y::text_seen(g, &text)))
    }

    /// The control by its accessible name, pressed through its own action.
    pub fn click(&mut self, name: &str) -> Res<()> {
        let label = name.to_string();
        self.retry_stale(|j| {
            let node = j.wait_node(&label)?;
            if !node.enabled() {
                return Err(format!("control_disabled:{label}"));
            }
            if !a11y::do_action(&node)? {
                return Err(format!("control_action_failed:{label}"));
            }
            Ok(())
        })?;
        sleep_ms(150);
        Ok(())
    }

    /// What the page and the accessibility tree say about a field that refused focus. It explains the failure, it never clears it.
    pub fn focus_refused(&mut self, name: &str, node: &a11y::Node) {
        let label = crate::json::quote(name, true);
        let page = self
            .q(&format!(
                "const label = {label}, fields = [...document.querySelectorAll('textarea, input')].filter(e => e.getAttribute('aria-label') === label); return JSON.stringify({{fields: fields.map(e => {{ const r = e.getBoundingClientRect(), s = getComputedStyle(e); return {{disabled: e.disabled, inert: !!e.closest('[inert]'), inDialog: !!e.closest('dialog'), hint: e.placeholder.slice(0, 80), box: [r.x, r.y, r.width, r.height].map(Math.round), display: s.display, visibility: s.visibility}}; }}), open: [...document.querySelectorAll('dialog[open]')].map(d => d.getAttribute('aria-label')), active: document.activeElement?.tagName + ':' + (document.activeElement?.getAttribute('aria-label') ?? ''), hasFocus: document.hasFocus(), route: location.pathname, viewport: [innerWidth, innerHeight]}});"
            ))
            .unwrap_or(Value::Null);
        let same: Vec<a11y::Node> = a11y::tree(self.group()).into_iter().filter(|n| n.name == name).collect();
        let retried: Vec<Value> = same.iter().take(4).map(|n| n.acc.grab_focus().map(b).unwrap_or_else(|e| s(e.chars().take(40).collect::<String>()))).collect();
        let nodes: Vec<Value> = same.iter().take(4).map(|n| arr(a11y::focus_states(&n.acc).into_iter().map(s).collect())).collect();
        self.note(
            "field_focus_refused",
            vec![("field", s(name)), ("page", page), ("refused_node", arr(a11y::focus_states(&node.acc).into_iter().map(s).collect())), ("nodes", arr(nodes)), ("node_count", n(same.len() as i64)), ("retried", arr(retried))],
        );
    }

    pub fn key(&mut self, name: &str) -> Res<()> {
        self.st.input.as_ref().ok_or("no keyboard")?.key(name)
    }

    pub fn type_text(&mut self, text: &str) -> Res<()> {
        self.st.input.as_ref().ok_or("no keyboard")?.type_text(text)
    }

    /// Replaces a field's value through its native focus and the real keyboard, then verifies the exact entry that took focus.
    pub fn fill(&mut self, name: &str, value: &str) -> Res<()> {
        let label = name.to_string();
        let node = self.retry_stale(|j| {
            let field = j.wait_node(&label)?;
            if !a11y::grab_focus(&field)? {
                j.focus_refused(&label, &field);
                return Err(format!("field_focus_failed:{label}"));
            }
            Ok(field)
        })?;
        sleep_ms(200);
        self.key("ctrl+a")?;
        for (index, text) in value.split('\n').enumerate() {
            if index > 0 {
                self.key("shift+Return")?;
            }
            self.type_text(text)?;
        }
        // WebKit can include the new value in an implicit label: verify the exact entry that acquired native focus, not its old name.
        let expected = value.to_string();
        self.wait(&format!("field_value_mismatch:{name}"), 3, |j| match node.acc.text() {
            Ok(text) => Ok(text == expected),
            Err(error) => {
                if !j.node_went_stale(&error) {
                    return Err(error);
                }
                Ok(j.js("return document.activeElement?.value ?? '';")? == expected)
            }
        })
    }

    /// Text on the OS clipboard through an independent owner, then a real paste into the focused field.
    pub fn paste(&mut self, text: &str) -> Res<()> {
        let child = self.st.input.as_ref().ok_or("no keyboard")?.paste(text)?;
        self.keep.push(child);
        Ok(())
    }

    pub fn release_clipboards(&mut self) {
        for mut child in self.keep.drain(..) {
            unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
            let _ = child.wait();
        }
    }

    // ------------------------------------------------------------ app lifecycle

    pub fn launch(&mut self, theme: &str) -> Res<()> {
        self.st.launch(theme)
    }

    /// The page is up and, on Wayland, owns native keyboard focus: a real pointer press gives it focus the way a user would.
    pub fn loaded(&mut self) -> Res<()> {
        self.st.loaded()?;
        if self.st.wayland {
            self.js("window.__cwClick = 0; document.addEventListener('mousedown', () => window.__cwClick++, {once:true}); return 'yes';")?;
            let input = self.st.input.as_ref().ok_or("no pointer")?;
            input.nudge(-(MONITOR.0 as f64 * 2.0), -(MONITOR.1 as f64 * 2.0))?;
            sleep_ms(150);
            input.nudge(MONITOR.0 as f64 / 2.0, MONITOR.1 as f64 / 2.0)?;
            sleep_ms(150);
            input.button(true)?;
            sleep_ms(60);
            input.button(false)?;
            sleep_ms(150);
            self.js("@focus")?;
            let facts = self.q("return JSON.stringify({focused:document.hasFocus(),clicks:window.__cwClick});")?;
            self.note("wayland_focus_observation", vec![("facts", facts)]);
            self.wait_js("wayland_native_keyboard_focus", 15, "return document.hasFocus() ? 'yes' : '';")?;
        }
        Ok(())
    }

    /// A relaunch waits for the page and the control tree of the new window.
    pub fn relaunch(&mut self, theme: &str, step: &str) -> Res<()> {
        self.launch(theme)?;
        self.wait_found(step, 15, "Add project")?;
        self.loaded()
    }

    /// The standard close request: WM_DELETE_WINDOW on X11 to the exact owned window, the page's own button on Wayland. No signal,
    /// screenshot or browser inspector is used, and exactly one web-process termination is accounted for per request.
    pub fn close_window(&mut self) -> Res<()> {
        if self.st.wayland {
            self.js("document.querySelector('.win button:last-child').click(); return 'yes';")?;
        } else {
            let window = self.st.window_id()?;
            super::x11::delete_window(&window)?;
        }
        self.finish_close(if self.st.wayland { "wayland_journey_close_failed" } else { "native_window_close_failed" }, if self.st.wayland { "native_window_close" } else { "native_window_close" })
    }

    fn finish_close(&mut self, failure: &str, step: &str) -> Res<()> {
        self.st.closes += 1;
        let code = self.wait_exit(8)?;
        if code != Some(0) {
            return Err(failure.to_string());
        }
        self.stop_app();
        let backend = if self.st.wayland { "wayland" } else { "x11" };
        self.step(step, vec![("exit_code", n(0)), ("backend", s(backend)), ("closes_requested", n(self.st.closes as i64))]);
        Ok(())
    }

    pub fn wait_exit(&mut self, seconds: u64) -> Res<Option<i32>> {
        let end = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < end {
            if let Some(status) = self.st.app_status() {
                return Ok(status.code());
            }
            sleep_ms(50);
        }
        Err("app_did_not_exit".into())
    }

    /// Terminates any remaining owned renderer-process group members.
    pub fn stop_app(&mut self) {
        stop(&mut self.st.app);
    }

    pub fn close_button(&mut self) -> Res<()> {
        if self.st.wayland {
            return self.close_window();
        }
        let window = self.st.window_id()?;
        click_control(&mut self.st, &window, ".win button[aria-label=\"Close window\"]")?;
        self.finish_close("close_button_failed", "close_button")
    }

    pub fn resize(&mut self, width: i64, height: i64) -> Res<()> {
        if self.st.wayland {
            self.js(&format!("@resize {width} {height}"))?;
            return self.wait("wayland_journey_resize", 15, |j| Ok(num(&j.q("@native")?, "top.0") as i64 == width && num(&j.q("@native")?, "top.1") as i64 == height));
        }
        let window = self.st.window_id()?;
        xdotool(&["windowsize", &window, &width.to_string(), &height.to_string()]).map(|_| ())
    }

    /// Dev registration reserves only the private `.cortex-dev` directory and its owner.json: no store, report or engine, and no
    /// release data.
    pub fn registration_inert(&self, project: &Path) -> bool {
        let dev = project.join(".cortex-dev");
        let names = std::fs::read_dir(&dev).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>()).unwrap_or_default();
        !project.join(".cortex").exists()
            && dev.is_dir()
            && !dev.is_symlink()
            && names == ["owner.json"]
            && dev.join("owner.json").is_file()
            && !dev.join("owner.json").is_symlink()
            && !project.join(".cortex-dev-owner.json").exists()
    }

    /// Paseo's flow: a way in, then a field that searches the machine. Enter adds the first row, the typed path.
    pub fn add_project(&mut self, path: &Path) -> Res<()> {
        self.click("Add project")?;
        let options = "[...document.querySelectorAll('dialog[open] [role=option]')]";
        self.wait_js("add_project_ways_in", 15, &format!("return {options}.length ? 'yes' : ''"))?;
        self.js(&format!("{options}.find(o => o.textContent.startsWith('Search for folder')).click(); return 'yes';"))?;
        self.wait_js("add_project_field", 15, "return document.querySelector('dialog[open] input') ? 'yes' : ''")?;
        let typed = crate::json::quote(&path.to_string_lossy(), true);
        self.js(&format!("const e = document.querySelector('dialog[open] input'); e.value = {typed}; e.dispatchEvent(new InputEvent('input', {{bubbles: true}})); return 'yes';"))?;
        self.wait_js("add_project_row", 15, &format!("return {options}.some(o => o.getAttribute('aria-selected') === 'true') ? 'yes' : ''"))?;
        self.js("document.querySelector('dialog[open] input').dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true, cancelable: true})); return 'yes';")?;
        Ok(())
    }

    pub fn inspector_open(&mut self, label: &str) -> Res<()> {
        let shown = self.q("const i = document.querySelector('.inspector'), m = document.querySelector('.main'), a = i.getBoundingClientRect(), b = m.getBoundingClientRect(); return JSON.stringify({ visible: getComputedStyle(i).display !== 'none' && a.width > 0, overlap: a.width > 0 && a.left < b.right - .5, width: a.width, rows: i.querySelectorAll('.ins-sec').length, empty: [...i.querySelectorAll('dd')].filter(d => !d.textContent.trim()).length });")?;
        let ok = flag(&shown, "visible") && !flag(&shown, "overlap") && num(&shown, "rows") > 0.0 && num(&shown, "empty") == 0.0;
        let mut pairs = vec![("screen", s(label))];
        pairs.extend(outcome_pairs(&shown));
        self.check("inspector", ok, pairs);
        Ok(())
    }

    pub fn wait_found_role(&mut self, step: &str, seconds: u64, contains: &str, role: &str) -> Res<a11y::Node> {
        let end = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < end {
            if let Some(node) = a11y::find_containing(self.group(), contains, role) {
                return Ok(node);
            }
            sleep_ms(50);
        }
        Err(step.to_string())
    }

    /// Opens a card link the accessibility tree names, through its own action.
    pub fn open_task(&mut self, step: &str, title: &str) -> Res<()> {
        let (step, title) = (step.to_string(), title.to_string());
        self.retry_stale(|j| {
            let node = j.wait_found_role(&step, 15, &title, "link")?;
            if !a11y::do_action(&node)? {
                return Err(format!("control_action_failed:{step}"));
            }
            Ok(())
        })
    }

    pub fn open_project(&mut self, name: &str) -> Res<()> {
        let q = crate::json::quote(name, true);
        self.wait_js("project_in_sidebar", 15, &format!("return [...document.querySelectorAll('.proj')].some(p => p.textContent.includes({q})) ? 'yes' : ''"))?;
        self.js(&format!("[...document.querySelectorAll('.proj')].find(p => p.textContent.includes({q})).click(); return 'yes';"))?;
        sleep_ms(300);
        Ok(())
    }

    /// The most recent runtime task's state in the project's own store.
    pub fn task_state(&self, project: &Path) -> Res<Option<String>> {
        native::state(project, &self.channel)
    }

    pub fn store_count(&self, project: &Path, sql: &str) -> Res<i64> {
        native::count(&self.channel.store_db(project), sql)
    }
}

pub(super) fn arr_i(values: &[i64]) -> Value {
    arr(values.iter().map(|v| n(*v)).collect())
}

pub(super) fn spawn_quiet(command: &mut Command) -> Res<Child> {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    use std::os::unix::process::CommandExt;
    command.spawn().map_err(|e| e.to_string())
}


impl J {
    /// The control by its accessible name, pressed with the real pointer at the centre of its on-screen box.
    /// The on-screen centre of the control named `name` inside an open modal dialog: its page box plus the web
    /// document's screen origin. None when no open dialog holds such a control or the document is not found.
    fn dialog_point(&mut self, name: &str) -> Option<(f64, f64)> {
        let q = crate::json::quote(name, true);
        let rect = self.q(&format!("const d = document.querySelector('dialog[open]'); if (!d) return 'null'; const c = [...d.querySelectorAll('button, a, [role=button], input, textarea')].find(e => (e.getAttribute('aria-label') ?? e.textContent ?? '').trim() === {q}); if (!c) return 'null'; const r = c.getBoundingClientRect(); return JSON.stringify({{x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2)}});"));
        let rect = rect.ok()?;
        let (cx, cy) = (rect.get("x")?.as_i64()? as f64, rect.get("y")?.as_i64()? as f64);
        // While a modal dialog is open every accessible extent is offset, the document's too, so the page's own
        // pointer coordinates calibrate the screen-to-page offset: move the real pointer and read where it lands.
        let doc = a11y::tree(self.group()).into_iter().find(|n| n.role.starts_with("document"))?;
        let (dx, dy, _, _) = doc.acc.extents(crate::installed::ffi::COORD_SCREEN).ok()?;
        let (px, py) = (dx as f64 + cx, dy as f64 + cy);
        self.js("window.__cwMove = null; if (!window.__cwMoveArmed) { window.__cwMoveArmed = true; document.addEventListener('pointermove', e => { window.__cwMove = [Math.round(e.clientX), Math.round(e.clientY)]; }, true); } return 'armed';").ok()?;
        let input = self.st.input.as_ref()?;
        input.move_to(px - 3.0, py - 3.0).ok()?;
        sleep_ms(120);
        input.move_to(px, py).ok()?;
        sleep_ms(200);
        let landed = self.q("return JSON.stringify(window.__cwMove ? {x: window.__cwMove[0], y: window.__cwMove[1]} : null);").ok()?;
        let (mx, my) = (landed.get("x")?.as_i64()? as f64, landed.get("y")?.as_i64()? as f64);
        Some((px + (cx - mx), py + (cy - my)))
    }

    pub fn click_pointer(&mut self, name: &str) -> Res<()> {
        let label = name.to_string();
        let (x, y, w, h) = self.retry_stale(|j| {
            let node = j.wait_node(&label)?;
            if !node.enabled() {
                return Err(format!("control_disabled:{label}"));
            }
            node.acc.extents(crate::installed::ffi::COORD_SCREEN)
        })?;
        require(w > 0 && h > 0, &format!("control_has_no_geometry:{name}"))?;
        // WebKitGTK reports a control inside an open modal <dialog> (the top layer) relative to the dialog's
        // own box, not the page. The real pointer aims at the control's page box instead, mapped to the
        // screen through the web document's own on-screen origin; a control outside a dialog is unchanged.
        let (px, py) = match self.dialog_point(name) {
            Some(point) => point,
            None => (x as f64 + w as f64 / 2.0, y as f64 + h as f64 / 2.0),
        };
        self.st.input.as_ref().ok_or("no pointer")?.click_at(px, py)?;
        sleep_ms(150);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reasons(list: &[&str]) -> Value {
        arr(list.iter().map(|r| s(*r)).collect())
    }

    fn unexpected(list: &[&str], closes: u32) -> Vec<String> {
        terminations_unexpected(&reasons(list), closes).as_arr().unwrap().iter().filter_map(|v| v.as_str().map(String::from)).collect()
    }

    #[test]
    fn exactly_one_terminated_by_api_per_requested_close_is_accepted() {
        assert!(unexpected(&[], 0).is_empty());
        assert!(unexpected(&["terminated_by_api"], 1).is_empty());
        assert!(unexpected(&["terminated_by_api", "terminated_by_api", "terminated_by_api"], 3).is_empty());
    }

    #[test]
    fn nothing_looser_is_accepted() {
        // One too many, one too few, an unrequested one, and any other reason.
        assert!(!unexpected(&["terminated_by_api", "terminated_by_api"], 1).is_empty());
        assert!(!unexpected(&[], 1).is_empty());
        assert!(!unexpected(&["terminated_by_api"], 0).is_empty());
        assert_eq!(unexpected(&["crashed"], 1), vec!["crashed".to_string()]);
        assert!(unexpected(&["terminated_by_api", "exceeded_memory_limit"], 2).contains(&"exceeded_memory_limit".to_string()));
        assert!(unexpected(&["terminated_by_api", "crashed"], 1).contains(&"crashed".to_string()));
    }

    #[test]
    fn a_count_mismatch_names_both_numbers() {
        assert!(unexpected(&["terminated_by_api"], 2)[0].contains("1_for_2_requested_closes"));
    }
}
