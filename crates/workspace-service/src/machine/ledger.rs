//! Durable owner-operation intents. Dispatched external effects are never relaunched by key.
//! Only identity/digests and sanitized replies are retained, never Git argv, URLs or stderr.
use super::{
    files::revision,
    native::*,
    settings::{self, Lock},
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
pub struct Intent {
    path: PathBuf,
    record: Value,
}
pub enum Begin {
    Run(Intent),
    Replay(Value),
}
fn address(
    state: &Path,
    op: &str,
    project: &str,
    root: Option<&Path>,
    params: &Value,
) -> Result<(PathBuf, String)> {
    let key = if let Some(value) = params.get("requestId") {
        value
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_:.-".contains(&b))
            })
            .ok_or_else(|| refuse("invalid_request", "Invalid operation request identity."))?
            .to_owned()
    } else {
        settings::unique()
    };
    let binding = json!({"owner":crate::local::uid(),"channel":workspace_protocol::channel::current().name,"project":project,"key":key});
    let identity = revision(binding.to_string().as_bytes());
    let digest = revision(
        json!({"op":op,"root":root,"params":params})
            .to_string()
            .as_bytes(),
    );
    Ok((
        state.join("operations").join(format!("{identity}.json")),
        digest,
    ))
}
fn existing(path: &Path, digest: &str) -> Result<Option<Value>> {
    let saved = settings::read_json(path, Value::Null)?;
    if saved.is_null() {
        return Ok(None);
    }
    if saved["digest"] != digest {
        return Err(refuse(
            "payload_conflict",
            "This operation identity already has different content.",
        ));
    }
    Ok(Some(if saved["state"] == "finished" {
        saved["reply"].clone()
    } else {
        json!({"ok":false,"code":"outcome_unknown","message":"This operation was already dispatched. Inspect Git before requesting a new action; it was not sent again."})
    }))
}
impl Intent {
    /// Check a supplied key before legacy projections can fall back. No intent is created.
    pub(super) fn existing_locked(
        state: &Path,
        op: &str,
        project: &str,
        root: Option<&Path>,
        params: &Value,
        _lock: &Lock,
    ) -> Result<Option<Value>> {
        if params.get("requestId").is_none() {
            return Ok(None);
        }
        let (path, digest) = address(state, op, project, root, params)?;
        existing(&path, &digest)
    }
    pub fn begin(
        state: &Path,
        op: &str,
        project: &str,
        root: Option<&Path>,
        params: &Value,
    ) -> Result<Begin> {
        let lock = Lock::new(state, "machine.lock")?;
        Self::begin_locked(state, op, project, root, params, &lock)
    }
    pub(super) fn begin_locked(
        state: &Path,
        op: &str,
        project: &str,
        root: Option<&Path>,
        params: &Value,
        _lock: &Lock,
    ) -> Result<Begin> {
        let (path, digest) = address(state, op, project, root, params)?;
        if let Some(reply) = existing(&path, &digest)? {
            return Ok(Begin::Replay(reply));
        }
        let owner = crate::local::uid();
        let channel = &workspace_protocol::channel::current().name;
        let record = json!({"owner":owner,"channel":channel,"project":project,"target":root,"operation":op,"digest":digest,
            "state":"dispatched","dispatchedAt":now_ms()});
        settings::write_json(&path, &record)?;
        Ok(Begin::Run(Self { path, record }))
    }
    pub fn finish(self, state: &Path, reply: &Value) -> Result<()> {
        let lock = Lock::new(state, "machine.lock")?;
        self.finish_locked(reply, &lock)
    }
    pub(super) fn finish_locked(mut self, reply: &Value, _lock: &Lock) -> Result<()> {
        let uncertain = matches!(
            reply["code"].as_str(),
            Some(
                "git_timeout"
                    | "integration_unknown"
                    | "index_restore_failed"
                    | "machine_tool_timeout"
                    | "machine_tool_cancelled"
                    | "machine_tool_failed"
            )
        );
        self.record["state"] = json!(if uncertain { "unknown" } else { "finished" });
        self.record["recordedAt"] = json!(now_ms());
        self.record["reply"] = reply.clone();
        settings::write_json(&self.path, &self.record)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integration_unknown_replays_under_the_existing_lock() {
        let state = std::env::temp_dir().join(format!("cw-operation-{}", settings::unique()));
        let params = json!({"requestId":"squash","confirmed":true});
        let lock = Lock::new(&state, "machine.lock").unwrap();
        let Begin::Run(intent) =
            Intent::begin_locked(&state, "product.integrate", "p", None, &params, &lock).unwrap()
        else {
            panic!("unexpected replay")
        };
        intent
            .finish_locked(&json!({"ok":false,"code":"integration_unknown"}), &lock)
            .unwrap();
        let Begin::Replay(reply) =
            Intent::begin_locked(&state, "product.integrate", "p", None, &params, &lock).unwrap()
        else {
            panic!("repeated effect")
        };
        assert_eq!(reply["code"], "outcome_unknown");
        drop(lock);
        std::fs::remove_dir_all(state).unwrap();
    }
    #[test]
    fn unresolved_replay_and_conflicting_payloads_do_not_dispatch() {
        let state = std::env::temp_dir().join(format!("cw-operation-{}", settings::unique()));
        let p = json!({"action":"push","requestId":"one","remote":"credential-redacted","confirmed":true});
        assert!(matches!(
            Intent::begin(&state, "git", "p", None, &p).unwrap(),
            Begin::Run(_)
        ));
        let Begin::Replay(reply) = Intent::begin(&state, "git", "p", None, &p).unwrap() else {
            panic!("replayed")
        };
        assert_eq!(reply["code"], "outcome_unknown");
        assert_eq!(
            Intent::begin(&state, "git_init", "p", None, &p)
                .err()
                .unwrap()
                .code,
            "payload_conflict"
        );
        let data = std::fs::read_to_string(
            std::fs::read_dir(state.join("operations"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        assert!(!data.contains("credential-redacted"));
        std::fs::remove_dir_all(state).unwrap();
    }
}
