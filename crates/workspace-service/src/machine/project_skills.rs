//! `repo.skills`: the project's own skills, named for the composer's `/` list. It is the same
//! read-only, bounded index the coordinator's instructions list and `read_skill` reads by name
//! (`native_skills::index`), so the composer never offers a skill the session cannot read.
//! Only names, project-relative paths and bounded descriptions leave; no skill body does, and
//! nothing is activated.
use super::native::*;
use serde_json::{json, Value};
use std::path::Path;

pub fn list(checkout: &Path) -> Result<Value> {
    let (skills, omitted) = crate::native_skills::index(checkout);
    let skills: Vec<Value> = skills
        .into_iter()
        .map(|s| {
            json!({"name":s.name,"path":s.path,
                "description":(!s.description.is_empty()).then_some(s.description),
                // A Cortex skill of the same name wins a read by name; only the path selects this file.
                "pathOnly":s.shadowed})
        })
        .collect();
    Ok(json!({"skills":skills,"omitted":omitted}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    /// An owned fixture folder, removed when the test ends.
    struct Tmp(std::path::PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tmp(name: &str) -> Tmp {
        let root =
            std::env::temp_dir().join(format!("cw-repo-skills-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Tmp(root.canonicalize().unwrap())
    }
    fn skill(root: &Path, folder: &str, name: &str, body: &str) {
        let dir = root.join(folder).join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), body).unwrap();
    }
    #[test]
    fn names_project_skills_with_paths_and_descriptions_and_never_a_body() {
        let t = tmp("names");
        skill(&t.0, ".agents/skills", "demo-data", "---\nname: demo-data\ndescription: \"Use when updating demo data.\"\n---\n# Body\nsecret body text\n");
        skill(
            &t.0,
            ".claude/skills",
            "demo-data",
            "---\ndescription: shadowed\n---\n",
        );
        skill(&t.0, ".claude/skills", "bare", "# No front matter\n");
        skill(
            &t.0,
            ".agents/skills",
            "tool-discipline",
            "---\ndescription: same name as a bundled skill\n---\n",
        );
        let v = list(&t.0).unwrap();
        let skills = v["skills"].as_array().unwrap();
        let names: Vec<_> = skills.iter().map(|s| s["path"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                ".agents/skills/demo-data/SKILL.md",
                ".agents/skills/tool-discipline/SKILL.md",
                ".claude/skills/bare/SKILL.md"
            ]
        );
        assert_eq!(skills[0]["name"], "demo-data");
        assert_eq!(skills[0]["description"], "Use when updating demo data.");
        assert_eq!(skills[0]["pathOnly"], false);
        assert_eq!(skills[1]["pathOnly"], true);
        assert!(skills[2]["description"].is_null());
        assert_eq!(v["omitted"], 0);
        assert!(!v.to_string().contains("secret body text"));
    }
    #[test]
    fn names_nothing_outside_the_checkout() {
        let t = tmp("outside");
        let (root, outside) = (t.0.join("p"), t.0.join("o"));
        fs::create_dir_all(root.join(".agents/skills")).unwrap();
        skill(&outside, "", "far", "---\ndescription: far\n---\n");
        std::os::unix::fs::symlink(outside.join("far"), root.join(".agents/skills/far")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".claude")).unwrap();
        assert_eq!(list(&root).unwrap()["skills"], json!([]));
    }
}
