#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use workspace_service::{
    config_read_model::{
        settings_view,
        usage_refresh::{
            checked_recently, merge, newest_codex, parse_claude, parse_claude_text, parse_codex,
            refresh, Refresh, Windows, REFRESH_AFTER_SECONDS,
        },
        ConfigurationReader,
    },
    native_routes::RouteEnvironment,
};

/// 2026-11-16T12:00:00Z.
const NOW: f64 = 1_794_830_400.0;
/// 2026-11-17T02:59:00Z, as printed `Nov 17, 2:59am (UTC)`.
const RESET: f64 = 1_794_884_340.0;
const ID: &str = "cn_3aec66405544";

/// The shape installed Claude Code 2.1.323/2.1.325 prints for `-p /usage --output-format json`.
fn claude_output(result: &str) -> Value {
    json!({"type":"result","subtype":"success","is_error":false,"num_turns":0,"total_cost_usd":0,
        "local_command":"usage","duration_api_ms":0,"modelUsage":{},"stop_reason":null,
        "usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},
        "result":result})
}
const BOTH: &str = "You are currently using your subscription to power your Claude Code usage\n\n\
Current week (all models): 67% used · resets Nov 17, 2:59am (UTC)\n\
Current week (Fable): 2% used · resets Nov 17, 2:59am (UTC)\n\n\
What's contributing to your usage?\n  79%  Start fresh when you switch tasks · run /clear\n";
fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

#[test]
fn claude_usage_reads_both_weekly_windows_exactly() {
    let windows = parse_claude(&bytes(&claude_output(BOTH)), NOW).unwrap();
    assert_eq!(
        Value::Object(windows),
        json!({"seven_day":{"used":67.0,"resets_at":RESET},"seven_day_fable":{"used":2.0,"resets_at":RESET}})
    );
    // A session line is not a weekly window and its absence changes nothing.
    let with_session = format!("Current session: 12% used · resets 4pm (UTC)\n{BOTH}");
    assert_eq!(parse_claude_text(&with_session, NOW).unwrap().len(), 2);
}

#[test]
fn a_missing_fable_line_reports_only_the_general_window() {
    let text = "Current week (all models): 40% used · resets Nov 17, 2:59am (UTC)\n";
    let windows = parse_claude(&bytes(&claude_output(text)), NOW).unwrap();
    assert_eq!(windows.keys().collect::<Vec<_>>(), ["seven_day"]);
    assert_eq!(windows["seven_day"]["used"], 40.0);
}

#[test]
fn unparseable_lines_report_nothing_for_their_window() {
    for line in [
        "Current week (Fable): abc% used · resets Nov 17, 2:59am (UTC)",
        "Current week (Fable): 101% used · resets Nov 17, 2:59am (UTC)",
        "Current week (Fable): 2% used · resets whenever (UTC)",
        "Current week (Fable): 2% used · resets Nov 17, 2:59am (CET)",
        "Current week (Fable): 2% used · resets Nov 17, 13:59pm (UTC)",
        "Current week (Fable): 2% used · resets Feb 30, 2:59am (UTC)",
        // Further away than one weekly window: not a weekly reset.
        "Current week (Fable): 2% used · resets Dec 30, 2:59am (UTC)",
        // Already over.
        "Current week (Fable): 2% used · resets Nov 16, 2:59am (UTC)",
        "Current week (Fable): 2% used",
        "Current week (fable): 2% used · resets Nov 17, 2:59am (UTC)",
        "Current week (Fable) 2% used · resets Nov 17, 2:59am (UTC)",
        "Current week (Fable): 2 % used · resets Nov 17, 2:59am (UTC)",
    ] {
        let text =
            format!("Current week (all models): 67% used · resets Nov 17, 2:59am (UTC)\n{line}\n");
        let windows = parse_claude_text(&text, NOW).unwrap();
        assert_eq!(windows.keys().collect::<Vec<_>>(), ["seven_day"], "{line}");
    }
    // A reset in early January read late in December is next year's.
    let december = 1_798_632_000.0; // 2026-12-30T12:00:00Z
    let windows = parse_claude_text(
        "Current week (all models): 5% used · resets Jan 2, 6am (UTC)",
        december,
    )
    .unwrap();
    assert_eq!(windows["seven_day"]["resets_at"], 1_798_869_600.0); // 2027-01-02T06:00:00Z
                                                                    // Two lines for one window are ambiguous: nothing is trusted.
    let twice = format!("{BOTH}Current week (Fable): 9% used · resets Nov 17, 2:59am (UTC)\n");
    assert!(parse_claude_text(&twice, NOW).is_none());
}

