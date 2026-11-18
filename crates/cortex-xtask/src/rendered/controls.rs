//! The owner's controls over running work, each on a task of its own so one refusal cannot hide the next: stopping a command, a
//! message to one worker, and stopping one worker. The peer's scenarios (`command-restart-1`, `guidance`, `stop-siblings`)
//! provide the running work; every action is the owner's own input on the page.

use super::journey::*;
use crate::installed::util::*;
use crate::json::Res;
use std::path::Path;

impl J {
    /// Writes the scenario, saves a draft with this title and starts it. Returns the task's route.
    pub fn start_task(&mut self, project: &Path, title: &str, scenario: &str) -> Res<String> {
        write(&project.join("scenario.txt"), scenario)?;
        self.nav("New task")?;
        self.fill("Describe the task", title)?;
        self.click("Save draft")?;
        self.wait_text("task_saved", 15, title)?;
        self.open_task("task_link", title)?;
        self.click_task("Start")?;
        self.path()
    }

    /// Opens the Workers page of a task and then the first worker card whose text contains `word`.
    pub fn open_worker(&mut self, task_url: &str, word: &str) -> Res<()> {
        self.go(&format!("{task_url}/workers"))?;
        let q = crate::json::quote(word, true);
        self.wait_js("workers_loaded", 30, &format!("return [...document.querySelectorAll('a.wcard')].some(a => a.textContent.includes({q})) ? 'yes' : ''"))?;
        let href = self.q(&format!("return JSON.stringify([...document.querySelectorAll('a.wcard')].filter(a => a.textContent.includes({q})).map(a => a.getAttribute('href'))[0] ?? '');"))?;
        let href = href.as_str().unwrap_or("").to_string();
        self.open_link(&href)?;
        sleep_ms(300);
        Ok(())
    }

    fn worker_running(&self, project: &Path) -> Res<bool> {
        Ok(self.store_count(project, "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE role_id='worker-backend-dev' AND status='ready')")? == 1)
    }

