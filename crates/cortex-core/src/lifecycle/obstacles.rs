//! Why an attempt failed, and what the runtime does about it (no-stop policy).
//!
//! Every failed node, dead end and runtime stop is classified from recorded
//! evidence into one class ([`classify`]):
//!
//! - `access`: a login, credential or permission is missing (GitHub CLI not logged
//!   in, an API key missing, HTTP 401/403, a cancelled `sudo` password). The runtime
//!   records one open `OWNER_ACTION_REQUIRED` per task and service and parks the
//!   blocked work; the rest of the plan keeps running. When the action is resolved
//!   (`OWNER_ACTION_RESOLVED`, or an environment preflight recheck that shows the
//!   access is now present) the parked work is retried at once.
//! - `owner_decision`: the requirements conflict or the decision is the owner's.
//!   The coordinator must ask the owner one question (`OWNER_DECISION_REQUIRED`,
//!   continuation strategy `ask_owner`); no rung of the strategy ladder is used.
//! - `external_outage`: a built-in public endpoint is down or unreachable (DNS,
//!   refused or timed-out connection, 5xx, 429). The runtime waits without model calls and
//!   rechecks the host itself with backoff ([`BACKOFF`]; a rate limit from a host it
//!   may not probe, or a checkout another actor keeps changing, is waited for by time
//!   alone, [`TIMER`]); `EXTERNAL_WAIT_STARTED`,
//!   `EXTERNAL_WAIT_RECHECKED` and `EXTERNAL_WAIT_ENDED` record it, and the parked
//!   work resumes once a recheck succeeds.
//! - `technical`: everything else, which continues on the strategy ladder.
//!
//! An obstacle the owner already settled is never raised again: an answered owner
//! decision and missing access whose action was resolved or dismissed
//! [`ACCESS_REPEAT_LIMIT`] times become technical failures with a disclosed limit
//! (`LOOP_BROKEN`).
//!
//! `FAILURE_CLASSIFIED` records each classification. `TASK_PROGRESS` marks real
//! progress ([`progress_for`]). Events are additive in format 12.
use super::{domain, event_tx};
use crate::{error::refuse, CoreResult, TaskState};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub const ACCESS: &str = "access";
pub const OWNER_DECISION: &str = "owner_decision";
pub const EXTERNAL_OUTAGE: &str = "external_outage";
pub const TECHNICAL: &str = "technical";
/// The stop reason of a plan whose only remaining work waits for an owner decision.
pub const OWNER_DECISION_STOP: &str = "owner_decision_required";
/// Recheck delays of an external wait, in seconds: 1, 2, 5, 10 and then 30 minutes,
/// repeated at 30 minutes for as long as the outage lasts.
pub const BACKOFF: [i64; 5] = [60, 120, 300, 600, 1800];
/// A node whose host answered again but which failed on it again this many times is
/// no longer treated as an outage: the work itself fails (strategy ladder).
const OUTAGE_REPEAT_LIMIT: i64 = 3;
/// The probe kind of a wait the runtime ends by time alone: a destination it may not
/// contact (an unlisted host that rate limits, a checkout another actor keeps
/// changing). Its recheck ends the wait once the backoff elapsed
/// (`EXTERNAL_WAIT_ENDED` reason `backoff_elapsed`) and the work is retried.
pub const TIMER: &str = "timer";
/// After this many owner actions for the same service were resolved or dismissed and
/// the work still fails for the same missing access, no further action is opened: the
/// failure takes the technical route and the missing access is a disclosed limit.
pub const ACCESS_REPEAT_LIMIT: i64 = 2;
const DETAIL_CHARS: usize = 300;
const OUTPUT_TAIL: usize = 4_000;

/// The recheck delay after `attempt` failed rechecks (0 for the first).
pub fn backoff(attempt: usize) -> i64 {
    BACKOFF[attempt.min(BACKOFF.len() - 1)]
}

/// One failed command of the attempt: its argv and the (bounded) output it printed.
#[derive(Debug, Default, Clone)]
pub struct Output {
    pub argv: Vec<String>,
    pub text: String,
}

/// The recorded evidence of one failed attempt, dead end or stop.
#[derive(Debug, Default, Clone)]
pub struct Signals {
    /// The worker's dead-end category (`blocker.kind`, a review item's `blocker`).
    pub blocker_kind: Option<String>,
    /// The worker's own words about the dead end (`blocker.detail`, a review reason).
    pub blocker_detail: Option<String>,
    /// Further recorded text: the result summary, error and refusal codes.
    pub texts: Vec<String>,
    /// Failed command receipts of the attempt (nonzero exit), newest first.
    pub outputs: Vec<Output>,
    /// Whether the attempt actually tried something that was refused or failed: a
    /// failed command or a refused permission or outside request on its channels.
    /// `None` when unknown (review items); `Some(false)` makes a `missing_access`
    /// dead end without any refusal a technical failure the next attempt corrects.
    pub attempted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub class: &'static str,
    /// Which rule decided it (stable, for tests and diagnostics).
    pub rule: &'static str,
    /// `access`: the service the owner must fix (`github`, `codex`, `claude`, `sudo`,
    /// a host or a tool name).
    pub service: Option<String>,
    /// `external_outage`: the remote host and port the runtime rechecks.
    pub host: Option<String>,
    pub port: Option<u16>,
    pub scheme: Option<String>,
    /// `tcp` (connect) or `http` (a HEAD request that must not answer 5xx/429).
    pub probe: Option<&'static str>,
    /// A short plain-English statement of what and why.
    pub detail: String,
}

impl Classification {
    fn new(class: &'static str, rule: &'static str, detail: String) -> Self {
        Self {
            class,
            rule,
            service: None,
            host: None,
            port: None,
            scheme: None,
            probe: None,
            detail,
        }
    }
    pub fn to_json(&self) -> Value {
        let mut v = json!({"class":self.class,"rule":self.rule,"detail":self.detail});
        for (key, value) in [
            ("service", self.service.as_deref()),
            ("host", self.host.as_deref()),
            ("scheme", self.scheme.as_deref()),
            ("probe", self.probe),
        ] {
            if let Some(value) = value {
                v[key] = json!(value);
            }
        }
        if let Some(port) = self.port {
            v["port"] = json!(port);
        }
        v
    }
}

fn clip(text: &str, chars: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= chars {
        return text;
    }
    let mut out: String = text.chars().take(chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn tail(text: &str, chars: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(chars)).collect()
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| text.contains(n))
}