#[test]
fn a_result_that_is_not_a_proven_free_local_command_is_never_trusted() {
    let good = claude_output(BOTH);
    assert!(parse_claude(&bytes(&good), NOW).is_some());
    let edits: [(&str, Value); 11] = [
        ("num_turns", json!(1)),
        ("total_cost_usd", json!(0.01)),
        ("is_error", json!(true)),
        ("subtype", json!("error_during_execution")),
        ("type", json!("assistant")),
        ("local_command", Value::Null),
        ("local_command", json!("cost")),
        ("duration_api_ms", json!(812)),
        ("modelUsage", json!({"claude-fable-5":{"inputTokens":3}})),
        (
            "usage",
            json!({"input_tokens":5,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}),
        ),
        ("result", json!(7)),
    ];
    for (key, value) in edits {
        let mut v = good.clone();
        v[key] = value;
        assert!(parse_claude(&bytes(&v), NOW).is_none(), "{key}");
    }
    let mut v = good.clone();
    v.as_object_mut().unwrap().remove("usage");
    assert!(parse_claude(&bytes(&v), NOW).is_none());
    for raw in [&b"not json"[..], b"[]", b"", b"{\"type\":\"result\"}"] {
        assert!(parse_claude(raw, NOW).is_none());
    }
    // An unknown, valid free result yields no windows rather than a guess.
    let other = parse_claude(
        &bytes(&claude_output(
            "/usage is only available for subscription plans.",
        )),
        NOW,
    );
    assert_eq!(other.unwrap().len(), 0);
}

/// The shape installed Codex 0.166.0 answers `account/rateLimits/read` with.
fn codex_response(primary: Value, secondary: Value) -> Value {
    json!({"id":2,"result":{"accountId":"redacted","ordinaryUsageAllowed":true,
        "rateLimits":{"limitId":"codex","primary":primary,"secondary":secondary,"planType":"pro"},
        "rateLimitsByLimitId":{}}})
}