    pub fn worker_controls(&mut self, project: &Path) -> Res<String> {
        // A command a worker is running through the Cortex gateway: its own marker proves it is alive, and the page must offer to stop it.
        let command_url =
            self.start_task(project, "Exercise worker controls", "command-restart-1")?;
        self.wait("real_worker_command_running", 90, |_| {
            Ok(project.join("command-entered").exists())
        })?;
        self.open_worker(&command_url, "Working")?;
        let started = self.store_count(project, "SELECT count(*) FROM runtime_events WHERE type='NATIVE_COMMAND_STARTED' AND task_id=(SELECT task_id FROM runtime_tasks ORDER BY rowid DESC LIMIT 1)")?;
        let offered = self.wait_js("running_gateway_command", 20, "return [...document.querySelectorAll('button')].some(b => b.textContent.trim() === 'Stop command…') ? 'yes' : ''").is_ok();
        if offered {
            self.press("Stop command…")?;
            self.fill(
                "Reason, sent to the worker",
                "Owner requested a focused stop",
            )?;
            self.click("Stop command")?;
            self.wait_js(
                "command_stop_dialog_closed",
                30,
                "return document.querySelector('dialog[open]') ? '' : 'yes';",
            )?;
            self.step(
                "command_stop",
                vec![(
                    "through",
                    s("real gateway process and runtime cancellation"),
                )],
            );
        } else {
            let words = self.js("return (document.querySelector('.livepane .stream')?.textContent ?? '').replace(/\\s+/g, ' ').trim().slice(0, 160);")?;
            self.fail("worker_command_stop_not_offered", vec![("running_native_command_events", n(started)), ("command_marker", b(project.join("command-entered").exists())), ("page_words", s(words)), ("note", s("a native gateway command is running but the worker page lists no command activity and no Stop command control"))]);
        }
        // The owner takes the task back; the held command is fixture-owned and ends with the service.
        self.go(&command_url)?;
        self.wait_found_task("command_task_stop_button", 15, "Stop")?;
        self.click_task("Stop")?;
        self.wait("command_task_paused", 60, |j| {
            Ok(j.has_task("Resume") && j.task_state(project)?.as_deref() == Some("PAUSED"))
        })?;
        write(&project.join("command-release"), "release")?;
        self.step("worker_command_task_paused", vec![]);

        // Guidance for one worker: the message goes to that attempt only and the same worker session observes it.
        let guidance_url = self.start_task(project, "Exercise worker guidance", "guidance")?;
        self.wait("guidance_worker_live", 60, |j| {
            Ok(j.task_state(project)?.as_deref() == Some("EXECUTING")
                && j.worker_running(project)?)
        })?;
        self.open_worker(&guidance_url, "Working")?;
        let composer_at = std::time::Instant::now();
        self.wait_js("live_worker_composer", 20, "const f = document.querySelector('[aria-label=\"Message backend-dev\"]'); return f && !f.disabled ? 'yes' : ''")?;
        self.step(
            "live_worker_composer",
            vec![("waited_ms", n(composer_at.elapsed().as_millis() as i64))],
        );
        self.fill(
            "Message backend-dev",
            "Keep the worker message verbatim\nsecond line",
        )?;
        self.click("Send to backend-dev")?;
        // The page says the guidance was recorded; the runtime queues it for that exact attempt.
        self.wait("worker_guidance_recorded", 60, |j| {
            Ok(j.store_count(
                project,
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_GUIDANCE_QUEUED'",
            )? >= 1)
        })?;
        self.wait_js("worker_guidance_receipt", 30, "return [...document.querySelectorAll('[role=alert],[role=status]')].some(e => e.textContent.includes('Recorded')) ? 'yes' : ''")?;
        let settled = self
            .wait("worker_guidance_settled", 60, |j| {
                Ok(j.store_count(
                    project,
                    "SELECT count(*) FROM runtime_events WHERE type='NATIVE_GUIDANCE_SETTLED'",
                )? >= 1)
            })
            .is_ok();
        let delivered = self.store_count(project, "SELECT count(*) FROM runtime_events WHERE type='NATIVE_GUIDANCE_SETTLED' AND json_extract(payload_json,'$.delivered')=1")?;
        self.check(
            "worker_message_recorded_and_settled",
            settled,
            vec![
                ("receipt", s("Cortex safe point, not proof of model action")),
                ("delivered_settlements", n(delivered)),
            ],
        );
        self.go(&guidance_url)?;
        self.wait_found_task("guidance_task_stop_button", 15, "Stop")?;
        self.click_task("Stop")?;
        self.wait("guidance_task_paused", 60, |j| {
            Ok(j.has_task("Resume") && j.task_state(project)?.as_deref() == Some("PAUSED"))
        })?;

        // One worker of two stops while its sibling has finished: the stop is that worker's alone.
        let stop_url = self.start_task(project, "Exercise worker stop", "stop-siblings")?;
        self.wait("sibling_finished_and_worker_held", 90, |j| Ok(j.store_count(project, "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE node_id='second' AND status='completed') AND EXISTS(SELECT 1 FROM runtime_nodes WHERE node_id='work' AND status='running')")? == 1))?;
        self.open_worker(&stop_url, "Change work.txt")?;
        self.press("Stop worker…")?;
        // Retained input diagnostic: where the real pointer events land, against the button's own box.
        let _ = self.js("window.__cwEvents = []; for (const t of ['pointerdown', 'pointerup', 'click']) document.addEventListener(t, e => { const el = e.target; window.__cwEvents.push({t, x: Math.round(e.clientX), y: Math.round(e.clientY), at: (el.getAttribute?.('aria-label') || el.textContent || el.tagName || '').trim().slice(0, 30), tag: el.tagName}); }, true); return 'armed';");
        self.click_pointer("Stop worker")?;
        let events = self.js("const b = [...document.querySelectorAll('dialog[open] button')].find(b => b.textContent.trim() === 'Stop worker'); const r = b?.getBoundingClientRect(); return JSON.stringify({events: window.__cwEvents, button: r && {x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height)}, dpr: devicePixelRatio, inner: [innerWidth, innerHeight]});").unwrap_or_default();
        let closed = self
            .wait_js(
                "worker_stop_dialog_closed",
                15,
                "return document.querySelector('dialog[open]') ? '' : 'yes';",
            )
            .is_ok();
        if closed {
            self.wait_text("worker_cancelled", 30, "Cancelled")?;
            self.step("worker_stop", vec![("through", s("runtime cancellation"))]);
        } else {
            // The dialog stays open with no refusal shown: the owner is told nothing and the worker keeps running.
            let alerts = self.js("return JSON.stringify([...document.querySelectorAll('dialog[open] [role=alert], .ws-err')].map(e => e.textContent.trim().slice(0, 160)));")?;
            let stops = self.store_count(project, "SELECT count(*) FROM runtime_events WHERE type IN ('LIVE_ASSIGNMENTS_CANCELLED','NATIVE_OWNER_CONTROL')")?;
            // Retained diagnostics: what the page offered and what the store holds for this task.
            let page = self.js("const d = document.querySelector('dialog[open]'); const b = d && [...d.querySelectorAll('button')].map(b => ({t: b.textContent.trim().slice(0, 40), l: b.getAttribute('aria-label'), dis: b.getAttribute('aria-disabled'), why: b.getAttribute('data-why')})); return JSON.stringify({path: location.pathname, buttons: b, receipt: document.querySelector('.ws-receipt')?.textContent?.trim().slice(0, 200) ?? null, focus: document.activeElement?.textContent?.trim().slice(0, 40) ?? null});")?;
            let db = self.channel.store_db(project);
            let store = crate::installed::native::query(&db, "SELECT group_concat(x, ' ; ') FROM (SELECT type||':'||substr(payload_json,1,80) AS x FROM runtime_events WHERE task_id=(SELECT task_id FROM runtime_tasks ORDER BY rowid DESC LIMIT 1) ORDER BY id DESC LIMIT 12)").unwrap_or_else(|e| e);
            let nodes = crate::installed::native::query(&db, "SELECT group_concat(n.node_id||'/'||n.status||'/'||coalesce(p.status,'-')||'/'||n.assignment_ref, ' ; ') FROM runtime_nodes n LEFT JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref WHERE n.task_id=(SELECT task_id FROM runtime_tasks ORDER BY rowid DESC LIMIT 1)").unwrap_or_else(|e| e);
            let requests = crate::installed::native::query(&db, "SELECT group_concat(operation, ' ; ') FROM (SELECT operation FROM runtime_requests ORDER BY rowid DESC LIMIT 6)").unwrap_or_else(|e| e);
            // The same stop sent straight to the service socket (not through the window) tells a
            // service-side wait from a page/bridge one.
            // A click dispatched by the page itself tells a lost input event from page logic.
            let before = self.store_count(
                project,
                "SELECT count(*) FROM runtime_requests WHERE operation='native_desktop_control'",
            )?;
            let in_page = self.js("const b = [...document.querySelectorAll('dialog[open] button')].find(b => b.textContent.trim() === 'Stop worker'); if (!b) return 'no-button'; b.click(); return 'clicked';")?;
            let page_closed = self
                .wait_js(
                    "worker_stop_dialog_closed_after_page_click",
                    10,
                    "return document.querySelector('dialog[open]') ? '' : 'yes';",
                )
                .is_ok();
            let after = self.store_count(
                project,
                "SELECT count(*) FROM runtime_requests WHERE operation='native_desktop_control'",
            )?;
            let page_click = format!(
                "{in_page}; dialog_closed={page_closed}; new_controls={}",
                after - before
            );
            let ids = crate::installed::native::query(&db, "SELECT s.project_id||'|'||s.execution_id FROM workspace_starts s JOIN workspace_executions e USING(execution_id) WHERE e.runtime_task_id=(SELECT task_id FROM runtime_tasks ORDER BY rowid DESC LIMIT 1)").unwrap_or_default();
            let worker = page
                .split("/workers/")
                .nth(1)
                .and_then(|r| r.split('"').next())
                .unwrap_or("")
                .to_string();
            let socket = self.st.socket();
            let direct = match ids.split_once('|') {
                Some((project_id, execution)) => {
                    let params = obj(vec![
                        ("projectId", s(project_id)),
                        ("executionId", s(execution)),
                        ("requestId", s("diagnostic-direct-stop")),
                        ("action", s("worker_stop")),
                        ("workerId", s(&worker)),
                    ]);
                    let (send, recv) = std::sync::mpsc::channel();
                    let began = std::time::Instant::now();
                    std::thread::spawn(move || {
                        let _ = send.send(crate::installed::seed_large::rpc(
                            &socket,
                            "executions.control",
                            params,
                        ));
                    });
                    match recv.recv_timeout(std::time::Duration::from_secs(20)) {
                        Ok(Ok(v)) => format!(
                            "ok after {} ms: {}",
                            began.elapsed().as_millis(),
                            line(&v).chars().take(200).collect::<String>()
                        ),
                        Ok(Err(e)) => format!(
                            "error after {} ms: {}",
                            began.elapsed().as_millis(),
                            e.chars().take(200).collect::<String>()
                        ),
                        Err(_) => "no reply within 20 s".to_string(),
                    }
                }
                None => format!("no execution binding: {ids}"),
            };
            self.fail(
                "worker_stop_did_not_complete",
                vec![
                    ("dialog_still_open_after_seconds", n(15)),
                    ("owner_control_events_in_store", n(stops)),
                    ("refusal_shown_in_dialog", s(alerts)),
                    ("page", s(page)),
                    ("recent_events", s(store)),
                    ("nodes", s(nodes)),
                    ("requests", s(requests)),
                    ("page_click", s(page_click)),
                    ("pointer_events", s(events)),
                    ("direct_service_stop", s(direct)),
                ],
            );
            if !page_closed {
                self.click("Keep working")?;
            }
            self.wait_js(
                "worker_stop_dialog_dismissed",
                15,
                "return document.querySelector('dialog[open]') ? '' : 'yes';",
            )?;
        }
        write(&project.join("release-sibling"), "release")?;
        self.go(&stop_url)?;
        self.wait_found_task("stop_task_stop_button", 15, "Stop")?;
        self.click_task("Stop")?;
        self.wait("stop_task_paused", 60, |j| {
            Ok(j.has_task("Resume") && j.task_state(project)?.as_deref() == Some("PAUSED"))
        })?;
        Ok(stop_url)
    }
}