/// An HTTP status named in a recognisable context: `HTTP 401`, `HTTP/1.1 503`,
/// `status 429`, `status code: 502`, `error 403`, `returned 500`, `(401)`, or the
/// code followed by its reason phrase.
fn has_status(text: &str, matches: impl Fn(u16) -> bool) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let digits = &bytes[i..i + 3];
        let bounded = (i == 0 || !bytes[i - 1].is_ascii_digit())
            && bytes.get(i + 3).is_none_or(|b| !b.is_ascii_digit());
        if bounded && digits.iter().all(u8::is_ascii_digit) {
            let code: u16 = std::str::from_utf8(digits).unwrap().parse().unwrap();
            if matches(code) {
                let before = text[..i].trim_end_matches([' ', ':', '=', '(', '"', '\'', '[']);
                let after = text[i + 3..].trim_start_matches([' ', ':', ')', '"', '\'', ']']);
                let context = [
                    "http",
                    "http/1.1",
                    "http/1.0",
                    "http/2",
                    "http/3",
                    "status",
                    "status code",
                    "code",
                    "error",
                    "returned",
                    "response",
                    "statuscode",
                    "status_code",
                ];
                let phrase = [
                    "unauthorized",
                    "forbidden",
                    "too many requests",
                    "internal server error",
                    "bad gateway",
                    "service unavailable",
                    "gateway timeout",
                    "service temporarily unavailable",
                ];
                if context.iter().any(|c| before.ends_with(c))
                    || phrase.iter().any(|p| after.starts_with(p))
                    || text[..i].ends_with('(') && text[i + 3..].starts_with(')')
                {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

const ACCESS_PHRASES: [&str; 40] = [
    "gh auth login",
    "not logged in",
    "not logged into",
    "you are not logged",
    "login required",
    "please log in",
    "please login",
    "authentication required",
    "authentication failed",
    "requires authentication",
    "must authenticate",
    "unauthorized",
    "bad credentials",
    "invalid credentials",
    "missing credentials",
    "no credentials",
    "credentials not found",
    "could not find credentials",
    "could not read username",
    "terminal prompts disabled",
    "permission denied (publickey",
    "invalid api key",
    "incorrect api key",
    "missing api key",
    "no api key",
    "api key is missing",
    "api key not",
    "api_key is not set",
    "api_key not set",
    "token expired",
    "token has expired",
    "invalid token",
    "expired token",
    "sudo: a password is required",
    "sudo: a terminal is required",
    "sudo: no tty present",
    "sudo: no password was provided",
    "incorrect password attempt",
    "password prompt was cancelled",
    "sudo password was cancelled",
];
const RATE_LIMIT_PHRASES: [&str; 4] = [
    "rate limit",
    "too many requests",
    "secondary rate limit",
    "quota exceeded",
];
const SERVER_ERROR_PHRASES: [&str; 6] = [
    "internal server error",
    "bad gateway",
    "service unavailable",
    "service temporarily unavailable",
    "gateway timeout",
    "upstream connect error",
];
const NETWORK_PHRASES: [&str; 26] = [
    "could not resolve host",
    "couldn't resolve host",
    "could not resolve hostname",
    "temporary failure in name resolution",
    "name or service not known",
    "nodename nor servname",
    "no such host",
    "getaddrinfo",
    "enotfound",
    "eai_again",
    "dns error",
    "failed to lookup address",
    "connection refused",
    "econnrefused",
    "connection timed out",
    "etimedout",
    "operation timed out",
    "network is unreachable",
    "no route to host",
    "connection reset",
    "econnreset",
    "tls handshake timeout",
    "i/o timeout",
    "failed to connect to",
    "could not connect to",
    "unable to connect to",
];
const DECISION_PHRASES: [&str; 12] = [
    "contradict",
    "conflicting requirement",
    "requirements conflict",
    "conflict with each other",
    "mutually exclusive",
    "incompatible requirement",
    "only the owner can",
    "owner must decide",
    "owner has to decide",
    "needs the owner's decision",
    "needs an owner decision",
    "requires an owner decision",
];

/// A rate limit named in lowercased failure text: a rate-limit phrase or an HTTP 429.
/// A full disk ("disk quota exceeded") is not a rate limit.
fn rate_limit_evidence(text: &str) -> bool {
    let text = text.replace("disk quota exceeded", "");
    contains_any(&text, &RATE_LIMIT_PHRASES) || has_status(&text, |c| c == 429)
}

/// The host a rate limit names: a URL or connection host, else a DNS name after
/// `from`, `by` or `at` (`429 from mirror.gcr.io`); `rate-limited-service` when none.
fn rate_limited_host(text: &str) -> String {
    let file = |h: &str| {
        h.rsplit('.').next().is_some_and(|ext| {
            [
                "json", "yml", "yaml", "toml", "md", "rs", "ts", "tsx", "js", "mjs", "txt", "lock",
                "sh", "py", "log", "xml", "html", "css", "png", "svg", "jpg",
            ]
            .contains(&ext)
        })
    };
    let ok = |h: &str| {
        !is_local(h)
            && access_service_ok(h)
            && !file(h)
            && h.rsplit('.')
                .next()
                .is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_lowercase()))
    };
    let mut candidates: Vec<String> = hosts(text).into_iter().map(|(h, _, _)| h).collect();
    for marker in [" from ", " by ", " at "] {
        candidates.extend(host_after(text, marker).into_iter().map(|(h, _)| h));
    }
    candidates
        .into_iter()
        .find(|h| ok(h))
        .unwrap_or_else(|| "rate-limited-service".into())
}

fn is_local(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    host == "localhost"
        || host.ends_with(".localhost")
        || host.starts_with("127.")
        || host == "::1"
        || host == "0.0.0.0"
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.starts_with("172.")
            && host
                .split('.')
                .nth(1)
                .and_then(|o| o.parse::<u8>().ok())
                .is_some_and(|o| (16..=31).contains(&o))
}

fn host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_'
}

/// The first host named after `marker` in `text`, with its port when written as
/// `host:port` or `host port N`.
fn host_after(text: &str, marker: &str) -> Vec<(String, Option<u16>)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(marker) {
        let after = &rest[at + marker.len()..];
        let after = after.trim_start_matches([' ', '\'', '"', '`', '<', '(']);
        // `user@host` in a URL: the host follows the last `@` of the authority.
        let authority: String = after
            .chars()
            .take_while(|c| host_char(*c) || *c == '@' || *c == ':' || *c == '[' || *c == ']')
            .collect();
        let host_port = authority.rsplit('@').next().unwrap_or("");
        let (host, port) = match host_port.split_once(':') {
            // Keep an invalid explicit port distinct from an omitted port.
            Some((h, p)) => (h, Some(p.parse().unwrap_or(0))),
            None => (host_port, None),
        };
        let host = host.trim_end_matches('.').to_owned();
        let port = port.or_else(|| {
            let after = &after[authority.len()..];
            let after = after.trim_start_matches([' ', ',', '\'', '"', ')']);
            after.strip_prefix("port ").map(|p| {
                p.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
        });
        if host.contains('.') || host == "localhost" {
            out.push((host, port));
        }
        rest = &rest[at + marker.len()..];
    }
    out
}

/// Hosts named in failure text, with port and scheme where known.
fn hosts(text: &str) -> Vec<(String, Option<u16>, Option<&'static str>)> {
    let mut out = Vec::new();
    for (scheme, marker) in [("https", "https://"), ("http", "http://")] {
        for (h, p) in host_after(text, marker) {
            out.push((h, p, Some(scheme)));
        }
    }
    for marker in [
        "could not resolve host:",
        "couldn't resolve host:",
        "could not resolve host",
        "could not resolve hostname",
        "failed to connect to",
        "could not connect to",
        "unable to connect to",
        "connect to",
        "dial tcp",
        "lookup",
        "enotfound",
        "eai_again",
        "host:",
    ] {
        for (h, p) in host_after(text, marker) {
            out.push((h, p, None));
        }
    }
    out
}

/// Built-in destinations for unauthenticated outage checks: public GitHub and
/// the default OpenAI/Codex and Anthropic endpoints. This is not a permission
/// to probe a Git remote, a configured custom URL or a host named by a command.
/// Return a static hostname so text/argv can only select an existing entry.
pub fn external_probe_host(host: &str, scheme: &str, port: u16) -> Option<&'static str> {
    if scheme != "https" || port != 443 {
        return None;
    }
    [
        "api.github.com",
        "github.com",
        "api.openai.com",
        "chatgpt.com",
        "api.anthropic.com",
    ]
    .into_iter()
    .find(|&known| known == host)
}

/// Service keys are short tokens or DNS names, never URLs or command arguments.
pub fn access_service_ok(service: &str) -> bool {
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if service.contains('.') {
        service.len() <= 253
            && service.split('.').all(|label| {
                let bytes = label.as_bytes();
                bytes.len() <= 63
                    && bytes.first().copied().is_some_and(alnum)
                    && bytes.last().copied().is_some_and(alnum)
                    && bytes.iter().all(|&b| alnum(b) || b == b'-')
            })
    } else {
        !service.is_empty()
            && service.len() <= 40
            && service.bytes().all(|b| alnum(b) || b == b'-' || b == b'_')
    }
}

fn command_name(arg: &str) -> &str {
    arg.rsplit('/').next().unwrap_or(arg)
}

/// Only commands known to contact a service or request credentials. Do not
/// unwrap shell commands or treat package scripts as network evidence.
fn access_command(argv: &[String]) -> bool {
    let Some((tool, args)) = argv.split_first() else {
        return false;
    };
    let subcommand = args.first().map(String::as_str);
    match command_name(tool) {
        "gh" | "curl" | "wget" | "ssh" | "scp" | "sftp" | "sudo" => true,
        "git" => matches!(
            subcommand,
            Some("clone" | "fetch" | "ls-remote" | "pull" | "push" | "submodule")
        ),
        "npm" => matches!(
            subcommand,
            Some("add" | "ci" | "install" | "login" | "ping" | "publish" | "view" | "whoami")
        ),
        "cargo" => matches!(subcommand, Some("fetch" | "login" | "publish" | "search")),
        "docker" | "podman" => matches!(subcommand, Some("login" | "pull" | "push")),
        "codex" | "claude" => matches!(subcommand, Some("login" | "auth")),
        _ => false,
    }
}

