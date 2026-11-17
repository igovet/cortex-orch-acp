//! The stages that run after the journey on a fresh window each: the Wayland frame and control stage (X11 journey only), the
//! terminal and the voice round trip. The attachment stage is `attachments.rs`.

use super::journey::*;
use super::session::*;
use crate::installed::util::*;
use crate::json::{parse, Res, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

impl J {
    // ------------------------------------------------------------ Wayland frame and controls

    fn native_size(&mut self) -> Res<(i64, i64)> {
        let native = self.q("@native")?;
        Ok((num(&native, "top.0") as i64, num(&native, "top.1") as i64))
    }

    fn pointer_at(&mut self) -> Res<Option<(f64, f64)>> {
        let v = parse(&self.js("return JSON.stringify(window.__cwPointer);")?)?;
        Ok(v.as_arr().map(|a| (num(&Value::Arr(a.to_vec()), "0"), num(&Value::Arr(a.to_vec()), "1"))))
    }

    fn nudge(&mut self, dx: f64, dy: f64) -> Res<()> {
        self.st.input.as_ref().ok_or("no pointer")?.nudge(dx, dy)?;
        sleep_ms(30);
        Ok(())
    }

    fn button(&mut self, down: bool) -> Res<()> {
        self.st.input.as_ref().ok_or("no pointer")?.button(down)?;
        sleep_ms(60);
        Ok(())
    }

    /// The page reports where the real pointer is; relative motion moves it there.
    fn go_to(&mut self, x: f64, y: f64) -> Res<(f64, f64)> {
        for _ in 0..40 {
            let Some(now) = self.pointer_at()? else {
                self.nudge(40.0, 25.0)?;
                continue;
            };
            if (now.0 - x).abs() <= 1.0 && (now.1 - y).abs() <= 1.0 {
                return Ok(now);
            }
            self.nudge(x - now.0, y - now.1)?;
            sleep_ms(50);
        }
        Err("wayland_pointer_unreachable".into())
    }

    fn glide(&mut self, dx: f64, dy: f64) -> Res<()> {
        for _ in 0..8 {
            self.nudge(dx / 8.0, dy / 8.0)?;
        }
        Ok(())
    }

    fn blank_spot(&mut self) -> Result<(f64, f64), String> {
        let v = self.q("const b = document.querySelector('.tb-main').getBoundingClientRect(), g = document.querySelector('.tb-main > .grow').getBoundingClientRect(); return JSON.stringify([Math.round(g.x + g.width / 2), Math.round(b.y + b.height / 2)]);")?;
        Ok((num(&v, "0"), num(&v, "1")))
    }

    fn page_button(&mut self, label: &str) -> Res<()> {
        let q = crate::json::quote(label, true);
        self.js(&format!("document.querySelector('.win button[aria-label=' + JSON.stringify({q}) + ']').click(); return 'yes';"))?;
        Ok(())
    }

    /// The owner's session type: a real headless GNOME compositor, its window rules and its pointer, separate from the X11 stack.
    pub fn wayland_stage(&mut self) -> Res<()> {
        let mut stage = Stack::new(self.st.scratch.clone(), self.st.probe_dir.clone(), self.st.shim.clone(), self.st.artifacts.clone(), self.st.checkout.clone(), self.st.channel.clone(), true);
        stage.wayland_name = "cw-wayland".into();
        stage.a11y = false;
        stage.launches = self.st.launches;
        stage.closes = 0;
        std::mem::swap(&mut self.st, &mut stage);
        let result = self.wayland_stage_inner();
        stop(&mut self.st.app);
        stop(&mut self.st.compositor);
        std::mem::swap(&mut self.st, &mut stage);
        self.st.launches = stage.launches;
        self.st.closes += stage.closes;
        result
    }

    fn wayland_stage_inner(&mut self) -> Res<()> {
        self.st.start_wayland()?;
        self.launch("dark")?;
        self.st.loaded()?;
        let state = self.q("@native")?;
        let framed = text(&state, "backend") != "GdkWaylandDisplay" || flag(&state, "decorated") || line(at(&state, "top").unwrap_or(&Value::Null)) != line(at(&state, "web").unwrap_or(&Value::Null));
        if framed {
            self.fail("wayland_frame_present", vec![("native", state)]);
        } else {
            self.step("wayland_no_frame", vec![("native", state)]);
        }
        self.audit("home on wayland", "dark")?;
        let monitor = MONITOR;
        self.page_button("Maximize")?;
        self.wait("wayland_maximize", 15, |j| Ok(j.native_size()? == monitor))?;
        self.wait_js("wayland_restore_label", 15, "return document.querySelector('.win button[aria-label=\"Restore\"]') ? 'yes' : ''")?;
        let size = self.native_size()?;
        self.step("wayland_maximize", vec![("size", arr_i(&[size.0, size.1]))]);
        self.audit("home on wayland maximized", "dark")?;
        self.page_button("Restore")?;
        self.wait("wayland_restore", 15, |j| Ok(j.native_size()? != monitor))?;
        let size = self.native_size()?;
        self.step("wayland_restore", vec![("size", arr_i(&[size.0, size.1]))]);

        self.js("window.__cwPointer = null; document.addEventListener('mousemove', e => { window.__cwPointer = [e.clientX, e.clientY]; }, true); return 'yes';")?;
        // The session's pointer starts in the far corner; the top-left corner is the known place to walk in from.
        self.nudge(-(MONITOR.0 as f64 * 2.0), -(MONITOR.1 as f64 * 2.0))?;
        let blank = self.blank_spot()?;
        self.go_to(blank.0, blank.1)?;
        self.button(true)?;
        self.glide(150.0, 60.0)?;
        self.button(false)?;
        sleep_ms(400);
        self.nudge(1.0, 0.0)?;
        sleep_ms(200);
        let after = self.pointer_at()?.unwrap_or((f64::NAN, f64::NAN));
        // The window followed the drag, so the pointer is still near where it started in the page.
        let moved = (after.0 - blank.0).abs() < 60.0 && (after.1 - blank.1).abs() < 40.0;
        self.check("wayland_band_drag", moved, vec![("pointer_in_page_before", arr_i(&[blank.0 as i64, blank.1 as i64])), ("after", arr(vec![Value::Num(format!("{}", after.0)), Value::Num(format!("{}", after.1))])), ("pointer_moved", arr_i(&[150, 60]))]);
        let pressed = self.js("return document.querySelector('.tb-toggle').getAttribute('aria-pressed');")?;
        let toggle = self.q("const r = document.querySelector('.tb-toggle').getBoundingClientRect(); return JSON.stringify([Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)]);")?;
        self.go_to(num(&toggle, "0"), num(&toggle, "1"))?;
        self.button(true)?;
        self.button(false)?;
        sleep_ms(200);
        if self.js("return document.querySelector('.tb-toggle').getAttribute('aria-pressed');")? == pressed {
            self.fail("wayland_first_click_after_drag_lost", vec![]);
        } else {
            self.step("wayland_click_after_drag", vec![]);
        }
        // Put the toggle back, then a double click on the blank band maximises and restores.
        self.button(true)?;
        self.button(false)?;
        sleep_ms(200);
        for (expect_full, step) in [(true, "wayland_double_click_maximizes"), (false, "wayland_double_click_restore")] {
            let blank = self.blank_spot()?;
            self.go_to(blank.0, blank.1)?;
            self.button(true)?;
            self.button(false)?;
            sleep_ms(50);
            self.button(true)?;
            self.button(false)?;
            self.wait(step, 15, |j| Ok((j.native_size()? == monitor) == expect_full))?;
            if expect_full {
                let size = self.native_size()?;
                self.step(step, vec![("size", arr_i(&[size.0, size.1]))]);
            }
        }
        sleep_ms(300);
        let before = self.native_size()?;
        self.go_to((before.0 - 2) as f64, (before.1 / 2) as f64)?;
        self.button(true)?;
        self.glide(-70.0, 0.0)?;
        self.button(false)?;
        sleep_ms(400);
        let right = self.native_size()?;
        let ok = (before.0 - right.0 - 70).abs() <= 8 && right.1 == before.1;
        self.check("wayland_edge_resize", ok, vec![("edge", s("right")), ("before", arr_i(&[before.0, before.1])), ("after", arr_i(&[right.0, right.1]))]);
        self.go_to((right.0 / 2) as f64, (right.1 - 2) as f64)?;
        self.button(true)?;
        self.glide(0.0, -50.0)?;
        self.button(false)?;
        sleep_ms(400);
        let bottom = self.native_size()?;
        let ok = (right.1 - bottom.1 - 50).abs() <= 8 && bottom.0 == right.0;
        self.check("wayland_edge_resize", ok, vec![("edge", s("bottom")), ("before", arr_i(&[right.0, right.1])), ("after", arr_i(&[bottom.0, bottom.1]))]);
        self.audit("home on wayland resized", "dark")?;
        // Wayland has no way to read minimized state back, so this proves the call is accepted and the app lives on.
        self.page_button("Minimize")?;
        sleep_ms(400);
        let alive = self.st.app_status().is_none();
        self.check("wayland_minimize_requested", alive, vec![("alive", b(alive)), ("observed", s("state not readable on Wayland"))]);
        self.page_button("Close window")?;
        self.st.closes += 1;
        let code = self.wait_exit(8)?;
        if code != Some(0) {
            return Err("wayland_close_failed".into());
        }
        self.stop_app();
        self.step("wayland_close_button", vec![("exit_code", n(0)), ("closes_requested", n(self.st.closes as i64))]);
        Ok(())
    }

    // ------------------------------------------------------------ terminal

    /// Ctrl+J toggles the terminal and the window remembers whether it was open: press it only when the terminal is not shown, then
    /// require that it is.
    fn show_terminal(&mut self) -> Res<()> {
        let shown = "const t = document.querySelector('.xterm-helper-textarea'); return t && t.offsetParent !== null ? 'yes' : '';";
        sleep_ms(500);
        if !self.yes(shown)? {
            self.key("ctrl+j")?;
        }
        self.wait_js("terminal_shown", 15, shown)
    }

    fn terminal_text(&mut self) -> Res<String> {
        self.js("return document.querySelector('.terminal-screen')?.textContent ?? '';")
    }

    fn shell_input(&mut self, text: &str) -> Res<()> {
        // A re-attached terminal repaints its replay after it is shown, and that can move focus away: type only once the shell's own input
        // has held focus across two reads.
        let mut held = 0;
        self.wait("terminal_input_focus", 10, |j| {
            j.js("document.querySelector('.xterm-helper-textarea').focus(); return 'yes';")?;
            sleep_ms(250);
            held = if j.yes("return document.activeElement?.classList.contains('xterm-helper-textarea') ? 'yes' : ''")? { held + 1 } else { 0 };
            Ok(held >= 2)
        })
        .map_err(|e| {
            let seen = self.jsn("return JSON.stringify({areas: [...document.querySelectorAll('.xterm-helper-textarea')].map(t => ({disabled: t.disabled, shown: t.offsetParent !== null, readOnly: t.readOnly, inert: !!t.closest('[inert]'), box: [t.offsetWidth, t.offsetHeight]})), active: document.activeElement?.tagName + ':' + (document.activeElement?.className ?? ''), focused: document.hasFocus(), status: document.querySelector('.terminal-status')?.textContent ?? ''});", 3).unwrap_or_default();
            format!("{e}:{seen}")
        })?;
        self.type_text(text)?;
        self.key("Return")
    }

    /// A real PTY in the project folder, driven with the real keyboard: it round-trips, resizes, survives a window close and the
    /// theme changes, and is reaped when the owner closes it. Opening it never starts an engine.
    pub fn terminal_stage(&mut self) -> Res<()> {
        let project = self.home.join("terminal");
        self.launch("dark")?;
        self.wait_found("terminal_window", 15, "Add project")?;
        self.loaded()?;
        self.add_project(&project)?;
        self.wait_found("terminal_registered", 15, "Activate")?;
        // PTYs are owner sessions; opening one must not activate an ACP engine.
        self.show_terminal()?;
        self.wait_js("terminal_running", 30, "return document.querySelector('.terminal-status')?.textContent === 'Running' ? 'yes' : '';")?;
        let inert = self.registration_inert(&project);
        self.check("terminal_registration_inert", inert, vec![]);
        self.shell_input("printf 'CW_%s\\n' TERMINAL; pwd > terminal-cwd; id -u > terminal-uid; stty size > terminal-size; echo $$ > terminal-pid")?;
        self.wait("terminal_round_trip", 15, |j| Ok(project.join("terminal-pid").is_file() && j.terminal_text()?.contains("CW_TERMINAL")))?;
        let read = |name: &str| std::fs::read_to_string(project.join(name)).map(|t| t.trim().to_string()).unwrap_or_default();
        let cwd_ok = read("terminal-cwd") == project.to_string_lossy();
        let uid_ok = read("terminal-uid") == unsafe { libc::getuid() }.to_string();
        let pid: i32 = read("terminal-pid").parse().map_err(|_| "terminal_pid_unreadable".to_string())?;
        let before = read("terminal-size");
        self.resize(1100, 780)?;
        sleep_ms(600);
        self.shell_input("stty size > terminal-resized")?;
        self.wait("terminal_resize", 15, |_| Ok(project.join("terminal-resized").is_file()))?;
        let resized = read("terminal-resized") != before;
        self.check("terminal_cwd_uid_and_resize", cwd_ok && uid_ok && resized, vec![("cwd", b(cwd_ok)), ("uid", b(uid_ok)), ("resized", b(resized))]);
        // Real preference controls in this disposable app, never the owner's settings.
        let project_path = self.path()?;
        for theme in ["Light", "Dark"] {
            self.js("document.querySelector('a[aria-label=Settings]').click(); return 'yes';")?;
            self.click(theme)?;
            self.go(&project_path)?;
            self.wait("terminal_theme_reattach", 15, |j| Ok(j.terminal_text()?.contains("CW_TERMINAL")))?;
            let facts = self.q("const t=document.querySelector('.terminal'), row=document.querySelector('.xterm-rows'), viewport=document.querySelector('.xterm-scrollable-element'); return JSON.stringify({theme:document.documentElement.dataset.theme,font:getComputedStyle(row).fontFamily,bg:getComputedStyle(t).backgroundColor,renderer:getComputedStyle(viewport).backgroundColor,width:row.clientWidth,height:row.clientHeight});")?;
            self.note("terminal_theme_facts", outcome_pairs(&facts));
            let ok = text(&facts, "theme") == theme.to_lowercase() && text(&facts, "font").contains("Cousine") && num(&facts, "width") > 0.0 && num(&facts, "height") > 0.0 && text(&facts, "bg") == text(&facts, "renderer");
            self.check("terminal_theme", ok, vec![("theme", s(theme))]);
        }
        self.js("document.querySelector('[aria-label=\"Hide terminal\"]').click(); return 'yes';")?;
        require(Path::new(&format!("/proc/{pid}")).exists(), "terminal_shell_died_when_hidden")?;
        self.show_terminal()?;
        self.wait("terminal_hide_reattach", 15, |j| Ok(j.terminal_text()?.contains("CW_TERMINAL")))?;
        self.js("document.querySelector('a[aria-label=Settings]').click(); return 'yes';")?;
        self.click("System")?;
        self.go(&project_path)?;
        self.close_window()?;
        self.relaunch("light", "terminal_reopen_window")?;
        self.open_project("terminal")?;
        self.show_terminal()?;
        self.wait("terminal_retained_after_window_close", 20, |j| Ok(j.terminal_text()?.contains("CW_TERMINAL")))?;
        self.shell_input("echo $$ > terminal-reattached-pid")?;
        self.wait("terminal_reattached_pid", 15, |_| Ok(project.join("terminal-reattached-pid").is_file()))?;
        let same = read("terminal-reattached-pid").parse::<i32>() == Ok(pid);
        self.check("terminal_same_shell_after_window_close", same, vec![]);
        self.js("document.querySelector('[aria-label=\"Close shell\"]').click(); return 'yes';")?;
        self.wait_js("terminal_explicit_close", 15, "return document.querySelector('.terminal-status')?.textContent === 'Closed' ? 'yes' : '';")?;
        let reaped = self.wait("terminal_shell_reaped", 10, |_| Ok(!Path::new(&format!("/proc/{pid}")).exists())).is_ok();
        self.check("native_terminal_round_trip", reaped && same, vec![("keyboard", s("Ctrl+J and real PTY input")), ("retained_after_window_close", b(same)), ("resized", b(resized)), ("shell_reaped", b(reaped)), ("engine_started", b(false))]);
        self.close_window()
    }

    // ------------------------------------------------------------ voice

    /// The voice round trip needs the speech model and the Russian fixture recording the artifact would carry. Without either the
    /// stage is not applicable, with the exact paths looked at; with both it runs against a real isolated PulseAudio source.
    pub fn voice_stage(&mut self) -> Res<()> {
        let artifacts = self.st.artifacts.clone();
        let (model, fixture) = (artifacts.join("voice-model"), artifacts.join("voice-fixture.wav"));
        if !model.is_dir() || !fixture.is_file() {
            let words = format!("{} {}; {} {}", model.display(), if model.is_dir() { "present" } else { "absent" }, fixture.display(), if fixture.is_file() { "present" } else { "absent" });
            self.not_applicable("native_voice_round_trip", "the artifact directory lacks the voice model or the fixture recording, so no transcription can be observed", s(words));
            return Ok(());
        }
        self.voice_round_trip(&model, &fixture)
    }

    fn pactl(&self, args: &[&str]) -> Res<String> {
        let mut all = vec!["pactl"];
        all.extend_from_slice(args);
        Ok(run(&all, Duration::from_secs(20))?.stdout)
    }

    fn voice_status(&mut self) -> Res<String> {
        self.js("return document.querySelector('.micbar [role=status]')?.textContent ?? '';")
    }

    fn voice_text(&mut self) -> Res<String> {
        self.js("return document.querySelector('.mic-text')?.textContent ?? '';")
    }

    fn voice_round_trip(&mut self, model: &Path, fixture: &Path) -> Res<()> {
        let project = self.home.join("voice");
        self.launch("dark")?;
        self.wait_found("voice_window", 15, "Add project")?;
        self.loaded()?;
        // The webview's own capability is inspected, not a presumed browser speech API.
        let facts = self.q("const speech = typeof window.SpeechRecognition === 'function' || typeof window.webkitSpeechRecognition === 'function'; const result = await Promise.race([(async()=>{try {const stream=await navigator.mediaDevices.getUserMedia({audio:true}); stream.getTracks().forEach(t=>t.stop()); return 'granted';}catch(e){return e.name;}})(),new Promise(r=>setTimeout(()=>r('unobservable_timeout'),1500))]); return JSON.stringify({speech, capture:result, mediaDevices:typeof navigator.mediaDevices?.getUserMedia});")?;
        self.note("webview_voice_capabilities", outcome_pairs(&facts));
        require(!flag(&facts, "speech"), "speech_api_now_available_reassess_native_route")?;
        // Fixture PCM feeds a real isolated PulseAudio monitor. No host device or audio socket is mounted.
        let audio = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR unset")?).join("pulse");
        std::fs::create_dir_all(&audio).map_err(|e| e.to_string())?;
        chmod(&audio, 0o700)?;
        let mut pulse = Command::new("pulseaudio");
        pulse.args(["-n", "--daemonize=no", "--exit-idle-time=-1"]).arg(format!("--load=module-native-protocol-unix socket={} auth-anonymous=1", audio.join("native").display())).arg("--load=module-null-sink sink_name=cw_voice");
        self.st.pulse = Some(spawn_quiet(&mut pulse)?);
        self.wait("voice_audio_server", 15, |_| Ok(audio.join("native").exists()))?;
        self.pactl(&["set-default-source", "cw_voice.monitor"])?;
        let state = PathBuf::from(std::env::var("XDG_STATE_HOME").map_err(|_| "XDG_STATE_HOME unset")?).join("cortex-workspace-dev/voice");
        std::fs::create_dir_all(&state).map_err(|e| e.to_string())?;
        let name = parse(&read_to_string(&self.st.checkout.join("packaging/voice/model.json"))?)?.get("name").and_then(Value::as_str).ok_or("voice model name unreadable")?.to_string();
        run(&["cp", "-R", &model.to_string_lossy(), &state.join(&name).to_string_lossy()], Duration::from_secs(120))?;
        self.add_project(&project)?;
        self.wait_found("voice_registered", 15, "Activate")?;
        self.key("ctrl+n")?;
        self.wait_found("voice_composer", 15, "Describe the task")?;
        self.fill("Describe the task", "Typed draft")?;
        self.key("ctrl+d")?;
        self.wait("voice_native_recording", 30, |j| Ok(j.voice_status()?.contains("Listening")))?;
        let mut playback = Command::new("paplay");
        playback.arg("--device=cw_voice").arg(fixture);
        let mut player = spawn_quiet(&mut playback)?;
        self.wait("voice_live_russian_text", 20, |j| Ok(j.voice_text()?.contains("родион")))?;
        let played = player.wait().map(|s| s.success()).unwrap_or(false);
        require(played, "voice_fixture_playback_failed")?;
        self.resize(1100, 780)?;
        sleep_ms(300);
        let layout = self.q("const b=document.querySelector('.micbar'), t=b.querySelector('.mic-text'), c=b.closest('.composer'), br=b.getBoundingClientRect(), cr=c.getBoundingClientRect(); return JSON.stringify({textWidth:t.getBoundingClientRect().width,textHeight:t.getBoundingClientRect().height,wrap:getComputedStyle(t).whiteSpace,contained:br.left>=cr.left-1 && br.right<=cr.right+1,controls:[...b.querySelectorAll('button')].every(x=>{const r=x.getBoundingClientRect();return r.width>0&&r.left>=br.left-1&&r.right<=br.right+1&&r.bottom<=br.bottom+1;})});")?;
        let fits = num(&layout, "textWidth") > 0.0 && num(&layout, "textHeight") > 0.0 && text(&layout, "wrap") == "pre-wrap" && flag(&layout, "contained") && flag(&layout, "controls");
        self.check("voice_composer_geometry", fits, outcome_pairs(&layout));
        self.key("ctrl+d")?;
        self.wait("voice_stopped_without_send", 20, |j| Ok(j.voice_status()?.contains("Ready to insert")))?;
        let draft = self.js("return document.querySelector('[aria-label=\"Describe the task\"]').value;")?;
        require(draft == "Typed draft" && self.registration_inert(&project), "voice_must_not_start_engine_or_save_task")?;
        self.press("Insert")?;
        self.wait_js("voice_insert_closed", 15, "return document.querySelector('.micbar') ? '' : 'yes';")?;
        let inserted = self.js("return document.querySelector('[aria-label=\"Describe the task\"]').value;")?;
        require(inserted.starts_with("Typed draft ") && inserted.contains("родион"), "voice_insert_lost_draft")?;
        // Mic toggles use the same capture path; cancelling preserves the earlier editable draft.
        self.js("document.querySelector('[aria-label=\"Voice input\"]').click(); return 'yes';")?;
        self.wait("voice_mic_button_restart", 30, |j| Ok(j.voice_status()?.contains("Listening")))?;
        self.js("document.querySelector('.micbar button').click(); return 'yes';")?;
        self.wait_js("voice_cancel", 15, "return document.querySelector('.micbar') ? '' : 'yes';")?;
        require(self.js("return document.querySelector('[aria-label=\"Describe the task\"]').value;")? == inserted, "voice_cancel_lost_the_draft")?;
        self.wait("voice_capture_reaped", 15, |j| Ok(j.pactl(&["list", "short", "source-outputs"])?.trim().is_empty()))?;
        require(self.registration_inert(&project), "voice_must_not_start_engine_or_save_task")?;
        // Closed native controls exercise the same real socket manager, with no test-only product route.
        let bridge = self.q("return JSON.stringify({invoke:typeof window.__TAURI_INTERNALS__?.invoke});")?;
        require(text(&bridge, "invoke") == "function", "native_invoke_not_observed")?;
        let first = self.q("const p=await window.__TAURI_INTERNALS__.invoke('workspace_request',{operation:'voice.start',message:{requestId:'voice-lease-check'}}); return JSON.stringify(p);")?;
        let duplicate = self.q("return JSON.stringify(await window.__TAURI_INTERNALS__.invoke('workspace_request',{operation:'voice.start',message:{requestId:'voice-lease-check'}}));")?;
        require(text(&first, "sessionId") == text(&duplicate, "sessionId") && !text(&first, "sessionId").is_empty(), "voice_duplicate_started_again")?;
        let busy = self.js("try {await window.__TAURI_INTERNALS__.invoke('workspace_request',{operation:'voice.start',message:{requestId:'voice-busy-check'}});return 'accepted';}catch(e){return String(e);}")?;
        require(busy.contains("voice_busy"), "voice_busy_not_bound")?;
        let wrong = self.js("try {await window.__TAURI_INTERNALS__.invoke('workspace_request',{operation:'voice.read',message:{sessionId:'wrong-session'}});return 'accepted';}catch(e){return String(e);}")?;
        require(wrong.contains("voice_session_unknown"), "voice_session_not_bound")?;
        self.wait("voice_lease_capture", 15, |j| Ok(!j.pactl(&["list", "short", "source-outputs"])?.trim().is_empty()))?;
        self.close_window()?;
        self.wait("voice_window_lease_expiry", 15, |j| Ok(j.pactl(&["list", "short", "source-outputs"])?.trim().is_empty()))?;
        self.step("voice_socket_lease", vec![("duplicate_start_same_identity", b(true)), ("busy_refused", b(true)), ("wrong_session_refused", b(true)), ("window_close_capture_reaped", b(true))]);
        self.step("native_voice_round_trip", vec![("capture", s("real PulseAudio virtual source")), ("transcription", s("real sherpa-onnx Russian fixture")), ("ctrl_d_start_stop", b(true)), ("live_text", b(true)), ("insert_preserves_draft", b(true)), ("cancel", b(true)), ("never_auto_sent", b(true)), ("engine_started", b(false))]);
        Ok(())
    }
}

