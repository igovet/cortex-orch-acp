//! The owner's controls over running work, each on a task of its own so one refusal cannot hide the next: stopping a command, a
//! message to one worker, and stopping one worker. The peer's scenarios (`command-scratch-hold`, `guidance`, `stop-siblings`)
//! provide the running work; every action is the owner's own input on the page.

use super::journey::*;
use crate::installed::{native, util::*};
use crate::json::Res;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

fn scratch_ready(output: impl Read) -> io::Result<bool> {
    const MARKER: &[u8] = b"scratch-control-ready\n";
    let mut bytes = Vec::new();
    output
        .take(MARKER.len() as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes == MARKER)
}

fn scratch_control_task(route: &str) -> Res<&str> {
    route
        .rsplit_once("/t/")
        .map(|(_, task)| task)
        .filter(|task| {
            task.strip_prefix("wt_").is_some_and(|id| {
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())
            })
        })
        .ok_or_else(|| "scratch_control_task_identity_missing".into())
}

fn scratch_output_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !name.contains("..")
        && name != "."
}

/// A timeout observation from the exact fixture task/request, not a substitute
/// for its required profile, payload marker, and cancellation receipt.
fn scratch_trace(db: &Path, task: &str) -> Res<crate::json::Value> {
    let row = native::query(db, &format!("SELECT
        CASE json_extract(result_json,'$.handle.profile')
          WHEN 'codex_scratch' THEN 'codex_scratch'
          WHEN 'development' THEN 'development'
          WHEN 'guarded' THEN 'guarded'
          ELSE CASE WHEN json_type(result_json,'$.handle') IS NULL THEN 'unbound' ELSE 'other' END
        END,
        coalesce(json_extract(result_json,'$.handle.output'),''),
        coalesce(json_extract(result_json,'$.handle.reference')=json_extract(result_json,'$.tool_call_ref'),0),
        coalesce(json_extract(result_json,'$.settled'),0),
        EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=r.task_id
          AND e.type='NATIVE_COMMAND_PROCESS'
          AND json_extract(e.payload_json,'$.ref')=json_extract(r.result_json,'$.tool_call_ref'))
        FROM runtime_requests r WHERE operation='command_run'
        AND request_key='scratch-command-control' AND task_id='{task}'"))?;
    if row.is_empty() {
        return Ok(obj(vec![("request", b(false))]));
    }
    let [profile, output, reference, settled, process]: [&str; 5] = row
        .split('|')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| "scratch_control_trace_ambiguous")?;
    let marker = if reference != "1" || !scratch_output_name(output) {
        "unbound"
    } else {
        let path = db.with_file_name(task).join(".native-output").join(output);
        match File::open(path) {
            Ok(file) => match scratch_ready(file) {
                Ok(true) => "exact",
                Ok(false) => "not_exact",
                Err(_) => "unreadable",
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => "absent",
            Err(_) => "unreadable",
        }
    };
    Ok(obj(vec![
        ("request", b(true)),
        ("handle_profile", s(profile)),
        ("reference_bound", b(reference == "1")),
        ("process_recorded", b(process == "1")),
        ("settled", b(settled == "1")),
        ("output_marker", s(marker)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::{scratch_control_task, scratch_output_name, scratch_ready};

    #[test]
    fn scratch_control_uses_the_open_task_and_a_plain_output_name() {
        assert_eq!(
            scratch_control_task("/m/local/t/wt_abc123").unwrap(),
            "wt_abc123"
        );
        for route in [
            "/m/local/t/",
            "/m/local/t/wt_abc'/",
            "/m/local/t/wt_abc/workers",
        ] {
            assert!(scratch_control_task(route).is_err(), "{route}");
        }
        assert!(scratch_output_name("nc-abc_123.bin"));
        for name in ["", ".", "..", "../output", "/tmp/output", "x|y"] {
            assert!(!scratch_output_name(name), "{name}");
        }
    }

    #[test]
    fn scratch_control_requires_exact_ready_output() {
        assert!(!scratch_ready(&b""[..]).unwrap());
        assert!(!scratch_ready(&b"scratch-control-ready"[..]).unwrap());
        assert!(!scratch_ready(&b"scratch-control-ready\nextra"[..]).unwrap());
        assert!(scratch_ready(&b"scratch-control-ready\n"[..]).unwrap());
    }
}

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
        // A bound supervisor is not proof that its payload started. Read the
        // fixture's retained output before offering it a Stop click; after the
        // click, still require cancellation and output on that same request.
        let command_url =
            self.start_task(project, "Exercise worker controls", "command-scratch-hold")?;
        let workspace_task = scratch_control_task(&command_url)?;
        let db = self.channel.store_db(project);
        self.step("scratch_control_task_opened", vec![("task", s(workspace_task))]);
        let mut task = String::new();
        let ready = self.wait("scratch_payload_ready", 90, |_| {
            if task.is_empty() {
                let id = native::query(&db, &format!("SELECT e.runtime_task_id
                    FROM workspace_tasks w JOIN workspace_executions e
                    ON e.execution_id=w.execution_id AND e.task_id=w.task_id
                    WHERE w.task_id='{workspace_task}'"))?;
                if id.is_empty() {
                    return Ok(false);
                }
                if !scratch_output_name(&id) {
                    return Err("scratch_control_execution_invalid".into());
                }
                task = id;
            }
            let output = native::query(&db, &format!("SELECT json_extract(result_json,'$.handle.output')
                FROM runtime_requests WHERE operation='command_run'
                AND request_key='scratch-command-control' AND task_id='{task}'
                AND json_extract(result_json,'$.handle.profile')='codex_scratch'
                AND json_extract(result_json,'$.handle.reference')=json_extract(result_json,'$.tool_call_ref')
                AND json_extract(result_json,'$.settled') IS NOT 1"))?;
            if output.is_empty() {
                return Ok(false);
            }
            if !scratch_output_name(&output) {
                return Err("scratch_payload_handle_invalid".into());
            }
            let file = db.with_file_name(&task).join(".native-output").join(output);
            match File::open(file) {
                Ok(file) => scratch_ready(file).map_err(|e| format!("scratch_payload_read:{}", e.kind())),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(format!("scratch_payload_open:{}", e.kind())),
            }
        });
        if let Err(error) = ready {
            let evidence = if task.is_empty() {
                obj(vec![("execution_bound", b(false))])
            } else {
                scratch_trace(&db, &task)
                    .unwrap_or_else(|_| obj(vec![("trace_unavailable", b(true))]))
            };
            super::session::outcome(
                &mut self.run,
                "scratch_control_trace",
                "diagnostic",
                vec![
                    ("task", s(workspace_task)),
                    ("runtime_task", s(&task)),
                    ("evidence", evidence),
                ],
            );
            return Err(error);
        }
        self.open_worker(&command_url, "Working")?;
        let started = self.store_count(project, &format!("SELECT count(*) FROM runtime_events WHERE type='NATIVE_COMMAND_STARTED' AND task_id='{task}'"))?;
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
            self.wait("scratch_payload_stopped", 30, |j| {
                Ok(j.store_count(project, &format!("SELECT count(*) FROM runtime_requests WHERE operation='command_run'
                    AND request_key='scratch-command-control' AND task_id='{task}'
                    AND json_extract(result_json,'$.profile')='codex_scratch' AND json_extract(result_json,'$.settled')=1
                    AND json_extract(result_json,'$.failure')='owner_stopped'
                    AND trim(json_extract(result_json,'$.output'),char(10))='scratch-control-ready'"))? == 1)
            })?;
            self.step(
                "command_stop",
                vec![(
                    "through",
                    s("real gateway process and runtime cancellation"),
                )],
            );
        } else {
            let words = self.js("return (document.querySelector('.livepane .stream')?.textContent ?? '').replace(/\\s+/g, ' ').trim().slice(0, 160);")?;
            self.fail("worker_command_stop_not_offered", vec![("running_native_command_events", n(started)), ("page_words", s(words)), ("note", s("a native gateway command is running but the worker page lists no command activity and no Stop command control"))]);
        }
        // The owner takes the task back after stopping its command.
        self.go(&command_url)?;
        self.wait_found_task("command_task_stop_button", 15, "Stop")?;
        self.click_task("Stop")?;
        self.wait("command_task_paused", 60, |j| {
            Ok(j.has_task("Resume") && j.task_state(project)?.as_deref() == Some("PAUSED"))
        })?;
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
        // The worker composer's action button is Stop while the worker runs and its field is empty; it has no
        // visible text, so it is found by its accessible name, as Send is.
        self.click("Stop worker…")?;
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