/// The service an access failure is about: the failing tool first, then names in
/// the text, then a named host, then the tool itself.
fn service(text: &str, outputs: &[Output]) -> String {
    let tool = outputs
        .iter()
        .filter_map(|o| o.argv.first())
        .map(|a| command_name(a).to_lowercase())
        .next();
    let named = |t: &str| -> Option<&'static str> {
        if t == "gh" || t.contains("github") {
            Some("github")
        } else if t.contains("codex") || t.contains("openai") {
            Some("codex")
        } else if t.contains("claude") || t.contains("anthropic") {
            Some("claude")
        } else if t == "sudo"
            || t.starts_with("sudo:")
            || t.contains("sudo ")
            || t.contains(" sudo")
        {
            Some("sudo")
        } else {
            None
        }
    };
    if let Some(found) = tool.as_deref().and_then(named) {
        return found.into();
    }
    if text.contains("gh auth") {
        return "github".into();
    }
    if let Some(found) = named(text) {
        return found.into();
    }
    if let Some((host, _, _)) = hosts(text)
        .into_iter()
        .find(|(h, _, _)| !is_local(h) && access_service_ok(h))
    {
        return host;
    }
    tool.filter(|t| access_service_ok(t))
        .unwrap_or_else(|| "unknown".into())
}

/// Classify one failed attempt, dead end or stop from its recorded evidence. The
/// rules, in order:
/// 1. a dead end `owner_decision` → `owner_decision`; `other_role_work` → `technical`;
/// 2. the worker's own dead-end words name conflicting requirements or a decision only
///    the owner can make → `owner_decision`;
/// 3. a rate limit (429, "rate limit") against a known endpoint → `external_outage` (http);
///    against any other host, named in the worker's words, the recorded texts or a
///    service command's output → `external_outage` with a [`TIMER`] wait, before access;
/// 4. a `missing_access` dead end or a failed access-related command reporting a
///    missing login, credential or permission (see [`ACCESS_PHRASES`], HTTP 401/403)
///    → `access`;
/// 5. a 5xx answer or a network failure (DNS, refused, timed out, unreachable) naming
///    a known endpoint → `external_outage` (http for 5xx, otherwise tcp); local,
///    unsupported or ambiguous destinations stay `technical`;
/// 6. everything else → `technical`.
pub fn classify(s: &Signals) -> Classification {
    let kind = s.blocker_kind.as_deref().unwrap_or("");
    let detail = s.blocker_detail.as_deref().unwrap_or("").to_lowercase();
    let mut corpus = detail.clone();
    for text in &s.texts {
        corpus.push('\n');
        corpus.push_str(&text.to_lowercase());
    }
    for output in &s.outputs {
        corpus.push('\n');
        corpus.push_str(&output.argv.join(" ").to_lowercase());
        corpus.push('\n');
        corpus.push_str(&tail(&output.text, OUTPUT_TAIL).to_lowercase());
    }
    let summary = |what: &str| -> String {
        let own = s.blocker_detail.as_deref().filter(|d| !d.trim().is_empty());
        clip(
            &match own {
                Some(own) => format!("{what}: {own}"),
                None => what.to_owned(),
            },
            DETAIL_CHARS,
        )
    };
    if kind == OWNER_DECISION {
        return Classification::new(
            OWNER_DECISION,
            "blocker_owner_decision",
            summary("A decision only the owner can make blocks this work"),
        );
    }
    if kind == "other_role_work" {
        return Classification::new(
            TECHNICAL,
            "blocker_other_role_work",
            summary("Another role must do work first"),
        );
    }
    if !kind.is_empty() && contains_any(&detail, &DECISION_PHRASES) {
        return Classification::new(
            OWNER_DECISION,
            "detail_conflicting_requirements",
            summary("The requirements conflict and need the owner's decision"),
        );
    }
    let candidates = hosts(&corpus);
    let local = candidates.iter().any(|(h, _, _)| is_local(h));
    let mut named = candidates.iter().filter(|(h, _, _)| !is_local(h));
    let first = named.next();
    let known_host = |(host, port, scheme): &(String, Option<u16>, Option<&str>)| {
        let scheme = scheme.unwrap_or("https");
        external_probe_host(
            host,
            scheme,
            port.unwrap_or(if scheme == "http" { 80 } else { 443 }),
        )
    };
    // A mention is not authority. Do not turn an unrelated URL in the same
    // evidence (or an explicit alternative port/scheme) into a readiness signal.
    let remote = first
        .and_then(known_host)
        .filter(|host| !local && named.all(|candidate| known_host(candidate) == Some(*host)));
    let outage = |host: &str, rule: &'static str, probe: &'static str, what: &str| {
        let mut c = Classification::new(
            EXTERNAL_OUTAGE,
            rule,
            clip(
                &format!(
                    "{host} {what}; the runtime rechecks it and resumes the work when it answers"
                ),
                DETAIL_CHARS,
            ),
        );
        c.port = Some(443);
        c.scheme = Some("https".into());
        c.host = Some(host.to_owned());
        c.probe = Some(probe);
        c
    };
    let rate_limited = rate_limit_evidence(&corpus);
    if let Some(host) = remote.filter(|_| rate_limited) {
        return outage(host, "rate_limited", "http", "is rate limiting requests");
    }
    // A rate limit is an outage of the service, whatever host answered it, and it
    // beats a missing-access reading of the same evidence (owner store 2026-11-21: a
    // registry mirror's 429 on an image pull was routed to the owner as missing
    // access). The worker's own words, the recorded texts and the output of commands
    // that contact a service count; a failing test's output does not. A host the
    // runtime may not probe is waited for by time alone (`probe: timer`): the wait
    // ends after its backoff and the work is retried, never contacting the host.
    let mut contact = detail.clone();
    for text in &s.texts {
        contact.push('\n');
        contact.push_str(&text.to_lowercase());
    }
    for output in s.outputs.iter().filter(|o| access_command(&o.argv)) {
        contact.push('\n');
        contact.push_str(&tail(&output.text, OUTPUT_TAIL).to_lowercase());
    }
    if rate_limit_evidence(&contact) {
        let host = rate_limited_host(&contact);
        let mut c = Classification::new(
            EXTERNAL_OUTAGE,
            "rate_limited_timer",
            clip(
                &format!("{host} is rate limiting requests; the runtime waits with backoff and retries the work"),
                DETAIL_CHARS,
            ),
        );
        c.host = Some(host);
        c.probe = Some(TIMER);
        return c;
    }
    let access = s
        .outputs
        .iter()
        .filter(|o| access_command(&o.argv))
        .find_map(|output| {
            let text = tail(&output.text, OUTPUT_TAIL).to_lowercase();
            let rule = contains_any(&text, &ACCESS_PHRASES)
                .then_some("access_phrase")
                .or_else(|| {
                    has_status(&text, |c| c == 401 || c == 403).then_some("http_401_403")
                })?;
            let evidence = format!("{}\n{text}", output.argv.join(" ").to_lowercase());
            Some((rule, service(&evidence, std::slice::from_ref(output))))
        })
        .or_else(|| {
            (kind == "missing_access").then(|| ("blocker_missing_access", service(&detail, &[])))
        });
    // A worker that only read its tool descriptions and declared access missing
    // never met a refusal: that is not an owner obstacle. The next attempt is told
    // to try the action and report the exact refusal (owner store 2026-11-23: a
    // render was parked on "no route" without one command being run).
    if matches!(access, Some(("blocker_missing_access", _)))
        && s.attempted == Some(false)
        && !contains_any(&detail, &ACCESS_PHRASES)
        && !has_status(&detail, |c| c == 401 || c == 403)
    {
        return Classification::new(
            TECHNICAL,
            "blocker_missing_access_unattempted",
            summary("Access was reported missing, but nothing was tried and refused. Run the needed command or tool first (command_run, or outside_command for the host) and report missing access only with the exact refusal or error it returned"),
        );
    }
    if let Some((rule, service)) = access {
        let why = match service.as_str() {
            "github" => {
                "GitHub access is missing (GitHub CLI not logged in or the token lacks permission)"
                    .to_owned()
            }
            "codex" => "Codex/OpenAI access is missing (not logged in or no API key)".to_owned(),
            "claude" => {
                "Claude/Anthropic access is missing (not logged in or no API key)".to_owned()
            }
            "sudo" => "Administrator (sudo) access was not granted".to_owned(),
            other => format!("Access to {other} is missing (login, credential or permission)"),
        };
        let mut c = Classification::new(ACCESS, rule, summary(&why));
        c.service = Some(service);
        return c;
    }
    let server_error = contains_any(&corpus, &SERVER_ERROR_PHRASES)
        || has_status(&corpus, |c| (500..600).contains(&c));
    let network = contains_any(&corpus, &NETWORK_PHRASES);
    if let Some(host) = remote.filter(|_| server_error || network) {
        return if server_error {
            outage(host, "server_error", "http", "answers with a server error")
        } else {
            outage(host, "network_unreachable", "tcp", "cannot be reached")
        };
    }
    if (rate_limited || server_error || network) && first.is_some() {
        return Classification::new(
            TECHNICAL,
            "outage_probe_unsupported",
            summary("The runtime cannot automatically recheck this destination"),
        );
    }
    if (server_error || network) && local {
        return Classification::new(
            TECHNICAL,
            "local_service_failure",
            summary("A local service of the project failed"),
        );
    }
    Classification::new(TECHNICAL, "technical", summary("The work failed"))
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}

