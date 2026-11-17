//! Worker skill/reference rendering from the shared `agent-sources` (`profiles check|write`).

use crate::fsutil::{py_rstrip, py_split_ws, py_splitlines, py_strip, read_bytes, read_text, write_bytes};
use crate::json::{parse, quote, Res, Value};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const WORKER_REFERENCES: &[&str] = &["code-and-evidence.md", "interactive-resources.md", "report-publication.md"];
const SPECIALIZED: &[&str] = &[
    "Profile title",
    "Role and responsibility",
    "When to use this profile",
    "Specialist workflow",
    "Quality criteria",
    "Report and handoff",
];
const REPORT_TEMPLATES: &[&str] =
    &["general", "planning", "investigation", "implementation", "verification", "documentation", "synthesis"];

fn verification_source(access_class: &str) -> Option<&'static str> {
    match access_class {
        "write" => Some("verification-write.md"),
        "read_only" => Some("verification-read-only.md"),
        "consultant" => Some("verification-consultant.md"),
        _ => None,
    }
}

fn sections(markdown: &str) -> Res<BTreeMap<String, String>> {
    let lines = py_splitlines(py_rstrip(markdown));
    if lines.is_empty() || !lines[0].starts_with("# ") {
        return Err("specialization must start with one profile title".into());
    }
    let mut result = BTreeMap::new();
    result.insert("Profile title".to_string(), lines[0].to_string());
    let rest = lines[1..].join("\n");
    // re.split(r"(?m)^## ", rest)
    let mut starts = Vec::new();
    if rest.starts_with("## ") {
        starts.push(0);
    }
    for (i, _) in rest.match_indices('\n') {
        if rest[i + 1..].starts_with("## ") {
            starts.push(i + 1);
        }
    }
    let first_end = starts.first().copied().unwrap_or(rest.len());
    if !py_strip(&rest[..first_end]).is_empty() {
        return Err("specialization has text between the title and its first section; move it into a section".into());
    }
    for (n, start) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(rest.len());
        let chunk = &rest[start + 3..end];
        let Some((heading, body)) = chunk.split_once('\n') else {
            return Err("invalid or duplicate specialization section".into());
        };
        if result.contains_key(heading) {
            return Err("invalid or duplicate specialization section".into());
        }
        result.insert(heading.to_string(), py_strip(body).to_string());
    }
    let keys: BTreeSet<&str> = result.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = SPECIALIZED.iter().copied().collect();
    if keys != expected {
        return Err("specialization sections do not match the canonical template".into());
    }
    Ok(result)
}

fn when_to_use(section: &str) -> Res<BTreeMap<&'static str, String>> {
    let bullet = Regex::new(r"^- \*\*(.+?):\*\* (.*)").unwrap();
    let mut found: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let mut current: Option<&'static str> = None;
    for line in py_splitlines(section) {
        if let Some(m) = bullet.captures(line) {
            let field = match &m[1] {
                "Select" => Some("select_when"),
                "Choose another specialist" => Some("avoid_when"),
                _ => None,
            };
            match field {
                Some(f) if !found.contains_key(f) => {
                    found.insert(f, vec![m[2].to_string()]);
                    current = Some(f);
                }
                _ => return Err("invalid or duplicate When to use bullet".into()),
            }
        } else if let Some(c) = current {
            if !py_strip(line).is_empty() {
                found.get_mut(c).unwrap().push(py_strip(line).to_string());
            }
        }
    }
    if found.len() != 2 {
        return Err("When to use must have one Select and one Choose another specialist bullet".into());
    }
    Ok(found.into_iter().map(|(k, v)| (k, py_split_ws(&v.join(" ")).join(" "))).collect())
}

