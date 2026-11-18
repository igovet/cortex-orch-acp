//! The generators and the package identity must agree with the committed artifacts.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn xtask(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cortex-xtask"))
        .args(args)
        .arg("--root")
        .arg(root)
        .output()
        .unwrap()
}

fn ok(args: &[&str]) {
    let out = xtask(&root(), args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn generated_contract_matches_committed_files() {
    ok(&["generate-contract", "check"]);
}

#[test]
fn native_instruction_data_matches_sources() {
    ok(&["generate-instructions", "check"]);
}

#[test]
fn worker_skills_match_agent_sources() {
    ok(&["profiles", "check"]);
}

#[test]
fn package_identity_is_current() {
    ok(&["package", "check"]);
}

#[test]
fn runtime_payload_contains_no_python_and_no_src_tree() {
    let out = xtask(&root(), &["runtime-payload", "list"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let listing = String::from_utf8(out.stdout).unwrap();
    assert!(listing.lines().count() > 100);
    for line in listing.lines() {
        assert!(
            !line.ends_with(".py") && !line.ends_with(".pyc") && !line.starts_with("src/"),
            "{line}"
        );
    }
}

#[test]
fn generated_drift_is_detected_in_a_scratch_tree() {
    let scratch = std::env::temp_dir().join(format!("xtask-drift-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    for dir in [
        "crates/workspace-protocol/schema",
        "crates/workspace-protocol/src",
        "packages/workspace-contract",
    ] {
        std::fs::create_dir_all(scratch.join(dir)).unwrap();
        for entry in std::fs::read_dir(root().join(dir)).unwrap().flatten() {
            if entry.path().is_file() {
                std::fs::copy(entry.path(), scratch.join(dir).join(entry.file_name())).unwrap();
            }
        }
    }
    std::fs::write(scratch.join("package-identity.json"), "{}").unwrap();
    assert!(xtask(&scratch, &["generate-contract", "check"])
        .status
        .success());
    let generated = scratch.join("crates/workspace-protocol/src/generated.rs");
    let mut text = std::fs::read_to_string(&generated).unwrap();
    text.push_str("// drift\n");
    std::fs::write(&generated, text).unwrap();
    let out = xtask(&scratch, &["generate-contract", "check"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("crates/workspace-protocol/src/generated.rs")
    );
    // An immutable fingerprint: changing the schema without a version bump must fail.
    let schema = scratch.join("crates/workspace-protocol/schema/workspace.json");
    let changed = std::fs::read_to_string(&schema).unwrap().replacen(
        "\"x-mapping-version\"",
        "\"x-mapping-versionz\"",
        1,
    );
    std::fs::write(&schema, changed).unwrap();
    assert!(!xtask(&scratch, &["generate-contract", "check"])
        .status
        .success());
    let _ = std::fs::remove_dir_all(&scratch);
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(root().join(relative)).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            markdown_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

#[test]
fn the_python_runtime_and_its_test_tree_are_gone() {
    let root = root();
    assert!(
        !root.join("src").exists(),
        "the Python runtime tree must not return"
    );
    assert!(!root.join("pytest.ini").exists());
    let tests = root.join("tests");
    if tests.exists() {
        for entry in std::fs::read_dir(tests).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                !name.ends_with(".py") && name != "workspace_support",
                "{name}"
            );
        }
    }
    assert!(!read("pinned-dependencies.json").contains("python_version_floor"));
}

/// Shipped model instructions name only tools the runtime serves. A tool that
/// is retired from every catalogue must not be advertised or named by any skill,
/// profile source or the generated instruction data. (Catalogued tools without
/// a handler are caught by the workspace-service catalogue test.)
#[test]
fn shipped_instructions_name_no_retired_cortex_tool() {
    const RETIRED: &[&str] = &["read_diagnostics"];
    let root = root();
    let catalogue = read("crates/cortex-engine/schema/tools.json");
    let advertised: std::collections::BTreeSet<String> =
        regex::Regex::new(r#""name": "([a-z_]+)",\s*"title""#)
            .unwrap()
            .captures_iter(&catalogue)
            .map(|c| c[1].to_owned())
            .collect();
    assert!(
        advertised.contains("read_capsule") && advertised.len() > 30,
        "{advertised:?}"
    );
    let mut sources = Vec::new();
    for dir in ["skills", "agent-sources"] {
        markdown_files(&root.join(dir), &mut sources);
    }
    sources.push(root.join("crates/workspace-service/schema/instructions.json"));
    let token = regex::Regex::new(r"\b[a-z]+(?:_[a-z]+)+\b").unwrap();
    for retired in RETIRED {
        assert!(
            !advertised.contains(*retired),
            "{retired} is still catalogued"
        );
    }
    // Operation names of an external code-index server: never Cortex tools, and a worker
    // session is not served them, so shipped instructions (references included) describe
    // such operations without naming them.
    const NOT_CORTEX: &[&str] = &[
        "search_code",
        "search_graph",
        "list_projects",
        "trace_path",
        "get_code_snippet",
        "query_graph",
        "get_architecture",
        "get_graph_schema",
        "detect_changes",
        "index_repository",
        "index_status",
        "check_index_coverage",
    ];
    let namespaced = regex::Regex::new(r"\bcortex\.([a-z]+(?:_[a-z]+)+)\b").unwrap();
    for source in sources {
        let text = std::fs::read_to_string(&source).unwrap();
        for found in token.find_iter(&text) {
            assert!(
                !RETIRED.contains(&found.as_str()),
                "{} names retired tool {}",
                source.display(),
                found.as_str()
            );
            assert!(
                !NOT_CORTEX.contains(&found.as_str()),
                "{} names non-Cortex tool {}",
                source.display(),
                found.as_str()
            );
        }
        for found in namespaced.captures_iter(&text) {
            assert!(
                advertised.contains(&found[1]),
                "{} names unknown Cortex tool cortex.{}",
                source.display(),
                &found[1]
            );
        }
    }
}

#[test]
fn the_packaged_skills_are_complete_and_agents_are_not_a_payload_entry() {
    let root = root();
    let workers = std::fs::read_dir(root.join("skills"))
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("worker-")
                && e.path().join("SKILL.md").is_file()
        })
        .count();
    assert_eq!(workers, 23, "22 specialists and the consultant");
    assert!(!root.join("agents").exists());
    assert!(root
        .join("integrations/paseo/plugin-integrity.json")
        .is_file());
}

#[test]
fn markdown_local_links_resolve() {
    let root = root();
    let mut files: Vec<PathBuf> = ["README.md", "SECURITY.md", "PRIVACY.md"]
        .iter()
        .map(|f| root.join(f))
        .collect();
    markdown_files(&root.join("docs"), &mut files);
    markdown_files(&root.join("skills"), &mut files);
    let link = regex::Regex::new(r"\]\(([^)]+)\)").unwrap();
    let mut broken = vec![];
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        for capture in link.captures_iter(&text) {
            let target = &capture[1];
            if target.contains("://") || target.starts_with('#') {
                continue;
            }
            let file = target.split('#').next().unwrap();
            if !path.parent().unwrap().join(file).exists() {
                broken.push(format!(
                    "{}: {target}",
                    path.strip_prefix(&root).unwrap().display()
                ));
            }
        }
    }
    assert!(
        broken.is_empty(),
        "broken local links:\n{}",
        broken.join("\n")
    );
}

#[test]
fn the_orchestrator_skill_stays_compact_and_routes_to_existing_references() {
    let root = root();
    let body = read("skills/orchestrator/SKILL.md");
    assert!(body.chars().count() < 8600, "{}", body.chars().count());
    let links: std::collections::BTreeSet<String> = regex::Regex::new(r"\]\((references/[^)]+)\)")
        .unwrap()
        .captures_iter(&body)
        .map(|c| c[1].to_owned())
        .collect();
    assert!(links.len() >= 7, "{links:?}");
    for link in &links {
        assert!(
            root.join("skills/orchestrator").join(link).is_file(),
            "{link}"
        );
    }
    assert!(
        !body.contains("decide"),
        "the orchestrator skill must not describe model acceptance"
    );
    assert!(body.to_lowercase().contains("runtime"));
    let routing = read("skills/orchestrator/references/worker-routing.md");
    assert!(routing.contains("## Runtime-reserved (not plannable)"));
    for reserved in ["code-reviewer", "build-verification", "senior-consultant"] {
        assert!(routing.contains(&format!("`{reserved}`")), "{reserved}");
    }
}

#[test]
fn instruction_sources_state_the_evidence_and_publication_boundaries() {
    let worker = read("agent-sources/worker-protocol.md");
    assert!(worker.contains("These role instructions are delivered"));
    assert!(!worker.contains("request_key"));
    for source in [
        "agent-sources/verification-write.md",
        "agent-sources/verification-read-only.md",
    ] {
        assert!(
            read(source)
                .contains("Unrun, failed, truncated and unverifiable checks remain explicit."),
            "{source}"
        );
    }
    let discipline = read("skills/tool-discipline/SKILL.md");
    for phrase in [
        "Reuse retained results while relevant state is unchanged.",
        "never replay a mutation for reassurance",
        "native subagents are disabled",
        "Read each tool's complete live description and schema",
    ] {
        assert!(discipline.contains(phrase), "{phrase}");
    }
    assert!(discipline.chars().count() < 4000);
    let publication = read("agent-sources/references/report-publication.md");
    assert!(publication.contains("immutable report"));
    assert!(publication.contains("does not replace the structured result"));
    let interactive = read("agent-sources/references/interactive-resources.md");
    assert!(interactive.contains("select, create or reuse only owned resources"));
    assert!(
        read("skills/orchestrator/references/pipeline-publication.md")
            .to_lowercase()
            .contains("runtime")
    );
}

#[test]
fn the_paseo_provider_keeps_its_replay_and_command_guards() {
    let provider = read("integrations/paseo/server/provider.ts");
    assert!(provider.contains("waitForInitialCommands: true"));
    assert!(provider.contains("initialCommandsTimeoutMs: 2000"));
    // Canceled failed tool cards must stay reloadable; supplied errors are preserved.
    for needle in [
        "event.item.status === \"failed\"",
        "event.item.error === null",
        "error: `Tool call failed: ${event.item.name}`",
        "return event;",
    ] {
        assert!(provider.contains(needle), "{needle}");
    }
    for file in [
        "acp-load-shim.mjs",
        "replay-spool.mjs",
        "replay-connector.ts",
    ] {
        assert!(
            root()
                .join("integrations/paseo/server")
                .join(file)
                .is_file(),
            "{file}"
        );
    }
}