/// The evidence of one failed node attempt: its result's blocker, summary and error,
/// and its failed command receipts (newest first, at most six).
pub fn node_signals(db: &Connection, task: &str, assignment: &str) -> CoreResult<Signals> {
    let raw: Option<String> = db
        .query_row(
            "SELECT result_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let result: Value = raw
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let mut s = Signals::default();
    match &result["blocker"] {
        Value::String(kind) => s.blocker_kind = Some(kind.clone()),
        blocker => {
            s.blocker_kind = text(&blocker["kind"]);
            s.blocker_detail = text(&blocker["detail"]);
        }
    }
    for key in ["summary", "error", "reason"] {
        s.texts.extend(text(&result[key]));
    }
    // An attempt that ended without a result records its error on the attempt.
    let failure: Option<String> = db
        .query_row(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=?
             AND event_type='assignment_failed' AND superseded=0 ORDER BY id DESC LIMIT 1",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?;
    if let Some(failure) = failure.and_then(|f| serde_json::from_str::<Value>(&f).ok()) {
        s.texts.extend(text(&failure["error"]));
    }
    let receipts = db
        .prepare(
            "SELECT r.result_json FROM runtime_requests r JOIN trusted_channels c ON c.channel_id=r.actor
             JOIN delegation_provisioning p ON p.attempt_id=c.attempt_id AND p.task_id=r.task_id
             WHERE r.task_id=? AND p.idempotency_key=? AND r.operation IN ('command_run','outside_command')
             ORDER BY r.rowid DESC LIMIT 32",
        )?
        .query_map((task, assignment), |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for raw in receipts {
        let Ok(receipt) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if receipt["exit_code"].as_i64().unwrap_or(-1) == 0 {
            continue;
        }
        let argv = receipt["argv"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str().map(str::to_owned))
            .collect();
        let mut out = receipt["output"].as_str().unwrap_or("").to_owned();
        if let Some(failure) = receipt["failure"].as_str() {
            out.push('\n');
            out.push_str(failure);
        }
        s.outputs.push(Output { argv, text: out });
        if s.outputs.len() >= 6 {
            break;
        }
    }
    // A refusal the runtime recorded for the attempt's channels counts as a try.
    let refused: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events e JOIN trusted_channels c
           ON c.channel_id=json_extract(e.payload_json,'$.channel_id')
         JOIN delegation_provisioning p ON p.attempt_id=c.attempt_id AND p.task_id=e.task_id
         WHERE e.task_id=? AND p.idempotency_key=? AND (
           (e.type='NATIVE_OUTSIDE_SETTLED' AND json_extract(e.payload_json,'$.allowed') IS NOT 1)
           OR (e.type='NATIVE_PERMISSION_DECIDED' AND coalesce(json_extract(e.payload_json,'$.outcome.optionId'),'') NOT LIKE 'allow%')
           OR e.type IN ('NATIVE_COMMAND_REFUSED','EXTERNAL_TOOL_REFUSED')))",
        (task, assignment),
        |r| r.get(0),
    )?;
    s.attempted = Some(!s.outputs.is_empty() || refused);
    Ok(s)
}

/// Open records of `start` without a later `end` naming the same `id`, oldest first.
fn open(db: &Connection, task: &str, start: &str, end: &str) -> CoreResult<Vec<(i64, Value)>> {
    let rows = db
        .prepare(
            "SELECT s.id,s.payload_json FROM runtime_events s WHERE s.task_id=?1 AND s.type=?2
             AND NOT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=?1 AND e.type=?3 AND e.id>s.id
                 AND json_extract(e.payload_json,'$.id')=json_extract(s.payload_json,'$.id'))
             ORDER BY s.id",
        )?
        .query_map((task, start, end), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, raw)| Ok((id, serde_json::from_str(&raw)?)))
        .collect()
}

/// Open owner actions (`OWNER_ACTION_REQUIRED` without `OWNER_ACTION_RESOLVED`).
pub fn open_actions(db: &Connection, task: &str) -> CoreResult<Vec<Value>> {
    Ok(
        open(db, task, "OWNER_ACTION_REQUIRED", "OWNER_ACTION_RESOLVED")?
            .into_iter()
            .map(|(_, v)| v)
            .collect(),
    )
}

/// Open external waits, each with its recheck state (`attempt`, `next_check_at`).
pub fn open_waits(db: &Connection, task: &str) -> CoreResult<Vec<Value>> {
    let mut out = Vec::new();
    for (_, mut wait) in open(db, task, "EXTERNAL_WAIT_STARTED", "EXTERNAL_WAIT_ENDED")? {
        let last: Option<String> = db
            .query_row(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='EXTERNAL_WAIT_RECHECKED'
                 AND json_extract(payload_json,'$.id')=? ORDER BY id DESC LIMIT 1",
                (task, wait["id"].as_str()),
                |r| r.get(0),
            )
            .optional()?;
        if let Some(last) = last.and_then(|l| serde_json::from_str::<Value>(&l).ok()) {
            wait["attempt"] = last["attempt"].clone();
            wait["next_check_at"] = last["next_check_at"].clone();
            wait["last_checked_at"] = last["checked_at"].clone();
        }
        out.push(wait);
    }
    Ok(out)
}

/// Owner decisions the coordinator still has to ask about: no owner answer, owner
/// message or reopen came after them.
pub fn open_decisions(db: &Connection, task: &str) -> CoreResult<Vec<Value>> {
    let rows = db
        .prepare(
            "SELECT s.payload_json FROM runtime_events s WHERE s.task_id=?1 AND s.type='OWNER_DECISION_REQUIRED'
             AND NOT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=?1 AND e.id>s.id AND e.type IN
                 ('USER_ANSWER_RECEIVED','USER_STEERING','WORKER_USER_ANSWER_RECEIVED','TASK_REOPENED'))
             ORDER BY s.id",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.iter()
        .map(|raw| Ok(serde_json::from_str(raw)?))
        .collect()
}

fn is_open(db: &Connection, task: &str, class: &str, id: &str) -> CoreResult<bool> {
    let list = match class {
        ACCESS => open_actions(db, task)?,
        EXTERNAL_OUTAGE => open(db, task, "EXTERNAL_WAIT_STARTED", "EXTERNAL_WAIT_ENDED")?
            .into_iter()
            .map(|(_, v)| v)
            .collect(),
        OWNER_DECISION => open_decisions(db, task)?,
        _ => return Ok(false),
    };
    Ok(list.iter().any(|v| v["id"] == id))
}

