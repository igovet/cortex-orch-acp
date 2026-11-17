//! The screens and the owner journey proper, in order: no project in dark and light, then one project through the native service
//! and the no-paid peer.

use super::a11y;
use super::journey::*;
use super::session::*;
use super::{ENGINE};
use crate::installed::util::*;
use crate::json::Value;
use crate::json::Res;
use std::path::PathBuf;

pub(super) const DRAFT_TITLE: &str = "Explain this disposable project";
const CONSTRAINT: &str = " Never lose constraint α";

impl J {
    pub fn settings_screens(&mut self, label: &str, theme: &str) -> Res<()> {
        self.js("document.querySelector('a[aria-label=\"Settings\"]').click(); return 'yes';")?;
        sleep_ms(300);
        let links = self.q("return JSON.stringify([...document.querySelectorAll('.sidebar a.snav')].filter(a => a.getAttribute('href')).map(a => a.getAttribute('href')));")?;
        for href in links.as_arr().unwrap_or(&[]).iter().filter_map(Value::as_str) {
            self.js(&format!("document.querySelector('.sidebar a.snav[href={}]').click(); return 'yes';", crate::json::quote(href, true)))?;
            self.audit(&format!("settings {href} {label}"), theme)?;
        }
        self.popovers(&format!("settings {label}"))?;
        self.js("[...document.querySelectorAll('.sidebar .nav')].find(e => e.textContent.includes('Back to workspace')).click(); return 'yes';")?;
        sleep_ms(200);
        Ok(())
    }

    pub fn common_screens(&mut self, label: &str, theme: &str) -> Res<()> {
        let mut baseline: Option<String> = None;
        let entry = self.q("const a = [...document.querySelectorAll('.sidebar .nav')].find(e => e.textContent.trim().startsWith('Archive')); return JSON.stringify({ disabled: a.getAttribute('aria-disabled'), href: a.getAttribute('href') });")?;
        if text(&entry, "disabled") == "true" || !flag(&entry, "href") {
            self.fail("archive_entry_is_off", vec![("archive", entry)]);
        }
        for screen in ["Home", "Inbox", "Archive", "New task"] {
            self.nav(screen)?;
            let report = self.audit(&format!("{} {label}", screen.to_lowercase()), theme)?;
            // Each row without its name: the navigation must not move between screens.
            let rows: String = at(&report, "ui.navs").and_then(Value::as_arr).map(|a| a.iter().map(|r| canon(&Value::Arr(r.as_arr().unwrap_or(&[]).iter().skip(1).cloned().collect()))).collect::<Vec<_>>().join(";")).unwrap_or_default();
            match &baseline {
                None => baseline = Some(rows),
                Some(first) if *first != rows => {
                    let before = first.clone();
                    self.fail("navigation_moved", vec![("screen", s(screen)), ("before", s(before)), ("after", s(rows))]);
                }
                _ => {}
            }
        }
        self.nav("Home")?;
        self.js("[...document.querySelectorAll('.sidebar .nav')].find(e => e.textContent.includes('Search')).click(); return 'yes';")?;
        self.wait_js("search_palette", 15, "return document.querySelector('dialog[open]') ? 'yes' : ''")?;
        self.audit(&format!("search {label}"), theme)?;
        self.key("Escape")?;
        self.wait_js("search_palette_closed", 15, "return document.querySelector('dialog[open]') ? '' : 'yes'")?;
        self.js("[...document.querySelectorAll('.sb-foot button')].find(b => b.textContent.includes('Add project')).click(); return 'yes';")?;
        sleep_ms(200);
        self.audit(&format!("add-project dialog {label}"), theme)?;
        self.key("Escape")?;
        sleep_ms(200);
        self.settings_screens(label, theme)
    }

    /// WebKitGTK against the Chromium the mockups were reviewed in.
    pub fn engine_features(&mut self) -> Res<()> {
        let engine = self.st.query(ENGINE, 15)?;
        let needed = ["popover", "dialog", "colorMix", "has", "focusVisible", "container", "scrollbarGutter", "mask"];
        let ok = needed.iter().all(|k| flag(&engine, k)) && num(&engine, "scrollbar") == 11.0 && num(&engine, "cyrillicDesign") != num(&engine, "cyrillicSans") && flag(&engine, "pageMenu") && text(&engine, "linkDrag") == "none";
        if ok {
            self.step("webkitgtk_features", outcome_pairs(&engine));
        } else {
            self.fail("webkitgtk_feature_gap", vec![("engine", engine)]);
        }
        Ok(())
    }

