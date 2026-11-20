//! Consent comes only from retained Core evidence, never coordinator reasons.
use super::{
    grammar::{self as g, re, Name},
    RouteResolver,
};
use cortex_core::routing::Evidence;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn plain(text: &str) -> String {
    let text = re(r"(?s)<chat-history-summary>.*?</chat-history-summary>").replace_all(text, "");
    let text=re(r"(?i)\bбез\s+(?:какой-?(?:либо|нибудь)\s+)?(?:замен\w*|подмен\w*)(?:\s+(?:на\s+)?(?:друг\w*|ин\w*)\s+модел\w*)?|\bwithout\s+(?:any\s+)?(?:substitut\w*|replac\w*)(?:\s+(?:by|with)\s+(?:another|other|a\s+different)\s+model)?|\bno\s+(?:substitut\w*|replacement)s?\b").replace_all(&text,"");
    let mut fence = false;
    text.lines()
        .map(|line| {
            if line.trim_start().starts_with("\u{60}\u{60}\u{60}")
                || line.trim_start().starts_with("~~~")
            {
                fence = !fence;
                return "";
            }
            if fence || line.trim_start().starts_with('>') {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace("**", "")
        .replace('’', "'")
}
fn covers(role: &str, text: &str) -> bool {
    // Qualified audits do not name the generic reviewer.
    let text=re(r"(?i)(security|accessibility|a11y)[\s-]+(?:audit\w*|аудит\w*)|(?:audit\w*|аудит\w*)\s+(?:of\s+)?(security|accessibility|безопасност\w*|доступност\w*)").replace_all(text," $1$2 ");
    let patterns = match role {
        "worker-code-reviewer" => r"review\w*|ревью|аудит\w*|audit\w*",
        "worker-security-auditor" => r"review\w*|ревью|аудит\w*|audit\w*|security|безопасност\w*",
        "worker-accessibility-auditor" => {
            r"review\w*|ревью|аудит\w*|audit\w*|accessibility|a11y|доступност\w*"
        }
        "worker-accessibility-fixer" => r"accessibility|a11y|доступност\w*",
        "worker-frontend-dev" => r"design\w*|дизайн\w*|ui|ux|верстк\w*|frontend|фронтенд\w*|фронт",
        "worker-ux-designer" => r"design\w*|дизайн\w*|ui|ux|верстк\w*",
        "worker-qa-engineer" | "worker-build-verification" => r"test\w*|тест\w*|qa",
        "worker-technical-writer" => r"docs?|documentation|документац\w*",
        "worker-backend-dev" => r"backend|бэкенд\w*|бэк",
        "worker-debugger" => r"debug\w*|отладк\w*",
        "worker-devops-engineer" => r"devops|deploy\w*|деплой\w*|ci",
        "worker-architect" | "worker-database-architect" => r"architect\w*|архитектур\w*",
        "worker-refactorer" => r"refactor\w*|рефактор\w*",
        "worker-planner" => r"plan\w*|планир\w*",
        _ => return false,
    };
    g::words(&text)
        .iter()
        .any(|w| re(&format!("(?i)^(?:{patterns})$")).is_match(w))
}
fn named_work(text: &str) -> bool {
    g::words(text).iter().any(|w| g::purpose_noun(w))
}
pub(super) fn normalize_effort(text: &str, names: &[Name]) -> String {
    let mut text = text.to_string();
    let spellings: Vec<_> = names
        .iter()
        .flat_map(|n| [n.label.as_str(), n.label.split_once('/').unwrap().1])
        .collect();
    // Preserve byte offsets so effort always binds to the same source mention.
    for spelling in &spellings {
        for cap in re(&format!(
            r#"(?P<model>{})(?P<effort>[:/](?:low|medium|high|xhigh|max|ultra))"#,
            regex::escape(spelling)
        ))
        .captures_iter(&text.clone())
        {
            let m = cap.name("model").unwrap();
            let e = cap.name("effort").unwrap();
            let before = &text[..m.start()];
            let tail = &text[e.end()..];
            if !before
                .chars()
                .next_back()
                .is_none_or(|c| c.is_whitespace() || "\"'\u{60}«“".contains(c))
            {
                continue;
            }
            if !tail
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || ".,!?;:\"'\u{60}»”".contains(c))
            {
                continue;
            }
            if spellings
                .iter()
                .any(|s| s.len() >= e.end() - m.start() && text[m.start()..].starts_with(*s))
            {
                continue;
            }
            let close = tail.chars().next().filter(|c| "\"'\u{60}»”".contains(*c));
            let replacement = if let Some(c) = close {
                format!("{c}{}", " ".repeat(e.len() - c.len_utf8() + c.len_utf8()))
            } else {
                " ".repeat(e.len())
            };
            let end = e.end() + close.map_or(0, char::len_utf8);
            text.replace_range(e.start()..end, &replacement);
        }
    }
    text
}
fn effort(source: &str, m: &g::Mention) -> Option<String> {
    let after = &source[m.end..];
    let levels = r"(?:low|medium|high|xhigh|max|ultra)";
    let suffix = format!(
        r#"(?i)^["'\u{{60}}»”]*(?:\s*[:/]\s*(?P<attached>{levels})(?:$|[^\w/-])|\s+(?:at|with|на|с)\s+(?P<qualified>{levels})(?:$|[^\w/-])|[,\s]+effort\s*[=:]\s*(?P<named>{levels})(?:$|[^\w/-])|[,\s]+(?P<described>{levels})\s+(?:effort|throughout)\b)"#
    );
    re(&suffix).captures(after).and_then(|c| {
        ["attached", "qualified", "named", "described"]
            .iter()
            .find_map(|name| c.name(name).map(|m| m.as_str().to_lowercase()))
    })
}
impl RouteResolver {
    fn consent_names(&self) -> Vec<Name> {
        let Some(models) = self.rules["models"].as_object() else {
            return Vec::new();
        };
        let enabled = |e: &str, m: &str| {
            self.rules["enabled"][e]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == m))
        };
        let derived = |m: &str| {
            m.split('[')
                .next()
                .unwrap_or(m)
                .split(['-', '_', '.', '/'])
                .filter(|w| !w.is_empty() && w.chars().all(char::is_alphabetic))
                .next_back()
                .map(str::to_lowercase)
        };
        let mut names = Vec::new();
        for (engine, config) in models {
            if let Some(tiers) = config["tiers"].as_object() {
                for model in tiers.keys() {
                    let mut aliases: Vec<String> = config["policy"][model]["request_names"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    if aliases.is_empty() {
                        if let Some(word) = derived(model).filter(|w| {
                            w.chars().count() >= 3
                                && ![
                                    "default", "auto", "latest", "flash", "pro", "mini", "plan",
                                    "preview", "chat",
                                ]
                                .contains(&w.as_str())
                        }) {
                            if !models.iter().any(|(e, c)| {
                                c["tiers"].as_object().is_some_and(|t| {
                                    t.keys().any(|m| {
                                        (e != engine || m != model)
                                            && enabled(e, m)
                                            && (derived(m).as_ref() == Some(&word)
                                                || c["policy"][m]["request_names"]
                                                    .as_array()
                                                    .is_some_and(|a| {
                                                        a.iter().any(|n| {
                                                            n.as_str().is_some_and(|n| {
                                                                n.eq_ignore_ascii_case(&word)
                                                            })
                                                        })
                                                    }))
                                    })
                                })
                            }) {
                                aliases.push(word);
                            }
                        }
                    }
                    let bare = enabled(engine, model)
                        && models
                            .iter()
                            .filter(|(e, c)| c["tiers"].get(model).is_some() && enabled(e, model))
                            .count()
                            == 1;
                    names.push(Name {
                        label: format!("{engine}/{model}"),
                        aliases,
                        bare,
                    });
                }
            }
        }
        names
    }
    /// (grant, changed-file diagnostic, revocation), all from retained evidence.
    pub(super) fn consent(
        &self,
        role: &str,
        _kind: &str,
        hint: &str,
        evidence: &Evidence,
    ) -> (Option<Value>, Option<Value>, Option<Value>) {
        let names = self.consent_names();
        let mut project = BTreeMap::new();
        let mut prohibited = BTreeSet::new();
        let mut changed = None;
        for file in &evidence.instructions {
            let text = normalize_effort(&plain(&file.text), &names);
            let mut file_grants = BTreeMap::new();
            let mut file_refused = BTreeSet::new();
            for (line, text) in text.lines().enumerate() {
                for sentence in g::sentences(text) {
                    let all = g::mentions(sentence, &names);
                    for m in &all {
                        let label = &names[m.model].label;
                        if g::refused(sentence, m, true) {
                            // Global prohibitions count; rules for another purpose do not.
                            if covers(role, sentence) || !named_work(sentence) {
                                file_refused.insert(label.clone());
                            }
                            continue;
                        }
                        if g::alternatives(sentence, m, &all) {
                            continue;
                        }
                        let assignment = g::assignment(sentence, m);
                        if all.iter().any(|other| {
                            other.model != m.model && other.start < m.end && m.start < other.end
                        }) {
                            continue;
                        }
                        let purpose = g::requested(sentence, m, &names, false)
                            && g::scopes(sentence, m, &all).iter().any(|s| covers(role, s))
                            || assignment.as_ref().is_some_and(|s| covers(role, s));
                        if purpose {
                            file_grants.entry(label.clone()).or_insert_with(||json!({"source":"project_instruction","path":file.path,"line":line+1,
                                "source_ref":format!("project_instruction:{}:{}",file.path,line+1),"family":label,"matched":m.text(sentence)}));
                        }
                    }
                }
            }
            if file.changed {
                if file_grants.len() == 1 {
                    let v = file_grants.values().next().unwrap();
                    changed = Some(
                        json!({"status":"changed_during_task","path":file.path,"line":v["line"],"route":v["family"]}),
                    );
                }
            } else {
                project.extend(file_grants);
                prohibited.extend(file_refused);
            }
        }
        let mut grant = if project.len() == 1 {
            project
                .into_iter()
                .next()
                .filter(|(k, _)| !prohibited.contains(k))
                .map(|(_, v)| v)
        } else {
            None
        };
        let mut revocation = None;
        for input in &evidence.inputs {
            // A lossy imported history invalidates older user grants, never project rules.
            if re(r"(?s)<chat-history-summary>.*?</chat-history-summary>").is_match(&input.text)
                && grant
                    .as_ref()
                    .is_some_and(|v| v["source"] != "project_instruction")
            {
                grant = None;
                revocation = None;
            }
            let source = plain(&input.text);
            let text = normalize_effort(&source, &names);
            let all = g::mentions(&text, &names);
            let refused: BTreeSet<_> = all
                .iter()
                .filter(|m| g::refused(&text, m, true))
                .map(|m| m.model)
                .collect();
            if let Some(old) = &grant {
                if names
                    .iter()
                    .enumerate()
                    .any(|(i, n)| old["family"] == n.label && refused.contains(&i))
                {
                    revocation = Some(
                        json!({"status":"revoked_by_user","reason":"revoked_by_user","family":old["family"],"source_ref":input.source_ref,"revoked_source_ref":old["source_ref"]}),
                    );
                    grant = None;
                }
            }
            // Refusals above see the entire answer. Only the selected label
            // may supply a positive route, purpose, or effort.
            let (source, text) = if let Some(label) = &input.option_with_comment {
                let source = plain(label);
                let text = normalize_effort(&source, &names);
                (source, text)
            } else {
                (source, text)
            };
            let mut purpose = BTreeMap::new();
            let mut direct = BTreeMap::new();
            for sentence in g::sentences(&text) {
                let local = g::mentions(sentence, &names);
                for m in &local {
                    if refused.contains(&m.model)
                        || local.iter().any(|other| {
                            other.model != m.model && other.start < m.end && m.start < other.end
                        })
                    {
                        continue;
                    }
                    let n = &names[m.model];
                    let selected = input.selected_option;
                    let offset = sentence.as_ptr() as usize - text.as_ptr() as usize;
                    let mut absolute = m.clone();
                    absolute.start += offset;
                    absolute.end += offset;
                    let asks = if selected {
                        g::selected(
                            &text,
                            &absolute,
                            &names,
                            input.question.as_deref().unwrap_or(""),
                        )
                    } else {
                        g::requested(&text, &absolute, &names, true)
                            || !g::alternatives(sentence, m, &local)
                                && g::assignment(sentence, m).is_some()
                    };
                    if !asks {
                        continue;
                    }
                    let own = if selected {
                        let mut work = sentence.to_string();
                        for n in local.iter().rev() {
                            if n.end <= work.len() {
                                work.replace_range(n.start..n.end, " ");
                            }
                        }
                        if g::names_work(&work) {
                            covers(role, &work)
                        } else {
                            g::question_scope(input.question.as_deref().unwrap_or(""), false)
                                .iter()
                                .any(|s| covers(role, s))
                        }
                    } else {
                        !g::alternatives(sentence, m, &local)
                            && (g::requested(sentence, m, &names, false)
                                && g::scopes(sentence, m, &local)
                                    .iter()
                                    .any(|s| covers(role, s))
                                || g::assignment(sentence, m)
                                    .as_ref()
                                    .is_some_and(|s| covers(role, s)))
                    };
                    if !own && hint != n.label {
                        continue;
                    }
                    let mut value = json!({"source":"user_request","source_ref":input.source_ref,"family":n.label,"matched":m.text(sentence)});
                    // Locate this mention in the complete normalized source for effort.
                    if let Some(e) = effort(&source, &absolute) {
                        value["effort"] = json!(e);
                    }
                    let target = if own { &mut purpose } else { &mut direct };
                    if let Some(old) = target.get(&n.label) {
                        let old: &Value = old;
                        if old["effort_ambiguous"] == true
                            || !old["effort"].is_null()
                                && !value["effort"].is_null()
                                && old["effort"] != value["effort"]
                        {
                            value["effort_ambiguous"] = json!(true);
                        } else if value["effort"].is_null() {
                            value["effort"] = old["effort"].clone();
                        }
                    }
                    target
                        .entry(n.label.clone())
                        .and_modify(|old| *old = value.clone())
                        .or_insert(value);
                }
            }
            let exact = purpose.remove(hint).or_else(|| direct.remove(hint));
            let current = if !purpose.is_empty() { purpose } else { direct };
            if let Some(exact) = exact {
                grant = Some(exact);
                revocation = None;
            } else if current.len() == 1 {
                grant = current.into_values().next();
                revocation = None;
            }
        }
        (grant, changed, revocation)
    }
}