/// Which of the obstacle ids `ids` are still open.
pub fn still_open(db: &Connection, task: &str, ids: &[Value]) -> CoreResult<Vec<Value>> {
    let mut out = Vec::new();
    for id in ids.iter().filter_map(Value::as_str) {
        let class = match id.split('_').next() {
            Some("oa") => ACCESS,
            Some("ew") => EXTERNAL_OUTAGE,
            Some("od") => OWNER_DECISION,
            _ => continue,
        };
        if is_open(db, task, class, id)? {
            out.push(json!(id));
        }
    }
    Ok(out)
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The environment preflight fact for a service, and whether it shows the access.
fn environment_fact(service: &str) -> Option<(&'static str, &'static str)> {
    match service {
        "github" => Some(("gh_auth", "logged_in")),
        "codex" => Some(("codex_login", "logged_in")),
        "claude" => Some(("claude_login", "logged_in")),
        "sudo" => Some(("sudo", "passwordless")),
        _ => None,
    }
}

fn latest_environment(db: &Connection, task: &str) -> CoreResult<Option<(i64, Value)>> {
    let row: Option<(i64, String)> = db
        .query_row(
            "SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='ENVIRONMENT_PREFLIGHT'
             ORDER BY id DESC LIMIT 1",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(id, raw)| Ok((id, serde_json::from_str(&raw)?)))
        .transpose()
}

/// Record (or reuse) the obstacle a classification calls for. Returns its id, or
/// `None` for `technical`. One open owner action per task and service; one open
/// external wait per task and host; one open owner decision per node.
pub fn open_obstacle_tx(
    db: &Transaction<'_>,
    task: &str,
    found: &Classification,
    node: Option<&str>,
    revision: i64,
) -> CoreResult<Option<String>> {
    match found.class {
        ACCESS => {
            let service = found
                .service
                .as_deref()
                .filter(|s| access_service_ok(s))
                .unwrap_or("unknown");
            if let Some(existing) = open_actions(db, task)?
                .into_iter()
                .find(|a| a["service"] == service)
            {
                return Ok(existing["id"].as_str().map(str::to_owned));
            }
            let id = new_id("oa");
            let mut payload = json!({"id":id,"kind":"access","service":service,"detail":found.detail,
                "node":node,"revision":revision});
            // What the environment preflight showed when the access was found missing:
            // only a later preflight that shows it present resolves the action.
            if let Some((fact, _)) = environment_fact(service) {
                payload["environment_status"] = latest_environment(db, task)?
                    .map(|(_, f)| f[fact]["status"].clone())
                    .unwrap_or(Value::Null);
            }
            event_tx(db, task, "OWNER_ACTION_REQUIRED", payload)?;
            Ok(Some(id))
        }
        EXTERNAL_OUTAGE => {
            let host = found.host.clone().unwrap_or_default();
            if let Some(existing) = open(db, task, "EXTERNAL_WAIT_STARTED", "EXTERNAL_WAIT_ENDED")?
                .into_iter()
                .find(|(_, w)| w["host"] == host.as_str())
            {
                return Ok(existing.1["id"].as_str().map(str::to_owned));
            }
            // A host that answered again within the last hour and failed again keeps
            // its place in the backoff instead of starting over at one minute.
            let now = unix_now();
            let previous: i64 = db.query_row(
                "SELECT coalesce(max(json_extract(r.payload_json,'$.attempt')),0) FROM runtime_events r
                 WHERE r.task_id=?1 AND r.type='EXTERNAL_WAIT_RECHECKED' AND json_extract(r.payload_json,'$.id')=(
                    SELECT json_extract(e.payload_json,'$.id') FROM runtime_events e WHERE e.task_id=?1
                    AND e.type='EXTERNAL_WAIT_ENDED' AND json_extract(e.payload_json,'$.host')=?2
                    AND json_extract(e.payload_json,'$.at')>=?3 ORDER BY e.id DESC LIMIT 1)",
                (task, &host, now - 3600),
                |r| r.get(0),
            )?;
            let id = new_id("ew");
            let attempt = previous.max(0) as usize;
            event_tx(
                db,
                task,
                "EXTERNAL_WAIT_STARTED",
                json!({"id":id,"kind":"external_outage","host":host,"port":found.port,"scheme":found.scheme,
                    "probe":found.probe,"detail":found.detail,"node":node,"revision":revision,
                    "attempt":attempt,"next_check_at":now+backoff(attempt)}),
            )?;
            Ok(Some(id))
        }
        OWNER_DECISION => {
            if let Some(existing) = open_decisions(db, task)?
                .into_iter()
                .find(|d| d["node"].as_str() == node && d["revision"] == revision)
            {
                return Ok(existing["id"].as_str().map(str::to_owned));
            }
            let id = new_id("od");
            event_tx(
                db,
                task,
                "OWNER_DECISION_REQUIRED",
                json!({"id":id,"kind":"owner_decision","detail":found.detail,"node":node,"revision":revision}),
            )?;
            Ok(Some(id))
        }
        _ => Ok(None),
    }
}

/// A decision text without the runtime's own lead-in, normalized.
fn decision_subject(detail: &str) -> String {
    let lower = super::review::normalize_text(detail);
    let mut subject = lower.as_str();
    for prefix in [
        "a decision only the owner can make blocks this work:",
        "the requirements conflict and need the owner's decision:",
    ] {
        if let Some(rest) = subject.strip_prefix(prefix) {
            subject = rest;
        }
    }
    subject.trim().trim_end_matches('…').trim().to_owned()
}

/// Significant words of a decision text: at least four letters, digits masked.
fn decision_words(subject: &str) -> std::collections::BTreeSet<String> {
    subject
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(|w| {
            w.chars()
                .map(|c| if c.is_ascii_digit() { '0' } else { c })
                .collect()
        })
        .collect()
}

/// Whether two owner-decision texts name the same decision: equal after
/// normalization, only file names the earlier one named, or mostly the same words
/// (a reworded detail of the next attempt).
fn same_decision(current: &str, earlier: &str) -> bool {
    let (a, b) = (decision_subject(current), decision_subject(earlier));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b || a.starts_with(&b) || b.starts_with(&a) {
        return true;
    }
    let (fa, fb) = (
        super::review::named_files(&a),
        super::review::named_files(&b),
    );
    if !fa.is_empty() && fa.is_subset(&fb) {
        return true;
    }
    let (wa, wb) = (decision_words(&a), decision_words(&b));
    let union = wa.union(&wb).count();
    union > 0 && wa.intersection(&wb).count() * 2 >= union
}

/// The owner's answer to an earlier decision of this task that `detail` repeats: an
/// answered owner question the detail cites, or an earlier `OWNER_DECISION_REQUIRED`
/// (since the newest reopen or owner message, in any plan revision) with the same
/// decision and an owner answer after it. `None` when the decision is new.
pub fn answered_decision_tx(
    db: &Connection,
    task: &str,
    detail: &str,
) -> CoreResult<Option<String>> {
    for id in super::review::cited_questions(detail) {
        if let Some(answer) = super::review::answer_text_tx(db, task, &id)? {
            return Ok(Some(answer));
        }
    }
    let since = super::review::repeat_window_tx(db, task)?;
    let rows = db
        .prepare(
            "SELECT id,json_extract(payload_json,'$.detail') FROM runtime_events WHERE task_id=?
             AND type='OWNER_DECISION_REQUIRED' AND id>? AND json_valid(payload_json) ORDER BY id DESC LIMIT 64",
        )?
        .query_map((task, since), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, earlier) in rows {
        if !same_decision(detail, earlier.as_deref().unwrap_or("")) {
            continue;
        }
        if let Some(answer) = super::review::answer_after_tx(db, task, id)? {
            return Ok(Some(answer));
        }
    }
    Ok(None)
}

/// Owner actions for `service` that were resolved or dismissed since the newest
/// reopen or owner message.
fn closed_actions(db: &Connection, task: &str, service: &str) -> CoreResult<i64> {
    let since = super::review::repeat_window_tx(db, task)?;
    Ok(db.query_row(
        "SELECT count(*) FROM runtime_events s WHERE s.task_id=?1 AND s.type='OWNER_ACTION_REQUIRED' AND s.id>?3
         AND json_extract(s.payload_json,'$.service')=?2
         AND EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=?1 AND e.type='OWNER_ACTION_RESOLVED' AND e.id>s.id
             AND json_extract(e.payload_json,'$.id')=json_extract(s.payload_json,'$.id'))",
        (task, service, since),
        |r| r.get(0),
    )?)
}

/// Append disclosed `unverified` runtime limits to a failed node's result, so the
/// final result discloses them under Not verified.
fn disclose_on_node_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    limits: &[Value],
) -> CoreResult<()> {
    let raw: Option<Option<String>> = db
        .query_row(
            "SELECT result_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?;
    let Some(Some(raw)) = raw else { return Ok(()) };
    let Ok(mut result) = serde_json::from_str::<Value>(&raw) else {
        return Ok(());
    };
    if !result.is_object() {
        return Ok(());
    }
    match result["checks"].as_array_mut() {
        Some(checks) => checks.extend(limits.iter().cloned()),
        None => result["checks"] = json!(limits),
    }
    db.execute(
        "UPDATE runtime_nodes SET result_json=? WHERE task_id=? AND assignment_ref=?",
        (result.to_string(), task, assignment),
    )?;
    Ok(())
}

/// An obstacle the owner already settled is never raised again. An owner decision the
/// owner answered ([`answered_decision_tx`]) is not another `OWNER_DECISION_REQUIRED`
/// and `ask_owner` continuation: the answer is applied, the failure takes the
/// technical route with the answer in its detail, and the decision is a disclosed
/// limit (owner store 2026-11-21: one review's declined reads were asked about after
/// every new plan). Missing access whose owner action was resolved or dismissed
/// [`ACCESS_REPEAT_LIMIT`] times opens no further action: technical, with the missing
/// access disclosed. Records `LOOP_BROKEN`; other classifications pass unchanged.
fn settle_repeats_tx(
    db: &Transaction<'_>,
    task: &str,
    found: Classification,
    node: Option<&str>,
    assignment: Option<&str>,
) -> CoreResult<Classification> {
    let (kind, subject, limit, settled) = match found.class {
        OWNER_DECISION => {
            let Some(answer) = answered_decision_tx(db, task, &found.detail)? else {
                return Ok(found);
            };
            let answer = clip(&answer, 200);
            let subject = decision_subject(&found.detail);
            let limit = json!({"name":clip(&format!("Owner decision applied: {subject}"), 160),
                "outcome":"unverified",
                "blocker":clip(&format!("The owner already answered this decision{}; it was not asked again and the work is not certified beyond that answer.",
                    if answer.is_empty() { String::new() } else { format!(" (\"{answer}\")") }), 600),
                "source":"runtime"});
            let settled = Classification::new(
                TECHNICAL,
                "owner_decision_answered",
                clip(
                    &format!(
                        "The owner already answered this decision{}; apply that answer instead of asking again: {subject}",
                        if answer.is_empty() { String::new() } else { format!(" (\"{answer}\")") }
                    ),
                    DETAIL_CHARS,
                ),
            );
            ("owner_decision", subject, limit, settled)
        }
        ACCESS => {
            let service = found.service.clone().unwrap_or_else(|| "unknown".into());
            let closed = closed_actions(db, task, &service)?;
            if closed < ACCESS_REPEAT_LIMIT {
                return Ok(found);
            }
            let limit = json!({"name":format!("Access to {service} not available"),"outcome":"unverified",
                "blocker":format!("Access to {service} was requested from the owner {closed} times and is still missing; it is not requested again, the work continues without it and nothing that needs it is verified."),
                "source":"runtime"});
            let mut settled = Classification::new(
                TECHNICAL,
                "access_repeats",
                clip(
                    &format!("Access to {service} is still missing after {closed} owner actions; continue without it: {}", found.detail),
                    DETAIL_CHARS,
                ),
            );
            settled.service = Some(service.clone());
            ("missing_access", service, limit, settled)
        }
        _ => return Ok(found),
    };
    if let Some(assignment) = assignment {
        disclose_on_node_tx(db, task, assignment, std::slice::from_ref(&limit))?;
    }
    event_tx(
        db,
        task,
        "LOOP_BROKEN",
        json!({"key":super::review::loop_key(kind, node.unwrap_or(""), &subject),"kind":kind,"node":node,
            "action":"disclosed_limit","limits":[limit]}),
    )?;
    Ok(settled)
}

fn classified(db: &Connection, task: &str, assignment: &str) -> CoreResult<Option<Value>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='FAILURE_CLASSIFIED'
             AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|r| Ok(serde_json::from_str(&r)?)).transpose()
}

