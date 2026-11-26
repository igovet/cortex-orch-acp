//! Python `acp.commands` recognition and reply text for the local commands
//! this host implements. A command never dispatches an engine or a task.
use serde_json::Value;

/// Python `_TRIGGERS`, in recognition priority order.
const TRIGGERS: [(&str, &str); 14] = [
    ("/cortex-clear", "cortex-clear"),
    ("/cortex-status", "cortex-status"),
    ("/cortex-new-task", "cortex-new-task"),
    ("/cortex-language", "cortex-language"),
    ("/skill-management", "skill-management"),
    ("/harvest-refresh", "harvest-refresh"),
    ("/harvest", "harvest"),
    ("/cortex-model", "cortex-model"),
    ("/cortex-effort", "cortex-effort"),
    ("/cortex-setup", "cortex-setup"),
    ("/cortex-stop", "cortex-stop"),
    ("/cortex-kill", "cortex-kill"),
    ("/cortex-finish", "cortex-finish"),
    ("/cortex-answer", "cortex-answer"),
];

/// Python `strip()`/`isspace()` whitespace, not only ASCII.
fn python_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}
/// Python `recognize`: exactly one text block whose trimmed text is a trigger,
/// alone or followed by a space, tab or newline. Anything else is a prompt.
pub(crate) fn recognize(blocks: &Value) -> Option<(&'static str, String)> {
    let [block] = blocks.as_array()?.as_slice() else {
        return None;
    };
    if block["type"] != "text" {
        return None;
    }
    let stripped = block["text"].as_str()?.trim_matches(python_space);
    for (trigger, id) in TRIGGERS {
        if stripped == trigger {
            return Some((id, String::new()));
        }
        if let Some(rest) = stripped.strip_prefix(trigger) {
            if rest.starts_with([' ', '\t', '\n']) {
                return Some((id, rest.trim_matches(python_space).to_owned()));
            }
        }
    }
    None
}
/// Python `resolve_language_argument`.
pub(crate) fn language(argument: &str) -> Option<String> {
    let value = argument.trim_matches(python_space).to_lowercase();
    matches!(value.as_str(), "auto" | "en" | "ru").then_some(value)
}
/// Python `resolve_model_argument`: an exact route id, else one case-insensitive
/// label match, else one engine model id. Never a best-effort guess.
pub(crate) fn model(argument: &str, entries: &[Value]) -> Option<String> {
    let argument = argument.trim_matches(python_space);
    if argument.is_empty() {
        return None;
    }
    if let Some(e) = entries.iter().find(|e| e["route"] == argument) {
        return e["route"].as_str().map(str::to_owned);
    }
    let lowered = argument.to_lowercase();
    let lower = |v: &Value| v.as_str().map(str::to_lowercase);
    let exact: Vec<_> = entries
        .iter()
        .filter(|e| {
            lower(&e["disambiguated_label"]).as_deref() == Some(&lowered)
                || lower(&e["label"]).as_deref() == Some(&lowered)
        })
        .collect();
    if exact.len() == 1 {
        return exact[0]["route"].as_str().map(str::to_owned);
    }
    let models: Vec<_> = entries
        .iter()
        .filter(|e| lower(&e["model"]).as_deref() == Some(&lowered))
        .collect();
    (models.len() == 1)
        .then(|| models[0]["route"].as_str().map(str::to_owned))
        .flatten()
}
fn count(states: &[String], state: &str) -> usize {
    states.iter().filter(|s| *s == state).count()
}
/// Python `_connection_setup_summary`.
pub(crate) fn connection_summary(states: &[String], ru: bool) -> String {
    if states.is_empty() {
        return (if ru { "нет зарегистрированных подключений; см. /cortex-setup" } else { "no connection registered yet; see /cortex-setup" }).into();
    }
    let configured = count(states, "configured");
    let total = states.len();
    let mut details = vec![if ru {
        format!("настроено подключений: {configured}/{total}")
    } else {
        format!("{configured}/{total} connection(s) configured")
    }];
    for (state, text) in [
        ("authorization_required", if ru { "ожидает авторизации" } else { "awaiting authorization" }),
        ("unavailable", if ru { "недоступно" } else { "unavailable" }),
        ("invalid", if ru { "с ошибками конфигурации" } else { "invalid" }),
    ] {
        let n = count(states, state);
        if n > 0 {
            details.push(format!("{n} {text}"));
        }
    }
    details.join("; ")
}
/// Python `build_status_text`. The execution line states this host truthfully:
/// it does not run Python's engine jail. A Russian reply renders built-in option
/// prose and choice names in Russian; IDs, route labels and model names stay verbatim.
pub(crate) fn status(options: &Value, summary: &str, ru: bool) -> String {
    let mut lines = vec![(if ru { "Настройки Cortex:" } else { "Cortex configuration status:" }).to_owned()];
    for option in options.as_array().into_iter().flatten() {
        let id = option["id"].as_str().unwrap_or_default();
        let local = |english: &str| -> String {
            ru.then(|| workspace_service::incoming::russian_option_text(id, english))
                .flatten()
                .unwrap_or(english)
                .to_owned()
        };
        let current = &option["currentValue"];
        let label = option["options"]
            .as_array()
            .and_then(|a| a.iter().find(|c| c["value"] == *current))
            .map(|c| c["name"].clone())
            .unwrap_or_else(|| current.clone());
        let text = |v: &Value| {
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string())
        };
        let note = option["description"].as_str().filter(|s| !s.is_empty());
        lines.push(format!(
            "- {}: {}{}",
            text(&option["id"]),
            label.as_str().map(local).unwrap_or_else(|| text(&label)),
            note.map(|n| format!(" {}", local(n))).unwrap_or_default()
        ));
    }
    let connections = if ru { "подключения" } else { "connections" };
    lines.push(format!("- {connections}: {summary}"));
    lines.push((if ru { "- выполнение: сервер на Rust; процессы движков не изолированы от выполнения недоверенного кода" } else { "- execution: Rust native host; engine processes are not jailed (not hostile-code isolation)" }).into());
    lines.join("\n")
}
const SETUP_GUIDANCE: &str = "No model is selectable yet for this project. To connect an engine:\n1. Install the pinned engine adapter with `cortex-paseo-tools install-engine --engine ENGINE`, then register its native account with `cortex-paseo-tools add-connection --engine ENGINE`.\n2. Complete the engine's native login in the isolated directory shown by setup.\n3. Run `cortex-paseo-tools validate-connection --connection ID` using the ID returned by setup. Once it reports 'configured', start a new Cortex session to refresh models.\nThis message never contains a secret and never mutates any connection.";
/// Python `build_setup_text`.
pub(crate) fn setup(states: &[String], ru: bool) -> &'static str {
    if count(states, "authorization_required") > 0 {
        return if ru { "Подключение уже зарегистрировано, но ждёт авторизации.\n1. Войдите в учётную запись средствами движка в изолированном каталоге, указанном при настройке.\n2. Запускайте `cortex-paseo-tools validate-connection --connection ID` с текущим ID, пока команда не выдаст 'configured'.\n3. После настройки модели появятся в списке выбора и в /cortex-model.\nНе регистрируйте повторно тот же движок с той же учётной записью. Это сообщение не раскрывает секреты и не меняет подключения." } else { "A connection is already registered but is awaiting authorization.\n1. Complete that engine's native login in the isolated engine directory shown during connection setup.\n2. Run `cortex-paseo-tools validate-connection --connection ID` using its existing ID until it reports 'configured'.\n3. Once configured, its models will start appearing in the model selector (or /cortex-model).\nDo not register another connection for the same engine and account. This message never contains a secret and never mutates any connection." };
    }
    if count(states, "unavailable") > 0 {
        return if ru { "Зарегистрированное подключение сейчас недоступно.\nПроверьте, установлен ли движок и доступен ли он, затем выполните `cortex-paseo-tools validate-connection --connection ID`. Если подключение больше нельзя использовать, исправьте или удалите его перед добавлением нового.\nЭто сообщение не раскрывает секреты и не меняет подключения." } else { "A registered connection is currently unavailable.\nCheck that the configured engine is installed and reachable, then run `cortex-paseo-tools validate-connection --connection ID`. If the connection is no longer usable, repair or remove that registration before adding a replacement.\nThis message never contains a secret and never mutates any connection." };
    }
    if count(states, "invalid") > 0 {
        return if ru { "В настройках зарегистрированного подключения есть ошибка.\nПроверьте настройки движка, провайдера и авторизации. Исправьте существующее подключение, затем повторите `cortex-paseo-tools validate-connection --connection ID`.\nЭто сообщение не раскрывает секреты и не меняет подключения." } else { "A registered connection has invalid configuration.\nReview its engine, provider, and authentication settings, repair the existing registration, then run `cortex-paseo-tools validate-connection --connection ID` again.\nThis message never contains a secret and never mutates any connection." };
    }
    if ru { "Для этого проекта пока нет доступных моделей. Чтобы подключить движок:\n1. Установите закреплённую версию адаптера командой `cortex-paseo-tools install-engine --engine ENGINE`, затем зарегистрируйте учётную запись движка командой `cortex-paseo-tools add-connection --engine ENGINE`.\n2. Войдите в учётную запись средствами движка в изолированном каталоге, указанном при настройке.\n3. Выполните `cortex-paseo-tools validate-connection --connection ID` с полученным ID. Когда команда выдаст 'configured', начните новый сеанс Cortex, чтобы обновить список моделей.\nЭто сообщение не раскрывает секреты и не меняет подключения." } else { SETUP_GUIDANCE }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn owner_status_and_setup_use_the_requested_language() {
        let states = ["configured".into(), "authorization_required".into()];
        for (ru, title, summary, setup_title) in [
            (false, "Cortex configuration status:", "1/2 connection(s) configured; 1 awaiting authorization", "A connection is already registered"),
            (true, "Настройки Cortex:", "настроено подключений: 1/2; 1 ожидает авторизации", "Подключение уже зарегистрировано"),
        ] {
            assert_eq!(connection_summary(&states, ru), summary);
            let status = status(&json!([{"id":"model","currentValue":"route-1"}]), summary, ru);
            assert!(status.starts_with(title));
            assert!(status.contains("- model: route-1"));
            assert!(status.contains(summary));
            let setup = setup(&states, ru);
            assert!(setup.starts_with(setup_title));
            assert!(setup.contains("`cortex-paseo-tools validate-connection --connection ID`"));
        }
    }
    /// The status options a real session presents: the host's own language and
    /// access options, plus route rows carrying the route-state notes.
    fn presented(mode: &str) -> Value {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut options = runtime
            .block_on(workspace_service::incoming::presentation(
                std::sync::Arc::new(workspace_service::native_runtime::UnqualifiedRoutes),
                std::env::temp_dir(),
                json!({"language":"ru","mode":mode}),
            ))
            .unwrap();
        let routes = json!([
            {"id":"model","name":"Model","category":"model","type":"select","currentValue":"fixture-model",
                "description":"(pending — applies to the next run)",
                "options":[{"value":"fixture-model","name":"Fixture Model","description":"codex"}]},
            {"id":"thought_level","name":"Thought level","category":"thought_level","type":"select",
                "currentValue":"high","description":"Depends on an unavailable route — choose a route",
                "options":[{"value":"high","name":"high"}]}]);
        let mut all = routes.as_array().unwrap().clone();
        all.append(options.as_array_mut().unwrap());
        Value::Array(all)
    }
    #[test]
    fn russian_status_localizes_built_in_option_prose() {
        for mode in cortex_core::access::MODES {
            let options = presented(mode);
            let access = options.as_array().unwrap().iter().find(|o| o["id"] == "access_mode").unwrap();
            let (en_name, en_description) = workspace_service::incoming::access_label(mode);
            assert_eq!(access["description"], en_description, "config options stay English");
            let english = status(&options, "s", false);
            assert!(english.contains("- language: Russian Language for Cortex's own status/question/stop lines; auto detects from your messages."), "{english}");
            assert!(english.contains(&format!("- access_mode: {en_name} {en_description}")), "{english}");
            assert!(english.contains("- model: Fixture Model (pending — applies to the next run)"), "{english}");
            assert!(english.contains("- thought_level: high Depends on an unavailable route — choose a route"), "{english}");
            let russian = status(&options, "s", true);
            assert!(russian.contains("- language: Русский Язык собственных строк Cortex"), "{russian}");
            assert!(russian.contains("- model: Fixture Model (ожидает — применится при следующем запуске)"), "{russian}");
            assert!(russian.contains("- thought_level: high Зависит от недоступного маршрута"), "{russian}");
            let (ru_name, ru_opening) = match mode {
                "development" => ("Режим разработки", "Работники могут работать в любом месте этого компьютера."),
                "auto" => ("Автоматический режим", "Работники могут работать в любом месте этого компьютера, и ничто"),
                _ => ("Защищённый режим", "Работники пишут только внутри проекта."),
            };
            assert!(russian.contains(&format!("- access_mode: {ru_name} {ru_opening}")), "{russian}");
            // No built-in English sentence or choice name survives in the Russian reply.
            for english_text in [
                en_name,
                en_description,
                "Russian",
                "Language for Cortex",
                "(pending",
                "Depends on an unavailable route",
            ] {
                assert!(!russian.contains(english_text), "{english_text}: {russian}");
            }
        }
    }
    #[test]
    fn recognition_matches_python_shapes() {
        let one = |t: &str| json!([{"type":"text","text":t}]);
        assert_eq!(
            recognize(&one(" /cortex-status ")),
            Some(("cortex-status", String::new()))
        );
        assert_eq!(
            recognize(&one("/cortex-model\tfoo bar ")),
            Some(("cortex-model", "foo bar".into()))
        );
        assert_eq!(
            recognize(&one("/harvest-refresh x")),
            Some(("harvest-refresh", "x".into()))
        );
        assert_eq!(recognize(&one("/cortex-modelx")), None);
        assert_eq!(recognize(&one("/cortex-foo")), None);
        assert_eq!(
            recognize(&json!([{"type":"text","text":"/cortex-status"},{"type":"text","text":"x"}])),
            None
        );
        assert_eq!(language(" RU "), Some("ru".into()));
        assert_eq!(language("de"), None);
    }
    #[test]
    fn model_resolution_never_guesses() {
        let entries = vec![
            json!({"route":"xr_a","model":"m","label":"claude · a · m","disambiguated_label":"claude · a · m"}),
            json!({"route":"xr_b","model":"m","label":"codex · b · m","disambiguated_label":"codex · b · m"}),
            json!({"route":"xr_c","model":"Other","label":"x","disambiguated_label":"x"}),
        ];
        assert_eq!(model("xr_b", &entries).as_deref(), Some("xr_b"));
        assert_eq!(model("CODEX · B · M", &entries).as_deref(), Some("xr_b"));
        assert_eq!(model("m", &entries), None);
        assert_eq!(model("other", &entries).as_deref(), Some("xr_c"));
        assert_eq!(model("", &entries), None);
        assert_eq!(
            connection_summary(&[], false),
            "no connection registered yet; see /cortex-setup"
        );
        assert_eq!(
            connection_summary(&["configured".into(), "invalid".into()], false),
            "1/2 connection(s) configured; 1 invalid"
        );
    }
}