    /// The window as the first screen of every launch: found by its own controls, then its binary identity.
    pub fn bundled_window(&mut self) -> Res<()> {
        let node = self.retry_stale(|j| j.wait_node("Add project"))?;
        let (_, _, w, h) = self.retry_stale(|_| node.acc.extents(crate::installed::ffi::COORD_SCREEN))?;
        if w <= 0 || h <= 0 {
            return Err("rendered_control_has_no_geometry".into());
        }
        let binary = sha_file(&self.st.artifacts.join(self.st.ns("desktop_binary")))?;
        self.step("bundled_native_window", vec![("binary_sha256", s(binary)), ("evidence", s("WebKitGTK accessible rendered control has nonzero screen geometry"))]);
        Ok(())
    }

    /// No project, dark then light: every common screen, the popovers and the window closes.
    pub fn no_project_screens(&mut self) -> Res<()> {
        self.launch("dark")?;
        self.bundled_window()?;
        self.loaded()?;
        self.audit("home at launch", "dark")?;
        self.engine_features()?;
        if self.st.wayland {
            let native = self.q("@native")?;
            let ok = text(&native, "backend") == "GdkWaylandDisplay" && !flag(&native, "decorated") && line(at(&native, "top").unwrap_or(&Value::Null)) == line(at(&native, "web").unwrap_or(&Value::Null));
            self.check("full_journey_wayland_backend", ok, vec![("native", native)]);
            if !ok {
                return Err("wayland_backend_is_not_frameless".into());
            }
        } else {
            window_behaviour(&mut self.run, &mut self.st)?;
        }
        self.common_screens("no project", "dark")?;
        self.popovers("home no project")?;
        self.close_button()?;
        self.relaunch("light", "light_window")?;
        let surface = self.q("@native")?;
        let white = (0..3).all(|i| num(&surface, &format!("background.{i}")) == 255.0);
        if !white {
            self.fail("window_background_is_not_the_surface", vec![("native", surface)]);
        }
        self.common_screens("no project", "light")?;
        self.popovers("home no project light")?;
        self.close_window()
    }

    /// What the page says about the project-folder choices of the task form, and whether a separate worktree is offered.
    pub fn worktree_choice(&mut self, project: &PathBuf) -> Res<()> {
        let radios = self.q("return JSON.stringify([...document.querySelectorAll('label.rc')].map(l => { const i = l.querySelector('input'); return {label: l.textContent.trim().replace(/\\s+/g, ' ').slice(0, 160), disabled: !!i?.disabled, title: l.title || i?.title || ''}; }));")?;
        let separate = radios.as_arr().unwrap_or(&[]).iter().find(|r| text(r, "label").contains("Separate worktree")).cloned();
        match separate {
            Some(radio) if !flag(&radio, "disabled") => self.step("task_worktree_choice_and_helper_exclusion", vec![("choices", radios)]),
            Some(radio) => {
                let words = text(&radio, "label").to_string();
                let title = text(&radio, "title").to_string();
                self.not_applicable("task_worktree_choice", "the native host reports task worktrees unavailable", obj(vec![("label", s(words)), ("title", s(title))]));
            }
            None => self.not_applicable("task_worktree_choice", "the task form offers no separate-worktree choice on this host", obj(vec![("choices", radios)])),
        }
        let status = capture(std::process::Command::new("git").args(["status", "--porcelain"]).current_dir(project), std::time::Duration::from_secs(20))?;
        let excluded = status.code == 0 && !status.stdout.contains(".cortex-dev-owner.json");
        self.check("helper_file_excluded_from_git", excluded, vec![]);
        Ok(())
    }
}

