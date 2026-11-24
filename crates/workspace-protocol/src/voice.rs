//! Closed local-owner dictation controls. Audio, paths and executable choices never cross this API.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Ru,
    En,
}
impl Language {
    pub const ALL: [Self; 2] = [Self::Ru, Self::En];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetLanguage {
    pub language: Language,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start {
    pub request_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Session {
    pub session_id: String,
}
pub enum Request {
    Settings,
    SetLanguage(SetLanguage),
    Start(Start),
    Download(Start),
    Read(Session),
    Stop(Session),
    Cancel(Session),
}
pub fn parse(method: &str, value: Value) -> Result<Request, String> {
    fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
        serde_json::from_value(value).map_err(|_| "invalid_voice_request".into())
    }
    let request = match method {
        "voice.settings" => {
            decode::<Empty>(value)?;
            Request::Settings
        }
        "voice.setLanguage" => Request::SetLanguage(decode(value)?),
        "voice.start" => Request::Start(decode(value)?),
        "voice.download" => Request::Download(decode(value)?),
        "voice.read" => Request::Read(decode(value)?),
        "voice.stop" => Request::Stop(decode(value)?),
        "voice.cancel" => Request::Cancel(decode(value)?),
        _ => return Err("unsupported_voice_operation".into()),
    };
    let id = match &request {
        Request::Settings | Request::SetLanguage(_) => return Ok(request),
        Request::Start(p) | Request::Download(p) => &p.request_id,
        Request::Read(p) | Request::Stop(p) | Request::Cancel(p) => &p.session_id,
    };
    crate::validate("Identity", &Value::String(id.clone()))?;
    Ok(request)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn language_is_closed() {
        for language in Language::ALL {
            assert!(matches!(
                parse("voice.setLanguage", json!({"language":language.as_str()})),
                Ok(Request::SetLanguage(p)) if p.language == language
            ));
            assert_eq!(json!(language), json!(language.as_str()));
        }
        for params in [
            json!({}),
            json!({"language":null}),
            json!({"language":"EN"}),
            json!({"language":"fr"}),
            json!({"language":"en","url":"https://invalid"}),
        ] {
            assert!(parse("voice.setLanguage", params).is_err());
        }
        assert!(parse("voice.settings", json!({})).is_ok());
        assert!(parse("voice.settings", json!({"language":"en"})).is_err());
        assert!(parse("voice.download", json!({"requestId":"r"})).is_ok());
        assert!(parse("voice.download", json!({"requestId":"r","language":"en"})).is_err());
    }

    #[test]
    fn controls_are_closed() {
        assert!(parse("voice.start", json!({"requestId":"voice-1"})).is_ok());
        for value in [
            json!({}),
            json!({"requestId":"x","command":"parec"}),
            json!({"requestId":"x","wav":"/private.wav"}),
        ] {
            assert!(parse("voice.start", value).is_err());
        }
        assert!(parse("voice.audio", json!({"sessionId":"x"})).is_err());
        assert!(parse("voice.stop", json!({"sessionId":"x","send":true})).is_err());
    }
}