/// Times this node failed on `host` again after a recheck had found it answering.
fn outage_repeats(db: &Connection, task: &str, node: &str, host: &str) -> CoreResult<i64> {
    Ok(db.query_row(
        "SELECT count(*) FROM runtime_events e WHERE e.task_id=?1 AND e.type='EXTERNAL_WAIT_ENDED'
         AND json_extract(e.payload_json,'$.reason') IN ('recheck_succeeded','backoff_elapsed')
         AND json_extract(e.payload_json,'$.host')=?3
         AND EXISTS(SELECT 1 FROM runtime_events c WHERE c.task_id=?1 AND c.type='FAILURE_CLASSIFIED'
             AND json_extract(c.payload_json,'$.node')=?2 AND json_extract(c.payload_json,'$.obstacle')=json_extract(e.payload_json,'$.id'))
         AND e.id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)",
        (task, node, host),
        |r| r.get(0),
    )?)
}

/// Classify every failed node of the current revision that is not classified yet,
/// recording `FAILURE_CLASSIFIED` and the obstacle it calls for. Idempotent.
pub fn classify_failed_tx(db: &Transaction<'_>, task: &str) -> CoreResult<()> {
    let (_, revision) = domain::task_tx(db, task)?;
    let rows = db
        .prepare(
            "SELECT node_id,assignment_ref FROM runtime_nodes WHERE task_id=? AND revision=?
             AND status='failed' AND assignment_ref IS NOT NULL ORDER BY rowid",
        )?
        .query_map((task, revision), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (node, assignment) in rows {
        if classified(db, task, &assignment)?.is_some() {
            continue;
        }
        let found = classify(&node_signals(db, task, &assignment)?);
        let mut found = settle_repeats_tx(db, task, found, Some(&node), Some(&assignment))?;
        if found.class == EXTERNAL_OUTAGE
            && outage_repeats(db, task, &node, found.host.as_deref().unwrap_or(""))?
                >= OUTAGE_REPEAT_LIMIT
        {
            found = Classification::new(
                TECHNICAL,
                "outage_repeats_after_recovery",
                clip(
                    &format!(
                        "The work keeps failing on {} although it answers again",
                        found.host.as_deref().unwrap_or("the host")
                    ),
                    DETAIL_CHARS,
                ),
            );
        }
        let obstacle = open_obstacle_tx(db, task, &found, Some(&node), revision)?;
        let mut payload = found.to_json();
        payload["node"] = json!(node);
        payload["revision"] = json!(revision);
        payload["assignment_ref"] = json!(assignment);
        payload["obstacle"] = json!(obstacle);
        event_tx(db, task, "FAILURE_CLASSIFIED", payload)?;
    }
    Ok(())
}

/// A failed node of the current revision with its classification.
struct Failed {
    node: String,
    assignment: String,
    class: String,
    obstacle: Option<String>,
}

fn failed_nodes(db: &Connection, task: &str) -> CoreResult<Vec<Failed>> {
    let rows = db
        .prepare(
            "SELECT n.node_id,n.assignment_ref FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
             WHERE n.task_id=? AND n.revision=t.revision AND n.status='failed' AND n.assignment_ref IS NOT NULL
             ORDER BY n.rowid",
        )?
        .query_map([task], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for (node, assignment) in rows {
        let c = classified(db, task, &assignment)?;
        out.push(Failed {
            node,
            assignment,
            class: c
                .as_ref()
                .and_then(|c| c["class"].as_str())
                .unwrap_or(TECHNICAL)
                .to_owned(),
            obstacle: c
                .as_ref()
                .and_then(|c| c["obstacle"].as_str())
                .map(str::to_owned),
        });
    }
    Ok(out)
}

/// Failed nodes of the current revision parked behind an open obstacle, with their
/// class and obstacle id. Their dependents wait; unrelated nodes keep running.
pub fn parked(db: &Connection, task: &str) -> CoreResult<Vec<(String, String, String)>> {
    let mut out = Vec::new();
    for f in failed_nodes(db, task)? {
        if let Some(id) = f.obstacle.as_deref() {
            if f.class != TECHNICAL && is_open(db, task, &f.class, id)? {
                out.push((f.node, f.class, id.to_owned()));
            }
        }
    }
    Ok(out)
}

/// Whether a failed node of the current revision is not parked: the ordinary failure
/// route (the coordinator, then the strategy ladder) applies to it.
pub fn unparked_failure(db: &Connection, task: &str) -> CoreResult<bool> {
    let parked: Vec<String> = parked(db, task)?.into_iter().map(|p| p.0).collect();
    Ok(failed_nodes(db, task)?
        .iter()
        .any(|f| !parked.contains(&f.node)))
}

/// Retry, at once, each parked node whose access action was resolved or whose
/// external wait ended. Returns the retried nodes.
pub fn retry_unblocked_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Vec<String>> {
    let (state, revision) = domain::task_tx(db, task)?;
    if !matches!(state, TaskState::Executing | TaskState::Recovering) {
        return Ok(Vec::new());
    }
    let mut retried = Vec::new();
    for f in failed_nodes(db, task)? {
        let (true, Some(id)) = (
            matches!(f.class.as_str(), ACCESS | EXTERNAL_OUTAGE),
            f.obstacle.as_deref(),
        ) else {
            continue;
        };
        if is_open(db, task, &f.class, id)? {
            continue;
        }
        let previous: Option<String> = db
            .query_row(
                "SELECT json_extract(result_json,'$.report_ref') FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                (task, &f.assignment),
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let changed = db.execute(
            "UPDATE runtime_nodes SET status='pending',assignment_ref=NULL,result_json=NULL
             WHERE task_id=? AND revision=? AND assignment_ref=? AND status='failed'",
            (task, revision, &f.assignment),
        )?;
        if changed != 1 {
            continue;
        }
        let count: i64 = db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'
             AND json_extract(payload_json,'$.node')=? AND json_extract(payload_json,'$.revision')=?",
            (task, &f.node, revision),
            |r| r.get(0),
        )?;
        event_tx(
            db,
            task,
            "ASSIGNMENT_RETRIED",
            json!({"node":f.node,"revision":revision,"retry":count+1,"previous_assignment":f.assignment,
                "previous_report":previous,"failure_class":f.class,"obstacle":id,"obstacle_retry":true}),
        )?;
        retried.push(f.node);
    }
    Ok(retried)
}

/// When nothing else of the plan can run (no live worker, no dependency-ready
/// node) and parked nodes remain, settle what the parked work waits for: an owner
/// decision becomes a stop that asks the owner (`owner_decision_required`), and
/// access or an outage pauses the task, without cancelling its plan, until the
/// obstacle clears. Returns whether the task changed state.
pub fn settle_parked_tx(db: &Transaction<'_>, task: &str) -> CoreResult<bool> {
    let (state, _) = domain::task_tx(db, task)?;
    if state != TaskState::Executing {
        return Ok(false);
    }
    let parked = parked(db, task)?;
    if parked.is_empty() || unparked_failure(db, task)? {
        return Ok(false);
    }
    let busy: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
             WHERE n.task_id=? AND n.revision=t.revision AND n.status='running')
         OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND kind='worker' AND state!='released')",
        (task, task),
        |r| r.get(0),
    )?;
    if busy {
        return Ok(false);
    }
    let nodes = db
        .prepare(
            "SELECT n.node_id,n.stage,n.status,n.dependencies_json FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
             WHERE n.task_id=? AND n.revision=t.revision ORDER BY n.rowid",
        )?
        .query_map([task], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(id, stage, status, deps)| {
            Ok(crate::scheduler::Node { id, stage, status, dependencies: serde_json::from_str(&deps)? })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    if !crate::scheduler::dependency_ready(&nodes, "execution")?.is_empty() {
        return Ok(false);
    }
    if parked.iter().any(|p| p.1 == OWNER_DECISION) {
        domain::stop_tx(db, task, OWNER_DECISION_STOP)?;
    } else {
        super::automatic::obstacle_pause_tx(db, task, &parked)?;
    }
    Ok(true)
}

/// The obstacle a runtime stop calls for, from the stop's own evidence: the dead ends
/// of an incomplete review, a failed reviewer, or the parked owner decisions of
/// [`OWNER_DECISION_STOP`]. Other stops are `technical` (a failed final verification
/// included: its sandbox has no network and no owner logins, so its output is no
/// evidence about a remote host or an account).
pub fn classify_stop(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    reason: &str,
) -> CoreResult<(Classification, Option<String>)> {
    let technical = || {
        (
            Classification::new(TECHNICAL, "technical", clip(reason, DETAIL_CHARS)),
            None,
        )
    };
    match reason {
        OWNER_DECISION_STOP => {
            let decisions = open_decisions(db, task)?;
            let detail = decisions
                .iter()
                .filter_map(|d| d["detail"].as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let node = decisions
                .first()
                .and_then(|d| d["node"].as_str())
                .map(str::to_owned);
            Ok((
                Classification::new(
                    OWNER_DECISION,
                    "parked_owner_decision",
                    clip(&detail, DETAIL_CHARS),
                ),
                node,
            ))
        }
        "review_incomplete" => {
            let raw: Option<String> = db
                .query_row(
                    "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_DEAD_END_ROUTED'
                     AND json_extract(payload_json,'$.revision')=? ORDER BY id DESC LIMIT 1",
                    (task, revision),
                    |r| r.get(0),
                )
                .optional()?;
            let Some(event) = raw.and_then(|r| serde_json::from_str::<Value>(&r).ok()) else {
                return Ok(technical());
            };
            let review = event["review"].as_str().map(str::to_owned);
            let blocked = event["blocked"].as_array().cloned().unwrap_or_default();
            // An owner decision the owner already made is never asked again: such
            // an item is skipped here (`review::decided_owner_item_tx`).
            let mut answered = None;
            for kind in [OWNER_DECISION, "missing_access"] {
                for item in blocked.iter().filter(|b| b["blocker"] == kind) {
                    let (name, reason) = (
                        item["item"].as_str().unwrap_or(""),
                        item["reason"].as_str().unwrap_or(""),
                    );
                    if kind == OWNER_DECISION {
                        if let Some(answer) =
                            super::review::decided_owner_item_tx(db, task, name, reason)?
                        {
                            answered.get_or_insert((name.to_owned(), answer));
                            continue;
                        }
                    }
                    let signals = Signals {
                        blocker_kind: Some(kind.into()),
                        blocker_detail: Some(format!("{name}: {reason}")),
                        ..Default::default()
                    };
                    let found =
                        settle_repeats_tx(db, task, classify(&signals), review.as_deref(), None)?;
                    return Ok((found, review));
                }
            }
            if let Some((item, answer)) = answered {
                return Ok((
                    Classification::new(
                        TECHNICAL,
                        "owner_decision_answered",
                        clip(
                            &format!(
                                "The owner already decided about {item}{}; apply that answer instead of asking again",
                                if answer.is_empty() { String::new() } else { format!(" (\"{}\")", clip(&answer, 200)) }
                            ),
                            DETAIL_CHARS,
                        ),
                    ),
                    review,
                ));
            }
            Ok(technical())
        }
        "review_blocked" => {
            let review: Option<(String, String)> = db
                .query_row(
                    "SELECT node_id,assignment_ref FROM runtime_nodes WHERE task_id=? AND revision=? AND stage='review'
                     AND status='failed' AND assignment_ref IS NOT NULL ORDER BY rowid DESC LIMIT 1",
                    (task, revision),
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match review {
                // A review that ended because another actor kept changing the
                // checkout outside the task's files is infrastructure: the runtime
                // waits by time alone and reviews again, without a rung
                // (`review::checkout_drift_tx`).
                Some((node, assignment))
                    if super::review::checkout_drift_tx(db, task, &assignment)?.is_some() =>
                {
                    let mut c = Classification::new(
                        EXTERNAL_OUTAGE,
                        "checkout_drift",
                        "Another actor kept changing the checkout outside this task's files while the review ran; the runtime waits and reviews again".into(),
                    );
                    c.host = Some("project-checkout".into());
                    c.probe = Some(TIMER);
                    Ok((c, Some(node)))
                }
                Some((node, assignment)) => {
                    let found = classify(&node_signals(db, task, &assignment)?);
                    let found = settle_repeats_tx(db, task, found, Some(&node), Some(&assignment))?;
                    Ok((found, Some(node)))
                }
                None => Ok(technical()),
            }
        }
        _ => Ok(technical()),
    }
}

/// Record one runtime recheck of an external wait. A successful recheck ends the wait
/// (`EXTERNAL_WAIT_ENDED`, reason `recheck_succeeded`); a failed one schedules the next
/// recheck with [`backoff`]. Returns whether the wait ended.
pub fn record_recheck_tx(
    db: &Transaction<'_>,
    task: &str,
    id: &str,
    ok: bool,
    now: i64,
    detail: &str,
) -> CoreResult<bool> {
    let Some(wait) = open_waits(db, task)?.into_iter().find(|w| w["id"] == id) else {
        return Ok(false);
    };
    let attempt = wait["attempt"].as_u64().unwrap_or(0) as usize + 1;
    // A timer wait is due exactly when its backoff elapsed; nothing is probed.
    if wait["probe"] == TIMER {
        event_tx(
            db,
            task,
            "EXTERNAL_WAIT_RECHECKED",
            json!({"id":id,"ok":false,"elapsed":true,"attempt":attempt,"checked_at":now,
                "detail":"backoff elapsed; the runtime retries the work"}),
        )?;
        event_tx(
            db,
            task,
            "EXTERNAL_WAIT_ENDED",
            json!({"id":id,"reason":"backoff_elapsed","host":wait["host"],"node":wait["node"],"at":now}),
        )?;
        return Ok(true);
    }
    if ok {
        event_tx(
            db,
            task,
            "EXTERNAL_WAIT_RECHECKED",
            json!({"id":id,"ok":true,"attempt":attempt,"checked_at":now,"detail":clip(detail, 200)}),
        )?;
        event_tx(
            db,
            task,
            "EXTERNAL_WAIT_ENDED",
            json!({"id":id,"reason":"recheck_succeeded","host":wait["host"],"node":wait["node"],"at":now}),
        )?;
        return Ok(true);
    }
    event_tx(
        db,
        task,
        "EXTERNAL_WAIT_RECHECKED",
        json!({"id":id,"ok":false,"attempt":attempt,"checked_at":now,"next_check_at":now+backoff(attempt),"detail":clip(detail, 200)}),
    )?;
    Ok(false)
}

/// External waits whose recheck is due at `now`.
pub fn due_waits(db: &Connection, task: &str, now: i64) -> CoreResult<Vec<Value>> {
    Ok(open_waits(db, task)?
        .into_iter()
        .filter(|w| w["next_check_at"].as_i64().is_none_or(|at| at <= now))
        .collect())
}

/// Services of open owner actions that an environment preflight can recheck.
pub fn environment_services(db: &Connection, task: &str) -> CoreResult<Vec<String>> {
    Ok(open_actions(db, task)?
        .into_iter()
        .filter(|a| a["service"].as_str().and_then(environment_fact).is_some())
        .filter(|a| {
            // A preflight that already showed the access when it failed cannot show
            // that the owner fixed it; only the owner's resolution can.
            let (_, present) = environment_fact(a["service"].as_str().unwrap()).unwrap();
            a["environment_status"] != present
        })
        .filter_map(|a| a["service"].as_str().map(str::to_owned))
        .collect())
}

/// Resolve open owner actions that fresh environment `facts` show fixed
/// (`OWNER_ACTION_RESOLVED` with `by: environment_recheck`). Records the facts as
/// the task's newest `ENVIRONMENT_PREFLIGHT` when anything was resolved.
pub fn resolve_by_environment_tx(
    db: &Transaction<'_>,
    task: &str,
    facts: &Value,
) -> CoreResult<Vec<String>> {
    let mut resolved = Vec::new();
    for action in open_actions(db, task)? {
        let Some(service) = action["service"].as_str() else {
            continue;
        };
        let Some((fact, present)) = environment_fact(service) else {
            continue;
        };
        if action["environment_status"] == present || facts[fact]["status"] != present {
            continue;
        }
        resolved.push(action["id"].as_str().unwrap_or("").to_owned());
    }
    if !resolved.is_empty() {
        event_tx(db, task, "ENVIRONMENT_PREFLIGHT", facts.clone())?;
        for id in &resolved {
            event_tx(
                db,
                task,
                "OWNER_ACTION_RESOLVED",
                json!({"id":id,"by":"environment_recheck"}),
            )?;
        }
    }
    Ok(resolved)
}

/// Plans wait for the owner's answer: while an owner decision is open and the
/// coordinator has not asked about it, a new plan is refused (`owner_question_required`).
pub fn require_owner_question_tx(db: &Transaction<'_>, task: &str) -> CoreResult<()> {
    let pending: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events s WHERE s.task_id=?1 AND s.type='OWNER_DECISION_REQUIRED'
         AND NOT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=?1 AND e.id>s.id AND e.type IN
             ('USER_QUESTION_REQUESTED','USER_ANSWER_RECEIVED','USER_STEERING','WORKER_USER_ANSWER_RECEIVED','TASK_REOPENED')))",
        [task],
        |r| r.get(0),
    )?;
    if pending {
        return refuse("owner_question_required");
    }
    Ok(())
}