pub(super) fn drive(j: &mut J) -> Res<()> {
    if j.st.wayland {
        j.st.start_wayland()?;
    } else {
        j.st.start_display()?;
        j.st.input = Some(crate::installed::input::Input::new("x11")?);
    }
    j.st.start_service()?;
    let socket = j.st.socket();
    wait_secs("service socket", 20, || Ok(socket.exists()))?;
    let only = j.only.clone();
    let wanted = |name: &str| only.as_deref().is_none_or(|o| o == name);
    if wanted("common") {
        j.no_project_screens()?;
    }
    // The owner controls on their own, to iterate on them without the whole journey.
    if only.as_deref() == Some("controls") {
        let project = j.home.join("journey");
        j.launch("dark")?;
        j.wait_found("controls_window", 15, "Add project")?;
        j.loaded()?;
        j.add_project(&project)?;
        j.wait_found("registered_project_visible", 15, "Activate")?;
        j.click("Activate")?;
        j.wait_gone("activation_finished", 60, "Activate")?;
        j.worker_controls(&project)?;
        return j.close_window();
    }
    if wanted("lifecycle") {
        j.lifecycle()?;
    }
    // The frame and controls stage is the owner's Wayland session next to the X11 journey; the Wayland journey is that session.
    if wanted("wayland") && !j.st.wayland {
        j.wayland_stage()?;
    }
    if wanted("terminal") {
        j.terminal_stage()?;
    }
    if wanted("voice") {
        j.voice_stage()?;
    }
    if wanted("attachments") {
        j.attachments_stage()?;
    }
    Ok(())
}


pub(super) fn finish(j: &mut J) -> Res<()> {
    // The common journey has closed the app before final evidence; the service stays up, the compositor too.
    stop(&mut j.st.app);
    let evidence = super::entry::native_evidence(j, "");
    let unexpected = evidence.get("web_process_terminations").and_then(Value::as_arr).is_some_and(|a| !a.is_empty());
    let cores = evidence.get("cores").and_then(Value::as_arr).is_some_and(|a| !a.is_empty());
    let reasons = evidence.get("web_process_terminations").cloned().unwrap_or(Value::Null);
    let mut pairs = outcome_pairs(&evidence);
    pairs.retain(|(k, _)| *k != "app");
    pairs.push(("app", Value::Null));
    // No screenshot is captured or read by this harness; the count is part of the evidence.
    pairs.push(("screenshots", n(0)));
    outcome(&mut j.run, "run_evidence", "diagnostic", pairs);
    if unexpected {
        j.fail("web_process_terminated", vec![("reasons", reasons), ("requested_closes", n(j.st.closes as i64))]);
    }
    if cores {
        j.fail("process_left_a_core", vec![("cores", evidence.get("cores").cloned().unwrap_or(Value::Null))]);
    }
    if j.run.failures.is_empty() {
        outcome(&mut j.run, "visual_audit_summary", "passed", vec![("failures", n(0))]);
    } else {
        let mut names = j.run.failures.clone();
        names.sort();
        names.dedup();
        let count = j.run.failures.len() as i64;
        outcome(&mut j.run, "visual_audit_summary", "failed", vec![("failures", n(count)), ("names", arr(names.iter().map(|x| s(x.as_str())).collect()))]);
    }
    Ok(())
}

/// Only names and roles from the exact owned app are retained, never raw logs.
pub(super) fn failure_diagnostics(j: &mut J) {
    if j.st.app.is_none() || j.st.app_status().is_some() {
        return;
    }
    let status = j.jsn("return document.querySelector('.terminal-status')?.textContent ?? '';", 3).unwrap_or_default();
    j.note("terminal_at_failure", vec![("terminal_status", s(status))]);
    if let Ok(state) = j.st.query("return JSON.stringify({alerts:[...document.querySelectorAll('[role=alert],[role=status],.ws-err,.ws-conn,.toast')].map(e=>e.textContent.trim().slice(0,160)),buttons:[...document.querySelectorAll('dialog[open] button')].map(b=>[b.textContent.trim().slice(0,24),b.disabled,b.getAttribute('aria-disabled'),b.dataset.why??'']),open:[...document.querySelectorAll('dialog[open]')].map(d=>d.getAttribute('aria-label')),receipt:document.querySelector('.ws-receipt')?.textContent??null,focus:document.activeElement?.tagName});", 3) {
        j.note("dialog_at_failure", vec![("state", state)]);
    }
    let route = j.jsn("return location.pathname;", 3).unwrap_or_default();
    let service_log = std::fs::metadata(j.st.scratch.join("service.log")).map(|m| m.len() as i64).unwrap_or(0);
    let visible = j.jsn("return (document.querySelector('.main')?.textContent ?? '').replace(/\\s+/g, ' ').trim().slice(0, 1500);", 3).unwrap_or_default();
    j.note("page_at_failure", vec![("route", s(route)), ("service_log_bytes", n(service_log)), ("main_text", s(visible))]);
    let nodes = a11y::names(j.group(), 90);
    j.note("owned_accessibility_diagnostic", vec![("nodes", arr(nodes.into_iter().map(|(role, name)| obj(vec![("role", s(role)), ("name", s(name))])).collect()))]);
    stop(&mut j.st.app);
}

