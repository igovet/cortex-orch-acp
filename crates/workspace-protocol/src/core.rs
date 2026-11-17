//! Native Core API v2. These DTOs carry no authority; the transport supplies it.
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

pub const NATIVE_API_MAJOR: u16 = 2;
pub const NATIVE_MAPPING: u16 = 2;

/// Canonical nonnegative SQLite i64, always a JSON string (never a JS Number).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(i64);
impl Version {
    pub fn new(value: i64) -> Result<Self, &'static str> {
        if value < 0 {
            Err("invalid_version")
        } else {
            Ok(Self(value))
        }
    }
    pub fn get(self) -> i64 {
        self.0
    }
}
impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s.is_empty()
            || !s.bytes().all(|b| b.is_ascii_digit())
            || (s.len() > 1 && s.starts_with('0'))
        {
            return Err(serde::de::Error::custom("invalid_version"));
        }
        s.parse::<i64>()
            .ok()
            .and_then(|n| Self::new(n).ok())
            .ok_or_else(|| serde::de::Error::custom("invalid_version"))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Protocol {
    pub major: u16,
    pub mapping: u16,
}
impl Default for Protocol {
    fn default() -> Self {
        Self {
            major: NATIVE_API_MAJOR,
            mapping: NATIVE_MAPPING,
        }
    }
}
impl Protocol {
    pub fn validate(&self) -> Result<(), &'static str> {
        if *self == Self::default() {
            Ok(())
        } else {
            Err("unsupported_protocol")
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskState {
    Created,
    Planning,
    AnswerReady,
    PlanReady,
    Executing,
    Reviewing,
    Verifying,
    Reconciling,
    WaitingUser,
    Recovering,
    Blocked,
    Paused,
    Failed,
    Cancelled,
    Completed,
}
impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Planning => "PLANNING",
            Self::AnswerReady => "ANSWER_READY",
            Self::PlanReady => "PLAN_READY",
            Self::Executing => "EXECUTING",
            Self::Reviewing => "REVIEWING",
            Self::Verifying => "VERIFYING",
            Self::Reconciling => "RECONCILING",
            Self::WaitingUser => "WAITING_USER",
            Self::Recovering => "RECOVERING",
            Self::Blocked => "BLOCKED",
            Self::Paused => "PAUSED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
            Self::Completed => "COMPLETED",
        }
    }
    pub fn terminal(self) -> bool {
        matches!(self, Self::Failed | Self::Cancelled | Self::Completed)
    }
}
impl std::str::FromStr for TaskState {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.into()))
            .map_err(|_| "unknown_task_state")
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentState {
    Planned,
    Running,
    Reported,
    Failed,
    NeedsRevision,
    CandidateReady,
}
impl AssignmentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Running => "running",
            Self::Reported => "reported",
            Self::Failed => "failed",
            Self::NeedsRevision => "needs_revision",
            Self::CandidateReady => "candidate_ready",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    pub target: String,
    pub cursor: Option<String>,
    pub limit: u16,
}
impl Page {
    pub fn validate(&self, max: u16) -> Result<(), &'static str> {
        if self.limit == 0 || self.limit > max {
            Err("invalid_page_limit")
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskText {
    pub task_id: String,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Guidance {
    pub worker_id: String,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Answer {
    pub question_id: String,
    pub answer: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionAnswers {
    pub answers: Vec<Answer>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    Accept,
    Reject,
    Integrate,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewRequest {
    pub execution_id: String,
    pub action: ReviewAction,
    pub candidate_digest: String,
    pub expected_base_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", content = "payload", deny_unknown_fields)]
pub enum CoreQuery {
    #[serde(rename = "project.status")]
    ProjectStatus,
    #[serde(rename = "project.snapshot")]
    ProjectSnapshot(Page),
    #[serde(rename = "tasks.page")]
    TasksPage(Page),
    #[serde(rename = "tasks.board")]
    TasksBoard(Target),
    #[serde(rename = "tasks.get")]
    TaskGet(Target),
    #[serde(rename = "tasks.state")]
    TaskState(Target),
    #[serde(rename = "tasks.options")]
    TaskOptions(Target),
    #[serde(rename = "executions.get")]
    ExecutionGet(Target),
    #[serde(rename = "decisions.list")]
    Decisions(Page),
    #[serde(rename = "questions.list")]
    Questions(Page),
    #[serde(rename = "permissions.list")]
    Permissions(Page),
    #[serde(rename = "results.get")]
    ResultGet(Target),
    #[serde(rename = "history.page")]
    History(Page),
    #[serde(rename = "reports.page")]
    Reports(Page),
    #[serde(rename = "requests.get")]
    RequestGet(Target),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", content = "payload", deny_unknown_fields)]
pub enum CoreCommand {
    #[serde(rename = "project.activate")]
    Activate,
    #[serde(rename = "tasks.create")]
    TaskCreate(TaskText),
    #[serde(rename = "tasks.edit")]
    TaskEdit(TaskText),
    #[serde(rename = "tasks.start")]
    TaskStart(Target),
    #[serde(rename = "executions.pause")]
    Pause(Target),
    #[serde(rename = "executions.resume")]
    Resume(Target),
    #[serde(rename = "executions.stop")]
    Stop(Target),
    #[serde(rename = "workers.guide")]
    WorkerGuidance(Guidance),
    #[serde(rename = "workers.stop")]
    WorkerStop(Target),
    #[serde(rename = "questions.resolve")]
    Answer(QuestionAnswers),
    #[serde(rename = "permissions.resolve")]
    Permission(PermissionResolution),
    #[serde(rename = "results.review")]
    Review(ReviewRequest),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionResolution {
    pub decision_id: String,
    pub allow: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestEnvelope<T> {
    pub protocol: Protocol,
    pub request_id: String,
    pub project_id: Option<String>,
    pub expected_version: Option<Version>,
    pub expected_owner_epoch: Option<Version>,
    #[serde(flatten)]
    pub body: T,
}
impl<T: NativeBody> RequestEnvelope<T> {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.protocol.validate()?;
        if self.request_id.is_empty()
            || self.request_id.chars().count() > 128
            || self.request_id.chars().any(char::is_control)
        {
            return Err("invalid_request_id");
        }
        if self.expected_owner_epoch.is_some_and(|v| v.get() == 0) {
            return Err("invalid_owner_epoch");
        }
        if let Some(ref id) = self.project_id {
            identity(id)?;
        }
        self.body.validate_body()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOutcome {
    Recorded,
    Pending,
    Unknown,
    Refused,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutationReceipt {
    pub request_id: String,
    pub command: String,
    pub target: String,
    pub outcome: MutationOutcome,
    pub entity_version: Version,
    pub owner_epoch: Version,
    pub event_cursor: Option<String>,
    pub recorded_at: String,
    pub code: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    StaleVersion,
    StaleOwner,
    Busy,
    Forbidden,
    NotFound,
    StorageUnavailable,
    UnsupportedProtocol,
    OutcomeUnknown,
    PayloadConflict,
    SnapshotExpired,
    ResyncRequired,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskSummary {
    pub id: String,
    pub version: Version,
    pub state: TaskState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotPage<T> {
    pub generation: String,
    pub project_id: String,
    pub snapshot_token: String,
    pub high_watermark: Version,
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}
pub fn identity(id: &str) -> Result<(), &'static str> {
    if id.is_empty() || id.chars().count() > 128 {
        Err("invalid_identity")
    } else {
        Ok(())
    }
}
fn text(value: &str) -> Result<(), &'static str> {
    if value.chars().count() > 32000 {
        Err("text_too_large")
    } else {
        Ok(())
    }
}
pub trait NativeBody {
    fn validate_body(&self) -> Result<(), &'static str>;
}
impl NativeBody for CoreCommand {
    fn validate_body(&self) -> Result<(), &'static str> {
        match self {
            Self::Activate => Ok(()),
            Self::TaskCreate(t) | Self::TaskEdit(t) => {
                identity(&t.task_id)?;
                text(&t.description)
            }
            Self::TaskStart(t)
            | Self::Pause(t)
            | Self::Resume(t)
            | Self::Stop(t)
            | Self::WorkerStop(t) => identity(&t.id),
            Self::WorkerGuidance(g) => {
                identity(&g.worker_id)?;
                text(&g.text)
            }
            Self::Answer(a) => {
                if a.answers.is_empty() {
                    return Err("empty_answers");
                }
                let mut ids = std::collections::BTreeSet::new();
                for answer in &a.answers {
                    identity(&answer.question_id)?;
                    text(&answer.answer)?;
                    if !ids.insert(&answer.question_id) {
                        return Err("duplicate_answer");
                    }
                }
                Ok(())
            }
            Self::Permission(p) => identity(&p.decision_id),
            Self::Review(r) => {
                identity(&r.execution_id)?;
                for digest in [&r.candidate_digest, &r.expected_base_digest] {
                    if digest.len() != 64
                        || !digest
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    {
                        return Err("invalid_digest");
                    }
                }
                Ok(())
            }
        }
    }
}
impl NativeBody for CoreQuery {
    fn validate_body(&self) -> Result<(), &'static str> {
        match self {
            Self::ProjectStatus => Ok(()),
            Self::ProjectSnapshot(p) | Self::TasksPage(p) => {
                identity(&p.target)?;
                p.validate(50)
            }
            Self::Decisions(p)
            | Self::Questions(p)
            | Self::Permissions(p)
            | Self::History(p)
            | Self::Reports(p) => {
                identity(&p.target)?;
                p.validate(100)
            }
            Self::TasksBoard(t)
            | Self::TaskGet(t)
            | Self::TaskState(t)
            | Self::TaskOptions(t)
            | Self::ExecutionGet(t)
            | Self::ResultGet(t)
            | Self::RequestGet(t) => identity(&t.id),
        }
    }
}
/// Transport entry: frame bound BEFORE parsing; serde rejects invalid UTF-8,
/// duplicate typed fields and unknown commands/fields. No authority is inferred.
pub fn decode_native_command(bytes: &[u8]) -> Result<RequestEnvelope<CoreCommand>, &'static str> {
    if bytes.len() > super::FRAME_BYTES {
        return Err("frame_too_large");
    }
    let request: RequestEnvelope<CoreCommand> =
        serde_json::from_slice(bytes).map_err(|_| "invalid_request")?;
    request.validate()?;
    Ok(request)
}
pub fn decode_native_query(bytes: &[u8]) -> Result<RequestEnvelope<CoreQuery>, &'static str> {
    if bytes.len() > super::FRAME_BYTES {
        return Err("frame_too_large");
    }
    let request: RequestEnvelope<CoreQuery> =
        serde_json::from_slice(bytes).map_err(|_| "invalid_request")?;
    request.validate()?;
    Ok(request)
}
