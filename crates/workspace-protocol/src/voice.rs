//! Closed local-owner dictation controls. Audio, paths and executable choices never cross this API.
use serde::Deserialize;
use serde_json::Value;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start { pub request_id: String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Session { pub session_id: String }
pub enum Request { Start(Start), Read(Session), Stop(Session), Cancel(Session) }
pub fn parse(method: &str, value: Value) -> Result<Request, String> {
    fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
        serde_json::from_value(value).map_err(|_| "invalid_voice_request".into())
    }
    let request = match method {
        "voice.start" => Request::Start(decode(value)?),
        "voice.read" => Request::Read(decode(value)?),
        "voice.stop" => Request::Stop(decode(value)?),
        "voice.cancel" => Request::Cancel(decode(value)?),
        _ => return Err("unsupported_voice_operation".into()),
    };
    let id = match &request { Request::Start(p) => &p.request_id, Request::Read(p) | Request::Stop(p) | Request::Cancel(p) => &p.session_id };
    crate::validate("Identity", &Value::String(id.clone()))?;
    Ok(request)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn controls_are_closed() {
        assert!(parse("voice.start", json!({"requestId":"voice-1"})).is_ok());
        for value in [json!({}), json!({"requestId":"x","command":"parec"}), json!({"requestId":"x","wav":"/private.wav"})] {
            assert!(parse("voice.start", value).is_err());
        }
        assert!(parse("voice.audio", json!({"sessionId":"x"})).is_err());
        assert!(parse("voice.stop", json!({"sessionId":"x","send":true})).is_err());
    }
}