impl J {
    /// Registering and activating the project, then the task form, the draft and the second project.
    pub fn lifecycle(&mut self) -> Res<()> {
        let project = self.home.join("journey");
        let second = self.home.join("second");
        self.launch("dark")?;
        self.wait_found("journey_window", 15, "Add project")?;
        self.loaded()?;
        self.add_project(&project)?;
        self.wait_found("registered_project_visible", 15, "Activate")?;
        let inert = self.registration_inert(&project);
        self.check("registration_is_inert", inert, vec![("channel", s("dev"))]);
        self.audit("project before activation", "dark")?;
        self.click("Activate")?;
        self.wait_gone("activation_finished", 60, "Activate")?;
        self.step("activation", vec![]);
        self.audit("project board empty", "dark")?;
        self.popovers("project board")?;
        self.click("New task")?;
        self.audit("new task in project", "dark")?;
        self.worktree_choice(&project)?;
        self.click("Sprint")?;
        let route = self.yes("return document.querySelector('.c-route') && ![...document.querySelectorAll('.c-route button')].some(b => b.title.includes('sprint route changes are unavailable')) ? 'yes' : 'no';")?;
        self.check("sprint_route_control_not_placeholder", route, vec![]);
        self.audit("new sprint configured route", "dark")?;
        self.click("Task")?;
        self.fill("Describe the task", DRAFT_TITLE)?;
        self.paste(CONSTRAINT)?;
        self.wait_js("constraint_pasted", 10, "return [...document.querySelectorAll('textarea')].some(t => t.value.includes('Never lose constraint')) ? 'yes' : ''")?;
        self.release_clipboards();
        self.click("Save draft")?;
        self.wait_text("draft_visible", 15, DRAFT_TITLE)?;
        let started = self.store_count(&project, "SELECT count(*) FROM runtime_tasks").unwrap_or(0);
        self.check("description_only_draft_does_not_start", started == 0, vec![("runtime_tasks", n(started))]);
        self.audit("project board with draft", "dark")?;
        self.inspector_open("project board with draft")?;
        self.add_project(&second)?;
        self.wait_js("second_project_visible", 15, "return document.querySelectorAll('.proj').length === 2 ? 'yes' : ''")?;
        self.step("two_projects", vec![]);
        self.common_screens("two projects", "dark")?;
        self.open_project("journey")?;
        self.open_task("task_link", DRAFT_TITLE)?;
        sleep_ms(300);
        self.audit("task session draft", "dark")?;
        self.inspector_open("task session draft")?;
        self.click("Start")?;
        self.wait_text("runtime_question", 90, "How should we finish?")?;
        self.step("start_and_question", vec![]);
        self.audit("task session question", "dark")?;
        self.popovers("task session question")?;
        self.resize(960, 540)?;
        sleep_ms(500);
        self.audit("task session at minimum size", "dark")?;
        self.resize(1440, 900)?;
        sleep_ms(400);
        self.close_window()?;
        self.relaunch("light", "reopen_window")?;
        self.open_project("journey")?;
        self.audit("project board light", "light")?;
        self.open_task("reopen_question_link", DRAFT_TITLE)?;
        self.wait_text("reopened_question", 30, "How should we finish?")?;
        let alive = self.st.service.as_mut().is_some_and(|c| c.try_wait().ok().flatten().is_none());
        self.check("question_survives_window_close", alive, vec![]);
        self.audit("task session question light", "light")?;
        self.fill("Your answer", "first line\nsecond line")?;
        self.click("Send answer")?;
        self.wait("answer_delivered", 60, |j| Ok(j.store_count(&project, "SELECT count(*) FROM runtime_events WHERE type='USER_ANSWER_RECEIVED'")? >= 1))?;
        self.step("multiline_answer", vec![]);
        self.wait("runtime_result", 90, |j| Ok(j.task_state(&project)?.as_deref() == Some("COMPLETED") && j.text_seen("Owner decision explained.")))?;
        self.step("runtime_owned_result", vec![]);
        self.audit("task session result", "light")?;
        self.inspector_open("task session result")?;
        let task_at = self.path()?;
        for href in self.follow("a.td-link")? {
            self.open_link(&href)?;
            sleep_ms(200);
            self.audit(&format!("result tab {}", href.rsplit('/').next().unwrap_or("")), "light")?;
        }
        self.go(&task_at)?;
        sleep_ms(300);
        // A finished worker's stream is its whole history, read-only, with its result at the end.
        let finished = self.follow("a.wins")?;
        self.open_link(&finished[0])?;
        sleep_ms(300);
        let history = self.yes("return document.querySelector('.livepane .stream .stream-in') && !document.querySelector('.livepane .dock-w textarea:not([disabled])') ? 'yes' : '';")?;
        self.check("finished_worker_history_is_read_only", history, vec![]);
        self.audit("actual finished worker", "light")?;
        self.go(&task_at)?;
        sleep_ms(300);
        let before_head = self.js("return document.querySelector('.rcard-h')?.textContent ?? '';")?;
        self.js("window.__cwReviewTrigger = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Request changes…'); window.__cwReviewTrigger.focus(); return 'yes';")?;
        self.press("Request changes…")?;
        self.fill("What should change", "Owner review feedback")?;
        let submitted = std::time::Instant::now();
        self.click("Send and reopen")?;
        self.wait_js("review_dialog_closed", 30, "return document.querySelector('dialog[open]') ? '' : 'yes';")?;
        let focus = self.q("const trigger = window.__cwReviewTrigger; return JSON.stringify({triggerConnected:trigger.isConnected,focusReturned:document.activeElement === (trigger.isConnected ? trigger : document.querySelector('main')),active:document.activeElement?.tagName,explicitInert:document.querySelectorAll('[inert]').length});")?;
        let ok = flag(&focus, "focusReturned") && num(&focus, "explicitInert") == 0.0;
        let mut pairs = vec![("elapsed_ms", n(submitted.elapsed().as_millis() as i64))];
        pairs.extend(outcome_pairs(&focus));
        self.check("review_dialog_closed", ok, pairs);
        let before = before_head;
        self.wait("review_returns_to_cortex", 90, |j| {
            let head = j.js("return document.querySelector('.rcard-h')?.textContent ?? '';")?;
            Ok(j.task_state(&project)?.as_deref() == Some("COMPLETED") && head.starts_with("Result v") && head != before)
        })?;
        let now = self.js("return document.querySelector('.rcard-h')?.textContent ?? '';")?;
        self.step("review_feedback_and_second_result", vec![("previous", s(before)), ("now", s(now))]);
        // A held worker keeps the task executing: the streaming, queue and steer screens need a turn that stays open.
        let hold = "Hold the turn open for streaming";
        write(&project.join("scenario.txt"), "hold")?;
        self.nav("New task")?;
        self.fill("Describe the task", hold)?;
        self.click("Save draft")?;
        self.wait_text("hold_draft_visible", 15, hold)?;
        self.open_task("hold_task_link", hold)?;
        self.click("Start")?;
        self.wait("worker_live", 60, |j| Ok(j.task_state(&project)?.as_deref() == Some("EXECUTING") && j.store_count(&project, "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE role_id='worker-backend-dev' AND status='ready')")? == 1))?;
        self.step("start_held_task", vec![]);
        let hold_at = self.path()?;
        self.close_window()?;
        self.relaunch("dark", "reopen_streaming_window")?;
        self.open_project("journey")?;
        self.open_task("streaming_task_link", hold)?;
        self.wait("observed_streaming", 30, |j| Ok(j.text_seen("Executing") || j.text_seen("Working") || j.text_seen("Planning")))?;
        let executing = self.task_state(&project)?.as_deref() == Some("EXECUTING");
        self.check("streaming_survives_window_close", executing, vec![]);
        self.audit("task session streaming", "dark")?;
        self.inspector_open("task session streaming")?;
        // Saved on the machine but not delivered; Edit restores the composer without starting a turn.
        self.fill("Message", "Queued from the audit")?;
        self.click("Queue")?;
        self.click("Queue message")?;
        self.wait_js("queued_message_visible", 15, "return document.querySelector('.queue') ? 'yes' : ''")?;
        self.audit("task session with a queued message", "dark")?;
        self.close_window()?;
        self.relaunch("dark", "reopen_durable_queue")?;
        self.open_project("journey")?;
        self.open_task("durable_queue_task_link", hold)?;
        self.wait_text("durable_queue_restored", 30, "Queued from the audit")?;
        let lost = self.text_seen("may be lost if the app closes");
        self.check("queue_survives_window_close", !lost, vec![]);
        self.click("Edit")?;
        self.wait_js("edit_returns_to_the_field", 15, "return !document.querySelector('.queue') && document.querySelector('[aria-label=\"Message\"]').value === 'Queued from the audit' ? 'yes' : ''")?;
        self.step("queue_and_edit", vec![]);
        self.fill("Message", "Focus on durable conversation")?;
        self.click("Send now")?;
        self.click("Send message now")?;
        self.wait_js("steer_recorded_during_turn", 30, "return document.body.innerText.includes('Focus on durable conversation') && document.querySelector('[aria-label=\"Message\"]').value === '' ? 'yes' : ''")?;
        self.step("steer_durable_capture_during_turn", vec![]);
        self.wait_found_role("workers_in_the_conversation", 15, "backend-dev", "link")?;
        self.step("workers_from_the_real_store", vec![("through", s("service, native peer, port and screen"))]);
        self.audit("task session with workers", "dark")?;
        self.inspector_open("task session with workers")?;
        let tabs = self.follow("a.td-link")?;
        for href in &tabs {
            self.open_link(href)?;
            sleep_ms(200);
            self.audit(&format!("task tab {}", href.rsplit('/').next().unwrap_or("")), "dark")?;
        }
        let workers_tab = tabs.iter().find(|h| h.ends_with("/workers")).cloned().ok_or("no_workers_tab")?;
        self.open_link(&workers_tab)?;
        sleep_ms(200);
        for href in self.follow(".wlist a.wcard")? {
            self.open_link(&href)?;
            sleep_ms(200);
            self.audit(&format!("worker {}", href.rsplit('/').next().unwrap_or("")), "dark")?;
            self.popovers(&format!("worker {}", href.rsplit('/').next().unwrap_or("")))?;
        }
        let live = self.q("return JSON.stringify([...document.querySelectorAll('a.wcard')].filter(a => a.textContent.includes('Working')).map(a => a.getAttribute('href')));")?;
        let live_href = live.as_arr().and_then(|a| a.first()).and_then(Value::as_str).ok_or("no_running_worker_card")?.to_string();
        self.open_link(&live_href)?;
        sleep_ms(300);
        // The worker's card is the way to its live view: there is no intermediate pane or Full view switch, so the opened page itself
        // holds the running worker's heading, its activity stream and its message composer.
        let live_view = self.yes("return location.pathname.includes('/workers/') && document.querySelector('.livepane .d-h .d-title') && document.querySelector('.livepane .stream .stream-in') && document.querySelector('.livepane .dock-w textarea') ? 'yes' : '';")?;
        self.check("worker_live_view", live_view, vec![]);
        self.audit("worker live view", "dark")?;
        // The owner takes the held turn back: Stop pauses the task, and Resume with the hold lifted lets the worker finish.
        self.go(&hold_at)?;
        self.wait_found("task_stop_button", 15, "Stop")?;
        write(&project.join("scenario.txt"), "")?;
        self.click("Stop")?;
        self.wait("task_paused", 60, |j| Ok(j.has("Resume") && !j.has("Stop") && j.task_state(&project)?.as_deref() == Some("PAUSED")))?;
        self.click("Resume")?;
        self.wait("runtime_result_after_hold", 90, |j| Ok(j.task_state(&project)?.as_deref() == Some("COMPLETED") && j.text_seen("Fixture result.")))?;
        let accepted = self.text_seen("accepted by you");
        let first_line = self.js("return document.querySelector('.rcard-h')?.textContent ?? '';")?;
        self.check("runtime_owned_result", !accepted && first_line.starts_with("Result v"), vec![("heading", s(first_line))]);
        self.audit("task session result after hold", "dark")?;
        self.inspector_open("task session result after hold")?;
        let guidance_url = self.worker_controls(&project)?;
        self.project_views(&guidance_url)?;
        self.offline_screens(&guidance_url)

    }
}

