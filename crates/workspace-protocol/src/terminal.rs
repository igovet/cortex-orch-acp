//! Closed local-owner PTY messages, shared by the native bridge and service.
use serde::Deserialize;
use serde_json::Value;

pub const OUTPUT_BYTES: usize = 65_536;
pub const READ_BYTES: usize = 16_384;
pub const INPUT_BYTES: usize = 4096;
pub const SESSION_LIMIT: usize = 8;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Open {
    pub project_id: String,
    pub cols: u16,
    pub rows: u16,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Session {
    pub project_id: String,
    pub session_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Read {
    pub project_id: String,
    pub session_id: String,
    pub after: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Write {
    pub project_id: String,
    pub session_id: String,
    pub data: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resize {
    pub project_id: String,
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
}
pub enum Request {
    Open(Open),
    Read(Read),
    Write(Write),
    Resize(Resize),
    Close(Session),
}
pub fn size(cols: u16, rows: u16) -> bool {
    (2..=500).contains(&cols) && (1..=500).contains(&rows)
}
fn identity(id: &str) -> Result<(), String> {
    crate::validate("Identity", &Value::String(id.into()))
}
pub fn parse(method: &str, message: Value) -> Result<Request, String> {
    fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, String> {
        serde_json::from_value(v).map_err(|_| "invalid_terminal_request".into())
    }
    let request = match method {
        "terminal.open" => Request::Open(decode(message)?),
        "terminal.read" => Request::Read(decode(message)?),
        "terminal.write" => Request::Write(decode(message)?),
        "terminal.resize" => Request::Resize(decode(message)?),
        "terminal.close" => Request::Close(decode(message)?),
        _ => return Err("unsupported_terminal_operation".into()),
    };
    let (project, session) = match &request {
        Request::Open(p) => {
            if !size(p.cols, p.rows) {
                return Err("invalid_terminal_size".into());
            }
            (&p.project_id, None)
        }
        Request::Read(p) => {
            if p.after > 9_007_199_254_740_991 {
                return Err("invalid_terminal_cursor".into());
            }
            (&p.project_id, Some(&p.session_id))
        }
        Request::Write(p) => {
            if p.data.is_empty() || p.data.len() > INPUT_BYTES {
                return Err("terminal_input_limit".into());
            }
            (&p.project_id, Some(&p.session_id))
        }
        Request::Resize(p) => {
            if !size(p.cols, p.rows) {
                return Err("invalid_terminal_size".into());
            }
            (&p.project_id, Some(&p.session_id))
        }
        Request::Close(p) => (&p.project_id, Some(&p.session_id)),
    };
    identity(project)?;
    if let Some(session) = session {
        identity(session)?;
    }
    Ok(request)
}