#[test]
fn codex_account_read_keeps_the_rollout_window_names() {
    let weekly = json!({"usedPercent":33,"windowDurationMins":10080,"resetsAt":1794994458});
    let windows = parse_codex(&codex_response(weekly.clone(), Value::Null), NOW).unwrap();
    assert_eq!(
        Value::Object(windows),
        json!({"primary":{"used":33.0,"resets_at":1794994458.0,"window_minutes":10080}})
    );
    for bad in [
        json!({"usedPercent":"33","windowDurationMins":10080,"resetsAt":1794994458}),
        json!({"usedPercent":101,"windowDurationMins":10080,"resetsAt":1794994458}),
        json!({"usedPercent":33,"windowDurationMins":0,"resetsAt":1794994458}),
        json!({"usedPercent":33,"windowDurationMins":10080,"resetsAt":"soon"}),
        json!(7),
    ] {
        assert!(
            parse_codex(&codex_response(bad.clone(), Value::Null), NOW).is_none(),
            "{bad}"
        );
    }
    let mut other = codex_response(weekly.clone(), Value::Null);
    other["result"]["rateLimits"]["limitId"] = json!("codex_other");
    assert!(parse_codex(&other, NOW).is_none());
    let error = json!({"id":2,"error":{"code":-32600,"message":"not signed in"}});
    assert!(parse_codex(&error, NOW).is_none());
    // Another request's answer, or a foreign protocol version, is not this read.
    let mut wrong = codex_response(json!(null), Value::Null);
    wrong["id"] = json!(1);
    assert!(parse_codex(&wrong, NOW).is_none());
    let mut version = codex_response(json!(null), Value::Null);
    version["jsonrpc"] = json!("1.0");
    assert!(parse_codex(&version, NOW).is_none());
    version["jsonrpc"] = json!("2.0");
    assert!(parse_codex(&version, NOW).is_some());
    // An expired window is not reported.
    let over = json!({"usedPercent":90,"windowDurationMins":300,"resetsAt":NOW as i64});
    assert_eq!(
        parse_codex(&codex_response(weekly, over), NOW)
            .unwrap()
            .len(),
        1
    );
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "cw-usage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for d in [
            root.clone(),
            root.join("state"),
            root.join("home"),
            root.join("config"),
        ] {
            fs::create_dir(&d).unwrap();
            fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(root)
    }
    fn state(&self) -> PathBuf {
        self.0.join("state")
    }
    fn cache(&self) -> Value {
        serde_json::from_slice(&fs::read(self.quota("json")).unwrap()).unwrap()
    }
    fn quota(&self, ext: &str) -> PathBuf {
        self.state().join(format!("cortex/quota/{ID}.{ext}"))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn parsed(v: Value) -> Windows {
    v.as_object().unwrap().clone()
}
fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn refresh_cadence_is_bounded_per_connection_and_failures_are_cached_too() {
    let f = Fixture::new();
    let mut runs = 0;
    let first = parsed(
        json!({"seven_day":{"used":54.0,"resets_at":RESET},"seven_day_fable":{"used":0.0,"resets_at":RESET}}),
    );
    let outcome = refresh(&f.state(), ID, NOW, || {
        runs += 1;
        Some(first.clone())
    });
    assert_eq!(outcome.unwrap(), Refresh::Checked { ok: true });
    assert_eq!(f.cache()["seven_day"]["used"], 54.0);
    assert_eq!(f.cache()["seven_day_fable"]["source"], "cli");
    assert_eq!(mode(&f.quota("json")), 0o600);
    assert_eq!(mode(&f.quota("cli-probe.json")), 0o600);
    assert_eq!(mode(&f.state().join("cortex/quota")), 0o700);
    // Within the interval nothing runs, however often it is asked.
    for later in [1.0, 60.0, REFRESH_AFTER_SECONDS - 1.0] {
        assert!(checked_recently(&f.state(), ID, NOW + later));
        let again = refresh(&f.state(), ID, NOW + later, || {
            runs += 1;
            None
        });
        assert_eq!(again.unwrap(), Refresh::Fresh);
    }
    assert_eq!(runs, 1);
    // After it, a newer value replaces the old one.
    let next = NOW + REFRESH_AFTER_SECONDS;
    let newer = parsed(
        json!({"seven_day":{"used":67.0,"resets_at":RESET},"seven_day_fable":{"used":2.0,"resets_at":RESET}}),
    );
    assert_eq!(
        refresh(&f.state(), ID, next, || Some(newer.clone())).unwrap(),
        Refresh::Checked { ok: true }
    );
    assert_eq!(f.cache()["seven_day"]["used"], 67.0);
    assert_eq!(f.cache()["seven_day_fable"]["observed_at"], next);
    // An untrusted read keeps the windows and is not retried before the interval.
    let failed = next + REFRESH_AFTER_SECONDS;
    assert_eq!(
        refresh(&f.state(), ID, failed, || None).unwrap(),
        Refresh::Checked { ok: false }
    );
    assert_eq!(f.cache()["seven_day"]["used"], 67.0);
    assert_eq!(
        refresh(&f.state(), ID, failed + 10.0, || panic!(
            "ran within the interval"
        ))
        .unwrap(),
        Refresh::Fresh
    );
}

#[test]
fn a_held_lock_or_unsafe_cache_is_never_raced_or_overwritten() {
    let f = Fixture::new();
    refresh(&f.state(), ID, NOW, || Some(Windows::new())).unwrap();
    let lock = cortex_core::store::locks::AccessLock::acquire(
        &f.quota("cli-probe.lock"),
        false,
        true,
        std::time::Duration::ZERO,
    )
    .unwrap();
    let later = NOW + REFRESH_AFTER_SECONDS;
    assert_eq!(
        refresh(&f.state(), ID, later, || panic!("ran while locked")).unwrap(),
        Refresh::Busy
    );
    drop(lock);
    fs::write(f.quota("json"), b"{").unwrap();
    let outcome = refresh(&f.state(), ID, later, || Some(Windows::new()));
    assert_eq!(outcome.unwrap(), Refresh::Checked { ok: false });
    assert_eq!(fs::read(f.quota("json")).unwrap(), b"{");
    assert!(refresh(&f.state(), "../escape", later, || None).is_err());
}

#[test]
fn a_snapshot_drops_windows_it_no_longer_reports_but_never_other_sources() {
    let cache = json!({
        "seven_day":{"used":54.0,"resets_at":RESET,"observed_at":NOW - 600.0,"source":"cli"},
        "seven_day_fable":{"used":0.0,"resets_at":RESET,"observed_at":NOW - 600.0,"source":"cli"},
        "seven_day_opus":{"used":9.0,"resets_at":RESET,"observed_at":NOW - 600.0,"source":"event"},
        "seven_day_newer":{"used":1.0,"resets_at":RESET,"observed_at":NOW + 5.0,"source":"event"}
    });
    let read = parsed(
        json!({"seven_day":{"used":67.0,"resets_at":RESET},"seven_day_newer":{"used":50.0,"resets_at":RESET}}),
    );
    let merged = merge(&cache, &read, NOW);
    assert_eq!(merged["seven_day"]["used"], 67.0);
    assert!(merged.get("seven_day_fable").is_none());
    assert_eq!(merged["seven_day_opus"]["used"], 9.0);
    assert_eq!(merged["seven_day_newer"]["used"], 1.0);
}

#[test]
fn codex_display_takes_the_newer_of_rollout_and_account_read() {
    let rollout = json!({"primary":{"used":30.0,"resets_at":RESET,"window_minutes":10080,"observed_at":NOW - 3600.0}});
    let fresh = json!({"primary":{"used":33.0,"resets_at":RESET,"window_minutes":10080,"observed_at":NOW,"source":"cli"}});
    assert_eq!(newest_codex(&rollout, &fresh)["primary"]["used"], 33.0);
    let old = json!({"primary":{"used":20.0,"resets_at":RESET,"window_minutes":10080,"observed_at":NOW - 7200.0,"source":"cli"}});
    assert_eq!(newest_codex(&rollout, &old)["primary"]["used"], 30.0);
    // A cache value without a recorded read is never substituted.
    let unknown = json!({"primary":{"used":99.0}});
    assert_eq!(newest_codex(&rollout, &unknown)["primary"]["used"], 30.0);
}

/// Settings show each Claude weekly window by itself; unknown windows are absent, never zero.
#[test]
fn settings_carry_the_general_and_fable_windows_separately() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/settings-view-python.json")).unwrap();
    let mut input = fixture["cases"][0]["input"].clone();
    let claude =
        |windows: Value| json!([{"engine":"claude-agent-acp","id":"cn_a","windows":windows}]);
    input["usage_records"] = claude(json!([
        {"name":"seven_day_fable","usedPercent":2.0,"resetsAt":RESET,"observedAt":NOW},
        {"name":"seven_day","usedPercent":67.0,"resetsAt":RESET,"observedAt":NOW},
        {"name":"scoped_week","usedPercent":80.0,"resetsAt":RESET}
    ]));
    let view = settings_view(&input).unwrap();
    let row = &view["usage"][0];
    assert_eq!(row["engine"], "claude");
    assert_eq!(
        row["windows"],
        json!([
            {"name":"seven_day","usedPercent":67.0,"resetAt":"2026-11-17T02:59:00Z","observedAt":"2026-11-16T12:00:00Z"},
            {"name":"seven_day_fable","usedPercent":2.0,"resetAt":"2026-11-17T02:59:00Z","observedAt":"2026-11-16T12:00:00Z"}
        ])
    );
    input["usage_records"] = claude(json!([
        {"name":"seven_day","usedPercent":67.0,"resetsAt":RESET,"observedAt":NOW}
    ]));
    let view = settings_view(&input).unwrap();
    assert_eq!(view["usage"][0]["windows"].as_array().unwrap().len(), 1);
    input["usage_records"] = claude(json!([]));
    let view = settings_view(&input).unwrap();
    assert_eq!(view["usage"][0]["windows"], json!([]));
    assert!(view["usage"][0]["usedPercent"].is_null());
}