pub fn expected_skills(root: &Path) -> Res<BTreeMap<PathBuf, Vec<u8>>> {
    let sources = root.join("agent-sources");
    let template = read_text(&sources.join("worker-protocol.md"))?;
    let profiles_doc = parse(&read_text(&root.join("profiles.json"))?)?;
    let profiles = profiles_doc.req("profiles")?.as_arr().ok_or("invalid profiles")?;
    let mut rendered = BTreeMap::new();
    for profile in profiles {
        let pname = profile.get("name").and_then(Value::as_str).unwrap_or("?");
        let access_class = profile.get("access_class").and_then(Value::as_str).unwrap_or("");
        let verification = verification_source(access_class).ok_or(format!("invalid access class for {pname}"))?;
        let source = profile.req("source")?.as_str().ok_or("invalid source")?;
        let report_template = profile.get("report_template").and_then(Value::as_str).unwrap_or("");
        if !REPORT_TEMPLATES.contains(&report_template) {
            return Err(format!("invalid report template for {pname}"));
        }
        let values = sections(&read_text(&sources.join(source))?)?;
        let stated = when_to_use(&values["When to use this profile"])?;
        for key in ["select_when", "avoid_when"] {
            if profile.get(key).and_then(Value::as_str) != stated.get(key).map(String::as_str) {
                return Err(format!("profiles.json select_when/avoid_when differ from {source} When to use"));
            }
        }
        let selected = if access_class == "consultant" {
            read_text(&sources.join("consultant-protocol.md"))?
        } else {
            template.clone()
        };
        let mut prompt = selected.replace("{{Default report template}}", report_template);
        prompt = prompt.replace(
            "{{Evidence and verification}}",
            py_strip(&read_text(&sources.join(verification))?),
        );
        for key in SPECIALIZED {
            prompt = prompt.replace(&format!("{{{{{key}}}}}"), &values[*key]);
        }
        if prompt.contains("{{") || prompt.contains("}}") {
            return Err("unresolved worker protocol placeholder".into());
        }
        let stem = source.strip_suffix(".md").unwrap_or(source);
        let stem = Path::new(stem).file_name().and_then(|s| s.to_str()).unwrap_or(stem);
        let name = format!("worker-{stem}");
        let description = format!(
            "Cortex delegated specialist only: {}",
            profile.req("description")?.as_str().ok_or("invalid description")?
        );
        let text = format!(
            "---\nname: {name}\ndescription: {}\n---\n\n{}\n\n<!-- END OF COMPLETE CORTEX WORKER SKILL -->\n",
            quote(&description, true),
            py_rstrip(&prompt)
        );
        rendered.insert(root.join("skills").join(&name).join("SKILL.md"), text.into_bytes());
    }
    Ok(rendered)
}

pub fn expected_worker_references(root: &Path) -> Res<BTreeMap<PathBuf, Vec<u8>>> {
    let source_root = root.join("agent-sources/references");
    let mut rendered = BTreeMap::new();
    for skill_path in expected_skills(root)?.keys() {
        let reference_root = skill_path.parent().unwrap().join("references");
        for filename in WORKER_REFERENCES {
            rendered.insert(reference_root.join(filename), read_bytes(&source_root.join(filename))?);
        }
    }
    Ok(rendered)
}

fn existing_generated(root: &Path) -> Res<(BTreeSet<PathBuf>, BTreeSet<PathBuf>)> {
    let mut skills = BTreeSet::new();
    let mut references = BTreeSet::new();
    let dir = root.join("skills");
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("worker-") {
            continue;
        }
        let skill = entry.path().join("SKILL.md");
        if skill.exists() {
            skills.insert(skill);
        }
        if let Ok(refs) = std::fs::read_dir(entry.path().join("references")) {
            for r in refs.flatten() {
                if r.path().extension().and_then(|e| e.to_str()) == Some("md") {
                    references.insert(r.path());
                }
            }
        }
    }
    Ok((skills, references))
}

pub fn check(root: &Path) -> Res<()> {
    let skills = expected_skills(root)?;
    if skills.len() != 23 {
        return Err(format!("expected exactly 23 worker profiles, found {}", skills.len()));
    }
    let (found_skills, found_refs) = existing_generated(root)?;
    if found_skills != skills.keys().cloned().collect::<BTreeSet<_>>() {
        return Err("generated worker skill set differs from source catalogue".into());
    }
    let references = expected_worker_references(root)?;
    if found_refs != references.keys().cloned().collect::<BTreeSet<_>>() {
        return Err("generated worker reference set differs from source catalogue".into());
    }
    for (path, body) in skills.iter().chain(references.iter()) {
        if read_bytes(path)? != *body {
            return Err(format!("generated worker skill or reference is stale: {}", path.display()));
        }
    }
    Ok(())
}

pub fn write(root: &Path) -> Res<()> {
    let mut all = expected_skills(root)?;
    all.extend(expected_worker_references(root)?);
    for (path, body) in all {
        write_bytes(&path, &body)?;
    }
    Ok(())
}

pub fn run(root: &Path, do_write: bool) -> Res<()> {
    let result = if do_write { write(root) } else { check(root) };
    result.map_err(|e| format!("Agent profile generation failed: {e}"))?;
    println!("Worker skills {}: 23 compact prompts with shared references", if do_write { "write" } else { "check" });
    Ok(())
}
