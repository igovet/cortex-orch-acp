//! Bounded EN/RU routing grammar, ported from the retained routing-consent tests.
//! This module interprets text only; provenance is supplied by Core, not by this parser.
use super::grammar_data::*;
use regex::Regex;
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

pub(super) fn re(pattern: &str) -> Regex {
    static CACHE: OnceLock<Mutex<BTreeMap<String, Regex>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    if cache.len() >= 512 && !cache.contains_key(pattern) {
        cache.clear();
    }
    cache
        .entry(pattern.into())
        .or_insert_with(|| Regex::new(pattern).expect("routing grammar"))
        .clone()
}
fn full(pattern: &str, text: &str) -> bool {
    re(&format!("(?i)^(?:{pattern})$")).is_match(text)
}
fn one(word: &str, choices: &str) -> bool {
    choices.split('|').any(|s| s == word)
}
pub(super) fn words(text: &str) -> Vec<String> {
    re(r"[\p{L}\p{N}_']+")
        .find_iter(text)
        .map(|m| m.as_str().trim_matches('\'').to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}
fn cyr(word: &str) -> bool {
    re("[а-яё]").is_match(word)
}
const LEAD: &str = "please|pls|plz|kindly|just|simply|only|always|also|then|and|now|so|ok|okay|yes|no|to|can|could|would|will|should|must|may|let's|lets|better";
const FILLERS: &str = "the|a|an|model|модели|моделью";
const VENDORS: &str = "claude|клод|anthropic|openai|codex|gpt";
const EXCLUSIVE: &str = "only|exclusively|solely|только|исключительно|лишь";
const PURPOSE: &str = "for|для";
const TAIL: &str = "please|pls|пожалуйста";
const AFFIRM: &str = "да|yes|ok|okay|ок";
const NOTES: &str =
    "recommended|default|рекомендуется|рекомендую|рекомендуемый|рекомендуемая|рекомендуемое";
const EX_MEANS: &str = "on|via|through|using|with|на|через|посредством|силами|помощью|с|со";
const BREAKS: &str = "but|then|however|yet|because|since|if|when|while|although|though|unless|until|so|а|но|затем|потом|однако|зато|потому|поскольку|если|когда|пока|хотя|чтобы|чтоб";
const INVERTED: &str = "do|does|did|must|should|shall|has|have|need|needs";
const QUESTION_WORDS: &str = "why|whether|почему|разве|ли|зачем|неужели";
const REQUEST_BREAK: &str = r#"[,()"«»“”„—–]"#;
const CLAUSE_BREAK: &str = r#"[,:()"«»“”„—–]"#;

pub(super) fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        let next = text[i + c.len_utf8()..].chars().next();
        let prev = text[..i].chars().next_back();
        if "!?;\n".contains(c)
            || c == '.'
                && (prev.is_none_or(char::is_whitespace)
                    || next.is_none_or(|n| n.is_whitespace() || "\"'\u{60}»”)]*_~".contains(n)))
        {
            out.push(&text[start..i + c.len_utf8()]);
            start = i + c.len_utf8();
        }
    }
    out.push(&text[start..]);
    out
}
fn clause_words(text: &str) -> Vec<(String, Vec<String>)> {
    re(CLAUSE_BREAK)
        .split(text)
        .flat_map(|c| {
            let w = words(c);
            w.iter()
                .enumerate()
                .map(|(i, s)| (s.clone(), w[..i].to_vec()))
                .collect::<Vec<_>>()
        })
        .collect()
}
fn request_form(pattern: &str, word: &str, lead: &[String], subjunctive: bool) -> bool {
    if subjunctive {
        let past = if pattern == REQUEST_WORDS {
            RU_SUBJUNCTIVE_REQUEST
        } else {
            RU_SUBJUNCTIVE_WORK
        };
        if full(past, word)
            && lead.iter().enumerate().any(|(i, w)| {
                (one(w, "чтобы|чтоб") || w == "бы" && i > 0 && lead[i - 1] == "что")
                    && !lead[i + 1..].iter().any(|w| full(NEGATION_WORDS, w))
            })
        {
            return true;
        }
    }
    if !full(pattern, word) {
        return false;
    }
    let Some(previous) = lead.last() else {
        return true;
    };
    if cyr(word) {
        return !full(RU_PAST, previous);
    }
    if one(word, "requirement|requires") {
        return true;
    }
    if previous == "to" {
        return lead.len()<2 || !one(&lead[lead.len()-2],"tried|wanted|used|failed|managed|attempted|meant|supposed|was|were|had|did|went|planned|started|began|continued|seemed");
    }
    if one(previous, LEAD) || previous.ends_with("'ll") || previous.ends_with("'d") {
        return true;
    }
    if one(previous, "i|we") {
        return one(word, "prefer|pick|choose|select|request|require");
    }
    previous == "you"
        && lead.len() >= 2
        && one(&lead[lead.len() - 2], "can|could|would|will|please")
}
fn segment_break(w: &[String], i: usize) -> bool {
    one(&w[i], BREAKS)
        || one(&w[i], "and|or|plus|и|или")
            && w.get(i + 1).is_some_and(|n| {
                one(n, LEAD)
                    || one(n, "просто|только|также|тоже|ещё|еще")
                    || [REFUSAL_WORDS, REQUEST_WORDS, WORK_VERBS, NEGATION_WORDS]
                        .iter()
                        .any(|p| full(p, n))
            })
}
struct Context {
    sentence: Vec<(String, Vec<String>)>,
    clause: Vec<(String, Vec<String>)>,
    before: Vec<String>,
    after: Vec<String>,
    alone: bool,
}
fn context(text: &str, m: &Mention) -> Context {
    let raw = &text[..m.start];
    let cut = raw
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let before = &raw[..cut];
    let before = sentences(before).last().copied().unwrap_or("");
    let lead = words(re(CLAUSE_BREAK).split(before).last().unwrap_or(""));
    let raw = text[m.end..].trim_start_matches(['"', '\'', '\u{60}', '»', '”']);
    let end = sentences(raw).first().copied().unwrap_or("");
    let follow = words(re(CLAUSE_BREAK).split(end).next().unwrap_or(""));
    let start = (0..lead.len())
        .filter(|i| segment_break(&lead, *i))
        .map(|i| i + 1)
        .next_back()
        .unwrap_or(0);
    let end = (0..follow.len())
        .find(|i| segment_break(&follow, *i))
        .unwrap_or(follow.len());
    let sentence = clause_words(before);
    let n = words(re(REQUEST_BREAK).split(before).last().unwrap_or("")).len();
    Context {
        clause: sentence[sentence.len().saturating_sub(n)..].to_vec(),
        sentence,
        before: lead[start..].to_vec(),
        after: follow[..end].to_vec(),
        alone: lead.is_empty() && follow.is_empty(),
    }
}
pub(super) fn refused(text: &str, m: &Mention, strict: bool) -> bool {
    let c = context(text, m);
    if c.before
        .iter()
        .any(|w| full(REFUSAL_WORDS, w) || full(SUBSTITUTION_WORDS, w))
        || c.after.iter().any(|w| full(REFUSAL_WORDS, w))
    {
        return true;
    }
    let w: Vec<_> = c.sentence.iter().map(|(w, _)| w).collect();
    if let Some(i) = w
        .iter()
        .enumerate()
        .filter(|(i, w)| {
            one(w, "чтобы|чтоб") || w.as_str() == "бы" && *i > 0 && c.sentence[*i - 1].0 == "что"
        })
        .map(|(i, _)| i)
        .next_back()
    {
        if w[..i].iter().any(|w| full(NEGATION_WORDS, w)) {
            return true;
        }
    }
    w.iter().rev().take(5).any(|w| {
        if strict {
            full(REFUSAL_WORDS, w) || full(SUBSTITUTION_WORDS, w)
        } else {
            full(NEGATION_WORDS, w)
        }
    })
}
#[derive(Clone, Debug)]
pub(super) struct Name {
    pub label: String,
    pub aliases: Vec<String>,
    pub bare: bool,
}
#[derive(Clone, Debug)]
pub(super) struct Mention {
    pub start: usize,
    pub end: usize,
    pub model: usize,
    pub exact: bool,
}
impl Mention {
    pub fn text<'a>(&self, text: &'a str) -> &'a str {
        &text[self.start..self.end]
    }
}
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
pub(super) fn mentions(text: &str, names: &[Name]) -> Vec<Mention> {
    let mut result = Vec::new();
    let spellings: Vec<_> = names
        .iter()
        .flat_map(|n| {
            [
                n.label.as_str(),
                n.label.split_once('/').map_or("", |(_, m)| m),
            ]
        })
        .collect();
    for (i, n) in names.iter().enumerate() {
        for token in std::iter::once(n.label.as_str())
            .chain(n.bare.then(|| n.label.split_once('/').unwrap().1))
        {
            for (start, _) in text.match_indices(token) {
                let end = start + token.len();
                let before = &text[..start];
                let after = &text[end..];
                let isolated = before.is_empty()
                    || before.chars().next_back().is_some_and(char::is_whitespace);
                let whole =
                    after.is_empty() || after.chars().next().is_some_and(char::is_whitespace);
                let punct = after.chars().next().is_some_and(|c| ".,!?;:".contains(c))
                    && after[1..].chars().next().is_none_or(char::is_whitespace);
                let longer = spellings.iter().any(|s| {
                    s.len() > token.len()
                        && s.starts_with(token)
                        && text[start..].starts_with(s)
                        && text[start + s.len()..]
                            .chars()
                            .next()
                            .is_none_or(char::is_whitespace)
                });
                let quoted = [
                    ('"', '"'),
                    ('\'', '\''),
                    ('\u{60}', '\u{60}'),
                    ('«', '»'),
                    ('“', '”'),
                    ('(', ')'),
                ]
                .iter()
                .any(|(o, c)| {
                    before.ends_with(*o)
                        && before[..before.len() - o.len_utf8()]
                            .chars()
                            .next_back()
                            .is_none_or(char::is_whitespace)
                        && after.starts_with(*c)
                        && {
                            let tail = &after[c.len_utf8()..];
                            tail.chars().next().is_none_or(char::is_whitespace)
                                || tail.chars().next().is_some_and(|c| ".,!?;:".contains(c))
                                    && tail[1..].chars().next().is_none_or(char::is_whitespace)
                        }
                });
                if isolated && (whole || punct && !longer) || quoted {
                    result.push(Mention {
                        start,
                        end,
                        model: i,
                        exact: true,
                    });
                }
            }
        }
        for alias in &n.aliases {
            let suffix = if cyr(alias) {
                r"(?:а|ы|е|у|ой|ою|и|ом|ам|ами|ах|ов)?"
            } else {
                "s?"
            };
            for m in re(&format!("(?i){}{suffix}", regex::escape(alias))).find_iter(text) {
                if text[..m.start()]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !word_char(c))
                    && text[m.end()..].chars().next().is_none_or(|c| !word_char(c))
                {
                    // Derived labels cannot authorize a substring of an opaque identity.
                    if text[..m.start()]
                        .chars()
                        .next_back()
                        .is_some_and(|c| "./-".contains(c))
                        || text[m.end()..]
                            .chars()
                            .next()
                            .is_some_and(|c| "/-[".contains(c))
                    {
                        continue;
                    }
                    result.push(Mention {
                        start: m.start(),
                        end: m.end(),
                        model: i,
                        exact: false,
                    });
                }
            }
        }
    }
    result.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end), !m.exact));
    result.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.model == b.model);
    result
}
fn role_noun(w: &str) -> bool {
    full(ROLE_OR_STAGE, w)
        && !(cyr(w)
            && (full(WORK_VERBS, w)
                || full(REQUEST_WORDS, w)
                || full(RU_PAST, w)
                || full(RU_VERB_FORM, w) && !full(RU_VERB_LOOKALIKE, w)))
}
pub(super) fn purpose_noun(w: &str) -> bool {
    role_noun(w) || one(w, "ui|ux|writing|testing|planning|operations|analysis")
}
pub(super) fn names_work(text: &str) -> bool {
    words(text)
        .iter()
        .any(|w| purpose_noun(w) || full(WORK_VERBS, w) && !full(GENERIC_WORK, w))
}
fn exclusive(m: &Mention, c: &Context, names: &[Name]) -> bool {
    let w: Vec<_> = c.clause.iter().map(|(w, _)| w.as_str()).collect();
    let mut p = w.len();
    for _ in 0..2 {
        if p > 0
            && (one(w[p - 1], VENDORS)
                || one(w[p - 1], FILLERS)
                || names[m.model].aliases.iter().any(|a| {
                    full(
                        &format!("{}(?:а|ы|е|у|ой|и|ом)?", regex::escape(a)),
                        w[p - 1],
                    )
                }))
        {
            p -= 1;
        }
    }
    let means = p > 0 && one(w[p - 1], EX_MEANS);
    if means {
        p -= 1;
    }
    let mut after = c.after.as_slice();
    let lead = if p > 0 && one(w[p - 1], EXCLUSIVE) {
        &c.clause[..p - 1]
    } else if after
        .first()
        .is_some_and(|w| one(w, "only|exclusively|solely"))
    {
        after = &after[1..];
        &c.clause[..p]
    } else {
        return false;
    };
    if after.first().is_some_and(|w| !one(w, PURPOSE)) && !after.iter().all(|w| one(w, TAIL)) {
        return false;
    }
    if lead.is_empty() {
        return true;
    }
    if !means
        && lead.len() <= 3
        && (cyr(&lead[0].0) || one(&lead[0].0, "for|для|the|a|an"))
        && lead
            .iter()
            .all(|(w, _)| one(w, FILLERS) || one(w, PURPOSE) || role_noun(w))
    {
        return true;
    }
    let recent = &lead[lead.len().saturating_sub(6)..];
    recent.iter().enumerate().any(|(i, (w, l))| {
        request_form(REQUEST_WORDS, w, l, false)
            || request_form(WORK_VERBS, w, l, false) && (means || i + 1 < recent.len())
    })
}
fn requirement_question(text: &str, c: &Context) -> bool {
    let w: Vec<_> = c.clause.iter().map(|(w, _)| w.as_str()).collect();
    if !w.iter().enumerate().any(|(i, w)| {
        full(REQUIREMENT_MODALS, w)
            || *w == "to" && i > 0 && one(c.clause[i - 1].0.as_str(), "have|has|had")
    }) {
        return false;
    }
    text.trim_end().ends_with('?')
        || w.iter().any(|w| one(w, QUESTION_WORDS))
        || w.len() >= 2
            && one(w[0], INVERTED)
            && !one(w[1], "to|always|only|exclusively|solely|use")
}
pub(super) fn requested(text: &str, m: &Mention, names: &[Name], exact: bool) -> bool {
    let c = context(text, m);
    if refused(text, m, false) || requirement_question(text, &c) {
        return false;
    }
    if exclusive(m, &c, names) {
        return true;
    }
    let window = &c.clause[c.clause.len().saturating_sub(4)..];
    let subj = !c.sentence.iter().any(|(w, _)| full(NEGATION_WORDS, w));
    let mut p = window.len();
    if p > 0 && one(&window[p - 1].0, FILLERS) {
        p -= 1;
    }
    let means = p > 0
        && full(MEANS_WORDS, &window[p - 1].0)
        && window[..p - 1]
            .iter()
            .any(|(w, l)| request_form(WORK_VERBS, w, l, subj));
    let message_request = sentences(text).iter().any(|s| {
        clause_words(s).iter().any(|(w, l)| {
            request_form(REQUEST_WORDS, w, l, false)
                && !l.iter().rev().take(5).any(|w| full(NEGATION_WORDS, w))
        })
    });
    window
        .iter()
        .any(|(w, l)| request_form(REQUEST_WORDS, w, l, subj))
        || means
        || exact
            && m.exact
            && (message_request && c.alone
                || window.last().is_some_and(|(w, _)| one(w, "на|on|via|with")))
}
pub(super) fn assignment(text: &str, m: &Mention) -> Option<String> {
    if refused(text, m, false) {
        return None;
    }
    let prefix = &text[..m.start];
    let prefix = sentences(prefix).last().copied().unwrap_or("");
    let tail = &text[m.end..];
    let tail = sentences(tail).first().copied().unwrap_or("");
    let before = words(re(REQUEST_BREAK).split(prefix).last().unwrap_or(""));
    let after = words(re(REQUEST_BREAK).split(tail).next().unwrap_or(""));
    if before
        .iter()
        .chain(&after)
        .any(|w| one(w, "was|were|previously|last|раньше|ранее|вчера|прошлый"))
    {
        return None;
    }
    let lead = before.join(" ");
    let follow = after.join(" ");
    let object = format!(r"(?P<subject>(?:\w+\s+){{0,2}}{ROLE_OR_STAGE})\b");
    let patterns = [
        (
            format!(r"\bза\s+{object}\s+отвеча(?:ет|ют|ть|й|йте)\s*(?:claude\s*)?$"),
            true,
        ),
        (format!(r"^отвеча(?:ет|ют|ть|й|йте)\s+за\s+{object}"), false),
        (
            format!(r"^(?:(?:is|be)\s+)?(?:responsible\s+for|in\s+charge\s+of)\s+{object}"),
            false,
        ),
    ];
    for (p, b) in patterns {
        if let Some(c) = re(&format!("(?i){p}")).captures(if b { &lead } else { &follow }) {
            return Some(c["subject"].into());
        }
    }
    if before
        .iter()
        .any(|w| one(w, "чтобы|пусть|should|must|will|let"))
        || before.windows(2).any(|w| w[0] == "что" && w[1] == "бы")
    {
        for (p, b) in [
            (
                format!(r"\bза\s+{object}\s+отвечал[аи]?\s*(?:claude\s*)?$"),
                true,
            ),
            (format!(r"^отвечал[аи]?\s+за\s+{object}"), false),
        ] {
            if let Some(c) = re(&format!("(?i){p}")).captures(if b { &lead } else { &follow }) {
                return Some(c["subject"].into());
            }
        }
    }
    if re(r"(?i)\blet\s*(?:claude\s*)?$").is_match(&lead)
        || re(r"(?i)^(should|must)\s+").is_match(&follow)
    {
        if let Some(c) = re(&format!(
            r"(?i)^(?:(?:should|must)\s+)?(?:handle|perform)\s+{object}"
        ))
        .captures(&follow)
        {
            return Some(c["subject"].into());
        }
    }
    for (requirement, n) in [(PASSIVE_REQUIREMENT, 4), (RU_REQUIREMENT, 3)] {
        let p = format!(
            r"(?i)\b(?P<subject>{ROLE_OR_STAGE}(?:\s+\w+){{0,{n}}}?)\s+{requirement}\s*(?:claude\s*)?$"
        );
        if let Some(c) = re(&p).captures(&lead) {
            let prefix = words(&lead[..c.get(0).unwrap().start()]);
            if !text.trim_end().ends_with('?')
                && !before.iter().any(|w| one(w, QUESTION_WORDS))
                && !prefix.first().is_some_and(|w| one(w, INVERTED))
            {
                return Some(c["subject"].into());
            }
        }
    }
    None
}
pub(super) fn alternatives(text: &str, m: &Mention, all: &[Mention]) -> bool {
    all.iter().filter(|n|n.model!=m.model).any(|n|{
        let between=if n.start>=m.end{&text[m.end..n.start]}else if n.end<=m.start{&text[n.end..m.start]}else{return false};
        full(r"\s*(?:,\s*(?:(?:or|или|либо)\s+(?:(?:by|on|with|via|using|на|через|с)\s+)?)?|(?:or|или|либо)\s+(?:(?:by|on|with|via|using|на|через|с)\s+)?|/\s*|и\s*/\s*или\s+)(?:(?:claude|клод|anthropic|openai|codex|gpt)\s+)?",between)
    })
}
pub(super) fn scopes<'a>(text: &'a str, m: &Mention, all: &[Mention]) -> Vec<&'a str> {
    let previous = all.iter().filter(|n| n.end <= m.start).map(|n| n.end).max();
    let next = all
        .iter()
        .filter(|n| n.start >= m.end)
        .map(|n| n.start)
        .min();
    let start = re(REQUEST_BREAK)
        .find_iter(&text[..m.start])
        .map(|b| b.end())
        .chain(previous)
        .max()
        .unwrap_or(0);
    let end = re(REQUEST_BREAK)
        .find_iter(&text[m.end..])
        .map(|b| b.start() + m.end)
        .chain(next)
        .min()
        .unwrap_or(text.len());
    let before = &text[start..m.start];
    let after = &text[m.end..end];
    let owner = |s: &str| {
        let w = words(s);
        let first = w.first().is_some_and(|w| one(w, PURPOSE));
        let last = w.last().is_some_and(|w| one(w, EX_MEANS) || w == "by");
        (first, last)
    };
    let mut own = Vec::new();
    let names_work = |s: &str| {
        words(s)
            .iter()
            .any(|w| purpose_noun(w) || full(ROLE_OR_STAGE, w))
    };
    if !(Some(start) == previous && owner(before) == (true, false)) && names_work(before) {
        own.push(before);
    }
    if !(Some(end) == next && owner(after) == (false, true)) && names_work(after) {
        own.push(after);
    }
    if own.is_empty() && !names_work(before) && !names_work(after) {
        own.push(text);
    }
    own
}
fn product_choice(w: &[String]) -> bool {
    for (i, word) in w.iter().enumerate() {
        if !full(QUESTION_WHICH, word) {
            continue;
        }
        let (mut limit, mut verb, mut blocked, mut p, mut articles) = (i + 4, None, false, i, 0);
        while p < limit.min(w.len().saturating_sub(1)) {
            p += 1;
            let later = &w[p];
            if one(later, "the|a|an") {
                articles += 1;
                limit += 1;
                continue;
            }
            if role_noun(later) {
                if verb.is_some() && !blocked {
                    return true;
                }
                if p <= i + 4 + articles {
                    return w[p + 1..].iter().any(|w| full(CHOICE_REQUESTS, w));
                }
                break;
            }
            if full(CHOICE_REQUESTS, later) {
                if verb.is_none() {
                    verb = Some(p);
                    limit = p + 4;
                }
            } else if full(SELECTION_NOUN, later)
                || full(WORK_VERBS, later)
                || one(later, "for|для|to|чтобы|чтоб")
                || one(later, "на|в|во|on|in|at") && {
                    let mut f = &w[p + 1..];
                    if f.first().is_some_and(|w| one(w, "the|a|an")) {
                        f = &f[1..];
                    }
                    f.first().is_some_and(|w| role_noun(w))
                }
            {
                blocked = true;
            }
        }
    }
    false
}
pub(super) fn question_scope(question: &str, selection_only: bool) -> Vec<&str> {
    let mut selections = Vec::new();
    let mut neutral = Vec::new();
    for s in sentences(question) {
        let w = words(s);
        if w.is_empty()
            || w.iter().enumerate().any(|(i, w)| {
                full(QUESTION_JUDGEMENT, w)
                    || w.ends_with("'s") && !one(w, "let's|it's|that's|what's|here's|there's|who's")
                    || if cyr(w) {
                        (full(RU_SUBJUNCTIVE_WORK, w) || full(RU_SUBJUNCTIVE_REQUEST, w))
                            && !words(s)[..i].iter().any(|w| one(w, "чтобы|чтоб"))
                    } else {
                        one(
                        w,
                        "did|done|ran|wrote|written|built|made|chose|chosen|took|taken|gave|given",
                    ) || w.ends_with("ed")
                        && (1..=3).any(|n| {
                            w.len() >= n
                                && w.is_char_boundary(w.len() - n)
                                && [REQUEST_WORDS, WORK_VERBS]
                                    .iter()
                                    .any(|p| full(p, &w[..w.len() - n]))
                        })
                    }
            })
        {
            continue;
        }
        let product = product_choice(&w);
        let requested = re(CLAUSE_BREAK).split(s).any(|clause| {
            let w = words(clause);
            w.iter().enumerate().any(|(i, v)| {
                [REQUEST_WORDS, WORK_VERBS].iter().any(|p| {
                    request_form(p, v, &w[..i], true)
                        && !(product && *p == REQUEST_WORDS && full(CHOICE_REQUESTS, v))
                })
            }) || !product
                && w.iter().enumerate().any(|(i, v)| {
                    full(CHOICE_REQUESTS, v)
                        && i >= 2
                        && one(&w[i - 1], "i|we")
                        && one(&w[i - 2], "should|shall|can|could|would|will|must|do")
                })
        });
        let results = w.iter().any(|w| full(QUESTION_RESULTS, w));
        let joined = w.join(" ");
        let frame=re(r"(?i)(?:^| )(?:(?:как|котор)(?:ой|ою) моделью|(?:как|котор)ым (?:маршрутом|движком)|на (?:как|котор)(?:ой модели|ом (?:маршруте|движке))|(?:как|котор)ую модель|(?:как|котор)ая модель для|(?:как|котор)ой модели (?:поруч|переда|отда|назнач)\w*|(?:which|what) (?:model|route|engine)(?:$| (?:to|for|should|shall|will|would|must|can|could|do|does)))(?: |$)").is_match(&joined);
        if frame || requested && !results {
            selections.push(s);
        } else if !results && !product {
            neutral.push(s);
        }
    }
    if !selections.is_empty() || selection_only {
        selections
    } else {
        neutral
    }
}
fn bare_word(w: &str) -> bool {
    one(w, VENDORS)
        || one(w, FILLERS)
        || one(w, AFFIRM)
        || one(w, NOTES)
        || one(w, EXCLUSIVE)
        || one(w, TAIL)
        || w == "модель"
}
fn option_prefix(w: &[String], post: bool) -> bool {
    let mut i = 0;
    while i < w.len() && (one(&w[i], AFFIRM) || one(&w[i], TAIL)) {
        i += 1;
    }
    let mut verb = i;
    while verb < w.len() && cyr(&w[verb]) && role_noun(&w[verb]) {
        verb += 1;
    }
    let governor = if verb < w.len() && request_form(REQUEST_WORDS, &w[verb], &w[..verb], false) {
        if full(
            r"переда\w*|поруч\w*|отда\w*|назнач\w*|assign|delegate",
            &w[verb],
        ) {
            "dative"
        } else {
            "request"
        }
    } else if verb < w.len() && request_form(WORK_VERBS, &w[verb], &w[..verb], false) {
        "work"
    } else {
        ""
    };
    let mut objects = !governor.is_empty() && verb > i;
    if !governor.is_empty() {
        i = verb + 1;
    }
    let mut purpose = false;
    while i < w.len() {
        let word = &w[i];
        if one(word, PURPOSE) {
            purpose = true;
        } else if purpose && (purpose_noun(word) || one(word, FILLERS) || one(word, "and|or|и|или"))
        {
        } else if !purpose
            && (role_noun(word) && (!governor.is_empty() || cyr(word))
                || governor == "work" && full(OPTION_WORK_OBJECTS, word)
                || !governor.is_empty() && one(word, "it|this|это|его|её|ее"))
        {
            objects = true;
        } else if !purpose && (one(word, FILLERS) || objects && one(word, "and|or|и|или")) {
        } else {
            break;
        }
        i += 1;
    }
    let (mut exclusive, mut means, mut strict) = (post, false, false);
    for word in &w[i..] {
        if one(word, EXCLUSIVE) {
            exclusive = true;
        } else if one(word, EX_MEANS) || word == "to" && one(governor, "request|dative") {
            means = true;
            strict |= full(MEANS_WORDS, word);
        } else if !one(word, VENDORS) && !one(word, FILLERS) && word != "модель" {
            return false;
        }
    }
    if governor == "work" {
        strict || exclusive && (objects || means)
    } else {
        !objects || means || exclusive || governor == "dative"
    }
}
fn option_suffix(w: &[String]) -> bool {
    let mut state = "free";
    for word in w {
        if word == "@other" {
            if state != "substitution" {
                return false;
            }
            state = "replaced";
        } else if state == "substitution" {
            if !one(word, "of|than|на") && !one(word, VENDORS) && !one(word, FILLERS) {
                return false;
            }
        } else if one(word, "only|exclusively|solely") || one(word, TAIL) || one(word, NOTES) {
        } else if one(state, "free|replaced") && (one(word, VENDORS) || one(word, FILLERS)) {
        } else if state == "free"
            && full(
                r"instead|rather|replac\w*|substitut\w*|вместо|замен\w*",
                word,
            )
        {
            state = "substitution";
        } else if one(word, "for|для|to") {
            state = "purpose";
        } else if state == "purpose"
            && (purpose_noun(word)
                || one(word, FILLERS)
                || one(word, "it|this|это|его|её|ее|and|or|и|или"))
        {
        } else {
            return false;
        }
    }
    state != "substitution"
}
pub(super) fn selected(text: &str, m: &Mention, names: &[Name], question: &str) -> bool {
    let all = mentions(text, names);
    if refused(text, m, false)
        || all
            .iter()
            .any(|n| n.model != m.model && !refused(text, n, false))
    {
        return false;
    }
    if assignment(text, m).is_some() {
        return true;
    }
    let text = re(r"\(([^()]*\d\s*%[^()]*)\)")
        .replace_all(text, |c: &regex::Captures| {
            if words(&c[1]).iter().all(|w| full(USAGE_NOTE_WORDS, w)) {
                " ".into()
            } else {
                c[0].to_string()
            }
        })
        .into_owned();
    let all = mentions(&text, names);
    let mut tokens = Vec::new();
    let mut cursor = 0;
    for n in &all {
        if n.start < cursor {
            continue;
        }
        let mut w = words(&text[cursor..n.start]);
        if n.model == m.model && text[..n.start].ends_with('(') && text[n.end..].starts_with(')') {
            for word in w.iter_mut().rev().take(2) {
                if cyr(word)
                    || one(word, EXCLUSIVE)
                    || full(ROLE_OR_STAGE, word)
                    || full(REQUEST_WORDS, word)
                    || one(word, "for|для|to")
                    || one(word, AFFIRM)
                    || one(word, NOTES)
                    || one(word, TAIL)
                {
                    break;
                }
                *word = "@self".into();
            }
        }
        tokens.extend(w);
        tokens.push(
            if n.model == m.model {
                "@self"
            } else {
                "@other"
            }
            .into(),
        );
        cursor = n.end;
    }
    tokens.extend(words(&text[cursor..]));
    let Some(first) = tokens.iter().position(|w| w == "@self") else {
        return false;
    };
    let last = tokens.iter().rposition(|w| w == "@self").unwrap();
    if first < last
        && !tokens[first + 1..last]
            .iter()
            .all(|w| w == "@self" || one(w, VENDORS) || one(w, FILLERS))
    {
        return false;
    }
    let prefix: Vec<_> = tokens[..first]
        .iter()
        .filter(|w| w.as_str() != "@self")
        .cloned()
        .collect();
    let suffix = &tokens[last + 1..];
    if !option_prefix(
        &prefix,
        suffix
            .first()
            .is_some_and(|w| one(w, "only|exclusively|solely")),
    ) || !option_suffix(suffix)
    {
        return false;
    }
    let cut = suffix
        .iter()
        .position(|w| full(r"instead|rather|replac\w*|substitut\w*|вместо|замен\w*", w))
        .unwrap_or(suffix.len());
    let verbs: Vec<_> = prefix
        .iter()
        .enumerate()
        .filter(|(_, w)| !bare_word(w))
        .collect();
    let bare = suffix[..cut].iter().all(|w| bare_word(w))
        && suffix[cut..].iter().all(|w| {
            w == "@other"
                || bare_word(w)
                || one(w, "of|than|на")
                || full(r"instead|rather|replac\w*|substitut\w*|вместо|замен\w*", w)
        })
        && (verbs.is_empty()
            || verbs.len() == 1
                && full(CHOICE_REQUESTS, verbs[0].1)
                && request_form(REQUEST_WORDS, verbs[0].1, &prefix[..verbs[0].0], false));
    !bare || !question_scope(question, true).is_empty()
}