impl J {
    /// The project board with a result, a real local Git refusal that stays readable inside its still-modal form, and every project view.
    fn project_views(&mut self, task_at: &str) -> Res<()> {
        self.open_project("journey")?;
        sleep_ms(300);
        self.audit("project board with a result", "dark")?;
        self.popovers("project board with a result")?;
        let git = self.follow(".ptab")?.into_iter().find(|h| h.ends_with("/git")).ok_or("no_git_view")?;
        self.open_link(&git)?;
        self.wait_js("branch_dialog_trigger", 15, "return [...document.querySelectorAll('button')].some(b => b.textContent.trim() === 'Create branch here') ? 'yes' : '';")?;
        self.js("window.__cwBranchTrigger = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Create branch here'); window.__cwBranchTrigger.focus(); return 'yes';")?;
        self.press("Create branch here")?;
        self.fill("Branch name", "main")?;
        self.click("Create branch")?;
        self.wait_js("branch_refusal_inside_modal", 15, "return document.querySelector('dialog[open] [role=alert]')?.textContent.trim() ? 'yes' : '';")?;
        let kept = self.js("return document.querySelector('dialog[open] input').value;")? == "main";
        self.check("branch_form_retained", kept, vec![]);
        self.audit("branch refusal stays in dialog", "dark")?;
        self.click("Cancel")?;
        self.wait_js("branch_cancel_closed", 15, "return document.querySelector('dialog[open]') ? '' : 'yes';")?;
        let focused = self.js("return document.activeElement === window.__cwBranchTrigger ? 'yes' : '';")? == "yes";
        self.check("dialog_refusal_and_cancel_focus", focused && kept, vec![("form_retained", b(kept)), ("focus_returned", b(focused))]);
        self.open_project("journey")?;
        // A view may carry its own query on these links, so each is found again by its place.
        let views = self.follow(".ptab")?;
        for (index, href) in views.iter().enumerate() {
            self.js(&format!("document.querySelectorAll('.ptab')[{index}].click(); return 'yes';"))?;
            sleep_ms(400);
            let name = href.rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
            self.audit(&format!("project view {name}"), "dark")?;
        }
        self.open_project("journey")?;
        sleep_ms(200);
        self.go(task_at)?;
        let want = task_at.to_string();
        self.wait("back_to_the_task", 15, |j| Ok(j.path()? == want))
    }

    /// The service goes away under an open window and again before a launch: the page says so and keeps what it last confirmed.
    fn offline_screens(&mut self, task_at: &str) -> Res<()> {
        let _ = task_at;
        stop(&mut self.st.service);
        self.wait_text("disconnect_visible", 30, "Offline")?;
        self.wait("last_confirmed_age_visible", 30, |j| Ok(j.text_seen("Last confirmed") && j.text_seen("old")))?;
        self.step("disconnected_state", vec![]);
        self.audit("task session offline", "dark")?;
        self.nav("Home")?;
        self.audit("home offline with project", "dark")?;
        self.close_window()?;
        self.relaunch("light", "offline_start")?;
        self.wait_js("offline_banner", 15, "return document.querySelector('.ws-conn') ? 'yes' : ''")?;
        self.audit("home offline at launch", "light")?;
        self.popovers("offline light")?;
        self.close_window()?;
        let socket = self.st.socket();
        let _ = std::fs::remove_file(&socket);
        self.st.start_service()?;
        wait_secs("service_restart", 20, || Ok(socket.exists()))?;
        Ok(())
    }
}