/// The reader turns a refreshed cache into dated windows; a later refresh replaces them.
#[test]
fn a_refreshed_cache_reaches_the_settings_read() {
    let f = Fixture::new();
    let home = f.0.join("home/.claude");
    fs::create_dir(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(f.0.join("config/cortex")).unwrap();
    fs::set_permissions(f.0.join("config/cortex"), fs::Permissions::from_mode(0o700)).unwrap();
    let registry = json!({"schema_version":1,"connections":{ID:{"id":ID,"name":"Claude","engine":"claude-agent-acp",
        "provider":"Fixture","auth_mode":"native_account","key_kind":null,"endpoint":null,"launch_command":null,
        "credential_ref":{"kind":"engine_native_dir","path":home,"owned":false},
        "approved_fingerprint":"untrusted","state":"configured","qualified":true,"created_at":"now","updated_at":"now"}}});
    let path = f.0.join("config/cortex/connections.json");
    fs::write(&path, serde_json::to_vec(&registry).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let reader = ConfigurationReader::new(RouteEnvironment {
        home: f.0.join("home"),
        config: f.0.join("config"),
        state: f.state(),
        routing_override: None,
    })
    .unwrap();
    assert_eq!(
        reader.usage_targets("claude-agent-acp").unwrap(),
        vec![(ID.to_owned(), home.clone())]
    );
    assert_eq!(reader.usage_targets("codex-acp").unwrap(), vec![]);
    let windows = |used: f64, at: f64| {
        refresh(&f.state(), ID, at, || Some(parsed(json!({"seven_day":{"used":used,"resets_at":RESET},"seven_day_fable":{"used":2.0,"resets_at":RESET}}))))
            .unwrap();
        reader.usage_cache(ID, "claude-agent-acp", at).unwrap()
    };
    let first = windows(54.0, NOW);
    let general = first
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "seven_day")
        .unwrap();
    assert_eq!(
        (
            general["usedPercent"].clone(),
            general["observedAt"].clone()
        ),
        (json!(54.0), json!(NOW))
    );
    let later = NOW + REFRESH_AFTER_SECONDS;
    let second = windows(67.0, later);
    let general = second
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "seven_day")
        .unwrap();
    assert_eq!(
        (
            general["usedPercent"].clone(),
            general["observedAt"].clone()
        ),
        (json!(67.0), json!(later))
    );
}