/// The coordinator capsule's view of open obstacles: owner actions, external waits,
/// owner decisions and the newest classifications. Bounded.
pub fn capsule(db: &Connection, task: &str) -> CoreResult<Value> {
    let actions: Vec<Value> = open_actions(db, task)?
        .into_iter()
        .take(8)
        .map(|a| json!({"id":a["id"],"service":a["service"],"detail":a["detail"],"node":a["node"]}))
        .collect();
    let waits: Vec<Value> = open_waits(db, task)?
        .into_iter()
        .take(8)
        .map(|w| {
            json!({"id":w["id"],"host":w["host"],"detail":w["detail"],"node":w["node"],
            "rechecks":w["attempt"],"next_check_at":w["next_check_at"]})
        })
        .collect();
    let decisions: Vec<Value> = open_decisions(db, task)?
        .into_iter()
        .take(8)
        .map(|d| json!({"id":d["id"],"detail":d["detail"],"node":d["node"]}))
        .collect();
    let recent = db
        .prepare(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='FAILURE_CLASSIFIED' ORDER BY id DESC LIMIT 6",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|raw| serde_json::from_str::<Value>(&raw).ok())
        .map(|c| json!({"node":c["node"],"revision":c["revision"],"class":c["class"],"service":c["service"],
            "host":c["host"],"detail":c["detail"]}))
        .collect::<Vec<_>>();
    if actions.is_empty() && waits.is_empty() && decisions.is_empty() && recent.is_empty() {
        return Ok(json!({"open":false}));
    }
    Ok(
        json!({"open":!(actions.is_empty() && waits.is_empty() && decisions.is_empty()),
        "owner_actions":actions,"external_waits":waits,"owner_decisions":decisions,"recent_classifications":recent,
        "note":"The runtime classifies failures. Work blocked by missing access waits for the owner's action and is retried by the runtime when it is resolved; work blocked by an outage is rechecked by the runtime without model calls and resumed when the host answers; do not plan workarounds, retries or escalations for them. An open owner decision must be asked with user_question_request (one question with concrete options) before the blocked work is planned again."}),
    )
}

/// The coordinator instruction of an `ask_owner` continuation.
pub fn ask_owner_instruction(detail: &str) -> String {
    format!(
        "A decision only the owner can make blocks the work ({}). Call user_question_request now with one question and concrete options (the alternatives named in the cited reports); do not plan, retry or escalate the blocked work before the owner answers. Unrelated work already finished stays done.",
        if detail.trim().is_empty() { "see the cited reports" } else { detail.trim() }
    )
}

/// The `TASK_PROGRESS` markers a runtime event stands for: an accepted plan, a
/// completed node (and its passed checks), a passed review or final verification.
/// Another component builds the owner's "no progress" notice on these markers.
pub fn progress_for(db: &Connection, kind: &str, payload: &Value) -> CoreResult<Vec<Value>> {
    let node = |assignment: &Value| -> CoreResult<Value> {
        Ok(match assignment.as_str() {
            Some(a) => json!(db
                .query_row(
                    "SELECT node_id FROM runtime_nodes WHERE assignment_ref=?",
                    [a],
                    |r| r.get::<_, String>(0)
                )
                .optional()?),
            None => Value::Null,
        })
    };
    Ok(match kind {
        "PLAN_ACCEPTED" => {
            vec![json!({"kind":"plan_accepted","node":null,"revision":payload["revision"]})]
        }
        "WORKER_COMPLETED"
            if !matches!(
                payload["result"]["verdict"].as_str(),
                Some("REQUEST_CHANGES" | "INCOMPLETE" | "BLOCK")
            ) =>
        {
            let node = node(&payload["assignment_ref"])?;
            let passed = payload["result"]["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["outcome"] == "passed")
                .count();
            let mut out = Vec::new();
            if passed > 0 {
                out.push(json!({"kind":"check_passed","node":node,"count":passed}));
            }
            out.push(json!({"kind":"node_completed","node":node}));
            out
        }
        "VERIFICATION_PASSED" => {
            vec![json!({"kind":"verification_passed","node":null,"revision":payload["revision"]})]
        }
        _ => Vec::new(),
    })
}
