//! Closed folder search on a machine. A client sends the text the owner typed, never a root or a limit.
use serde::Deserialize;
use serde_json::Value;

pub const QUERY_CHARS: usize = 4096;
pub const LIMIT_MOST: u32 = 100;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Search {
    pub session: String,
    pub query: String,
    #[serde(default)]
    pub limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cancel {
    pub session: String,
}
pub enum Request {
    Search(Search),
    Cancel(Cancel),
}
pub fn parse(method: &str, value: Value) -> Result<Request, String> {
    fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
        serde_json::from_value(value).map_err(|_| "invalid_directory_search".into())
    }
    let request = match method {
        "directories.search" => Request::Search(decode(value)?),
        "directories.cancel" => Request::Cancel(decode(value)?),
        _ => return Err("unsupported_directory_operation".into()),
    };
    let session = match &request {
        Request::Search(p) => {
            if p.query.chars().count() > QUERY_CHARS
                || p.query.contains('\0')
                || p.limit.is_some_and(|n| n == 0 || n > LIMIT_MOST)
            {
                return Err("invalid_directory_search".into());
            }
            &p.session
        }
        Request::Cancel(p) => &p.session,
    };
    crate::validate("Identity", &Value::String(session.clone()))?;
    if !session.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)) {
        return Err("invalid_directory_search".into());
    }
    Ok(request)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn requests_are_closed() {
        assert!(parse("directories.search", json!({"session":"pick-1","query":"~/code"})).is_ok());
        assert!(parse("directories.search", json!({"session":"pick-1","query":"","limit":30})).is_ok());
        assert!(parse("directories.cancel", json!({"session":"pick-1"})).is_ok());
        for value in [
            json!({"query":"x"}),
            json!({"session":"pick-1"}),
            json!({"session":"pick-1","query":"x","root":"/"}),
            json!({"session":"pick-1","query":"x","maxDepth":99}),
            json!({"session":"pick-1","query":"x","limit":0}),
            json!({"session":"pick-1","query":"x","limit":101}),
            json!({"session":"pick-1","query":"a\u{0}b"}),
            json!({"session":"pick-1","query":"x".repeat(QUERY_CHARS + 1)}),
            json!({"session":"","query":"x"}),
            json!({"session":"../x","query":"x"}),
        ] {
            assert!(parse("directories.search", value).is_err());
        }
        assert!(parse("directories.cancel", json!({"session":"pick-1","query":"x"})).is_err());
        assert!(parse("directories.read", json!({"session":"pick-1"})).is_err());
    }
}
