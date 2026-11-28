//! Native channel authority. Only the runtime issues roles, attempts and secrets.
use crate::{
    error::refuse,
    lifecycle::domain,
    ownership::{self, OwnerEpoch},
    CoreError, CoreResult, TaskState,
};
use rusqlite::{Connection, OptionalExtension, Transaction};

#[derive(Clone)]
pub struct Credential {
    pub channel_id: String,
    secret: String,
}
impl Credential {
    pub fn secret(&self) -> &str {
        &self.secret
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub task: String,
    pub attempt: String,
    pub principal: String,
    pub epoch: i64,
    pub role: String,
    pub profile: Option<String>,
    pub stage: Option<String>,
    pub read_only: bool,
    pub assignment: Option<String>,
    pub session: Option<String>,
}
pub enum Principal<'a> {
    Coordinator,
    ActiveSteering,
    OwnerDecision { obstacle: &'a str },
    Worker { assignment: &'a str },
}
/// Worker assignments must already be claimed by the scheduler. Never replay a
/// launch with an existing provisioning intent, even if its process is unknown.
pub fn issue_tx(
    db: &Transaction<'_>,
    task: &str,
    owner: &OwnerEpoch,
    who: Principal<'_>,
) -> CoreResult<Credential> {
    ownership::check_tx(db, task, owner)?;
    let (state, revision) = domain::task_tx(db, task)?;
    let steering = matches!(who, Principal::ActiveSteering);
    let decision = match who {
        Principal::OwnerDecision { obstacle } => Some(obstacle),
        _ => None,
    };
    let (role, profile, assignment) = match who {
        Principal::Coordinator => {
            if !matches!(state, TaskState::Planning | TaskState::Reconciling) {
                return refuse("coordinator_not_expected");
            }
            ("coordinator", "orchestrator".to_owned(), None)
        }
        Principal::ActiveSteering => {
            if state != TaskState::Executing
                || crate::lifecycle::steering::pending(db, task)?.is_empty()
            {
                return refuse("coordinator_not_expected");
            }
            ("coordinator", "orchestrator".to_owned(), None)
        }
        Principal::OwnerDecision { obstacle } => {
            let trigger = crate::lifecycle::obstacles::decision_trigger(db, task, Some(obstacle))?;
            if trigger.is_none_or(|trigger| trigger.started) {
                return refuse("owner_decision_not_current");
            }
            ("coordinator", "orchestrator".to_owned(), None)
        }
        Principal::Worker { assignment } => {
            let row: Option<(String, String)> = db.query_row(
                "SELECT role,stage FROM runtime_nodes WHERE task_id=? AND revision=? AND assignment_ref=? AND status='running' AND result_json IS NULL",
                (task, revision, assignment), |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            let (profile, stage) = row.ok_or(CoreError::Refused("stale_assignment"))?;
            if !matches!(
                (state, stage.as_str()),
                (TaskState::Executing, "execution") | (TaskState::Reviewing, "review")
            ) {
                return refuse("stale_assignment");
            }
            ("worker", profile, Some(assignment))
        }
    };
    let existing: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND
        ((? IS NOT NULL AND idempotency_key=?) OR (?='coordinator' AND role_id='orchestrator'
        AND (EXISTS(SELECT 1 FROM capacity_reservations r WHERE r.attempt_id=delegation_provisioning.attempt_id AND r.state!='released')
        OR EXISTS(SELECT 1 FROM trusted_channels c WHERE c.channel_id=delegation_provisioning.channel_id AND c.revoked_at IS NULL)))))",
        (task, assignment, assignment, role), |r| r.get(0))?;
    if existing {
        return refuse("session_already_provisioned");
    }
    let channel_id = format!("ch_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let attempt = format!("xa_{}", uuid::Uuid::new_v4().simple());
    let principal = format!("native_{}", uuid::Uuid::new_v4().simple());
    let provisioning = assignment
        .map(str::to_owned)
        .unwrap_or_else(|| format!("native:{channel_id}"));
    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at)
        VALUES (?,?,?,?,?,?,?,?,?)",
        (&channel_id, crate::digest(secret.as_bytes()), task, &principal, role, assignment, &attempt, owner.epoch, crate::now()))?;
    db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,channel_id,owner_epoch,created_at,updated_at)
        VALUES (?,?,?,?,'starting','starting',?,?,?,?)",
        (provisioning, task, attempt, profile, &channel_id, owner.epoch, crate::now(), crate::now()))?;
    if steering {
        crate::lifecycle::event_tx(db, task, "NATIVE_ACTIVE_STEER_CHANNEL",
            serde_json::json!({"channel_id":channel_id,"revision":revision}))?;
    }
    if let Some(obstacle) = decision {
        crate::lifecycle::event_tx(db, task, "OWNER_DECISION_COORDINATOR_STARTED",
            serde_json::json!({"channel_id":channel_id,"obstacle":obstacle,"revision":revision}))?;
    }
    Ok(Credential { channel_id, secret })
}
fn equal(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |n, (a, b)| n | (a ^ b)) == 0
}
/// The non-ready check permits supervision during handshake, never MCP tools.
pub fn authenticate(
    db: &Connection,
    channel: &str,
    secret: &str,
    ready: bool,
) -> CoreResult<Identity> {
    let row: Option<(String, Identity, String, String, String)> = db.query_row(
        "SELECT c.secret_hash,c.task_id,c.attempt_id,c.principal_thread_id,c.owner_epoch,c.role,
        CASE WHEN c.role='worker' THEN p.role_id END,
        CASE WHEN c.role='coordinator' AND EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=c.task_id
                 AND e.type='NATIVE_ACTIVE_STEER_CHANNEL' AND json_extract(e.payload_json,'$.channel_id')=c.channel_id)
             THEN 'active_steering'
             WHEN c.role='coordinator' AND EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=c.task_id
                 AND e.type='OWNER_DECISION_COORDINATOR_STARTED' AND json_extract(e.payload_json,'$.channel_id')=c.channel_id)
             THEN 'owner_decision' ELSE n.stage END,
        CASE WHEN c.role='worker' THEN p.idempotency_key END,p.engine_session_ref,p.step,p.status,t.state,
        EXISTS(SELECT 1 FROM runtime_events e, json_each(e.payload_json,'$.plan.tasks') j
          WHERE e.id=(SELECT max(p.id) FROM runtime_events p WHERE p.task_id=n.task_id
            AND p.type IN ('PLAN_ACCEPTED','PLAN_AMENDED') AND json_extract(p.payload_json,'$.revision')=n.revision)
            AND json_extract(j.value,'$.id')=n.node_id AND json_extract(j.value,'$.read_only')=1)
        FROM trusted_channels c JOIN task_owners o ON o.task_id=c.task_id AND o.epoch=c.owner_epoch
        JOIN runtime_tasks t ON t.task_id=c.task_id
        JOIN delegation_provisioning p ON p.task_id=c.task_id AND p.channel_id=c.channel_id
            AND p.attempt_id=c.attempt_id AND p.owner_epoch=c.owner_epoch
        LEFT JOIN runtime_nodes n ON n.task_id=c.task_id AND n.assignment_ref=p.idempotency_key
        WHERE c.channel_id=? AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>?)
        AND NOT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=c.task_id)
        AND (c.role='coordinator' AND p.role_id='orchestrator' OR
            c.role='worker' AND n.status='running' AND n.result_json IS NULL AND n.revision=t.revision
            AND c.assignment_id=n.assignment_ref AND p.role_id=n.role)",
        (channel, crate::now()), |r| Ok((r.get(0)?, Identity {
            task:r.get(1)?,attempt:r.get(2)?,principal:r.get(3)?,epoch:r.get(4)?,role:r.get(5)?,
            profile:r.get(6)?,stage:r.get(7)?,assignment:r.get(8)?,session:r.get(9)?, read_only:r.get(13)?,
        },r.get(10)?,r.get(11)?,r.get(12)?))).optional()?;
    let (hash, identity, step, status, state) =
        row.ok_or(CoreError::Refused("channel_authority_refused"))?;
    if !equal(&hash, &crate::digest(secret.as_bytes())) || hash.len() != 64 {
        return refuse("channel_authority_refused");
    }
    let state: TaskState = state.parse().map_err(CoreError::Refused)?;
    let permitted = if matches!(
        identity.stage.as_deref(),
        Some(crate::lifecycle::steering::ACTIVE_STAGE | crate::lifecycle::obstacles::DECISION_STAGE)
    ) {
        state == TaskState::Executing
    } else if identity.role == "coordinator" {
        matches!(
            state,
            TaskState::Planning
                | TaskState::Reconciling
                | TaskState::WaitingUser
                | TaskState::PlanReady
                | TaskState::AnswerReady
        )
    } else {
        matches!(
            (state, identity.stage.as_deref()),
            (TaskState::Executing, Some("execution")) | (TaskState::Reviewing, Some("review"))
        )
    };
    if !permitted
        || !matches!(
            (step.as_str(), status.as_str()),
            ("starting", "starting") | ("ready", "ready")
        )
        || ready && (step != "ready" || identity.session.as_deref().is_none_or(str::is_empty))
    {
        return refuse("channel_authority_refused");
    }
    Ok(identity)
}
pub fn bind_session_tx(
    db: &Transaction<'_>,
    credential: &Credential,
    session: &str,
) -> CoreResult<()> {
    authenticate(db, &credential.channel_id, credential.secret(), false)?;
    if session.is_empty() || session.chars().count() > 512 {
        return refuse("invalid_session");
    }
    if db.execute("UPDATE delegation_provisioning SET engine_session_ref=?,step='ready',status='ready',updated_at=?
        WHERE channel_id=? AND engine_session_ref IS NULL AND step='starting' AND status='starting'",
        (session,crate::now(),&credential.channel_id))? != 1 { return refuse("session_already_bound"); }
    Ok(())
}
pub fn revoke_tx(db: &Transaction<'_>, channel: &str) -> CoreResult<()> {
    db.execute(
        "UPDATE trusted_channels SET revoked_at=? WHERE channel_id=? AND revoked_at IS NULL",
        (crate::now(), channel),
    )?;
    Ok(())
}
