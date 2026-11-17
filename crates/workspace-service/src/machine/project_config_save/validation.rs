use super::*;
/// Every invalid setting of one request as `(field, code, message)`; the
/// management API reports all of them, the desktop protocol only the message.
#[derive(Default)]
pub(super) struct Errors(Vec<(String, &'static str, String)>);
impl Errors {
    pub fn add(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.coded(field, "invalid_value", message);
    }
    pub fn coded(
        &mut self,
        field: impl Into<String>,
        code: &'static str,
        message: impl Into<String>,
    ) {
        self.0.push((field.into(), code, message.into()));
    }
    pub fn finish(self) -> Result<()> {
        if self.0.is_empty() {
            return Ok(());
        }
        let shown = self
            .0
            .iter()
            .take(5)
            .map(|(f, _, m)| format!("{f}: {m}"))
            .collect::<Vec<_>>()
            .join("; ");
        let more = if self.0.len() > 5 {
            format!(" (and {} more)", self.0.len() - 5)
        } else {
            String::new()
        };
        let errors: Vec<Value> = self
            .0
            .iter()
            .map(|(field, code, message)| json!({"field":field,"code":code,"message":message}))
            .collect();
        Err(refuse(
            "invalid_configuration",
            format!("Correct the invalid setting(s); nothing was written. {shown}{more}"),
        )
        .with(json!({"field":self.0[0].0,"errors":errors})))
    }
}
pub(super) fn truth(v: &Value) -> bool {
    super::super::workers::truthy(v)
}
pub(super) fn unique(v: &Value, valid: impl Fn(&Value) -> bool) -> bool {
    v.as_array().is_some_and(|a| {
        a.iter().all(valid)
            && a.iter()
                .map(Value::to_string)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == a.len()
    })
}
pub(super) fn model(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        (1..=256).contains(&s.chars().count())
            && !s
                .chars()
                .any(|c| c.is_whitespace() || c < ' ' || c == '\u{7f}')
    })
}
pub(super) fn exact(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.contains('/') && !s.starts_with('/') && !s.ends_with('/'))
}
pub(super) fn repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if c < ' ' || c == '\u{7f}' => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
pub(super) fn object_error() -> Refused {
    refuse("machine_tool_failed", "AttributeError")
}
