//! Non-executing Git observation. Ordinary commands keep their own Git behavior.
use super::{policy, runtime_files, unavailable, Sandbox};
use std::{fs, io, os::unix::fs::symlink, path::Path, process::Stdio};

impl Sandbox {
    pub(crate) fn restrict_git_observation(&mut self, root: &Path) -> io::Result<()> {
        let isolated = Path::new(runtime_files::HOME).join("git-observation");
        let Some((dir, common)) = &self.git_context else {
            // Preserve Git's non-repository result without allowing discovery
            // of a newly appearing repository or one above the task checkout.
            self.environment
                .insert("GIT_DIR".into(), isolated.into_os_string());
            return Ok(());
        };
        let config = common.join("config");
        let config = config.to_str().ok_or_else(|| unavailable("invalid_git_config_path"))?;
        // Read only direct format settings from inside the already masked view.
        // The later observation cannot see this file as Git configuration, even
        // if the host edits it between these two invocations.
        let mut query = self.command(
            &["/usr/bin/git", "config", "--no-includes", "--file", config, "--null", "--list"]
                .map(str::to_owned),
        )?;
        query.stderr(Stdio::null());
        let settings = policy::s_output(&mut query)?
            .ok_or_else(|| unavailable("git_observation_config_unavailable"))?;
        let scratch = self.scratch.join("git-observation");
        fs::create_dir(&scratch)?;
        fs::write(scratch.join("config"), observation_config(&settings)?)?;
        // HEAD, index and operation state still come from GIT_DIR. Only Git's
        // shared data paths (not config or worktree registrations) are delegated
        // to the original, read-only, masked common directory.
        for name in ["objects", "refs", "packed-refs", "reftable", "shallow", "info", "logs"] {
            symlink(common.join(name), scratch.join(name))?;
        }
        // Only switch Git to the isolated common directory once its
        // configuration and shared data paths actually exist.
        self.environment.insert("GIT_WORK_TREE".into(), root.as_os_str().into());
        self.environment.insert("GIT_COMMON_DIR".into(), isolated.into_os_string());
        self.environment.insert("GIT_NO_LAZY_FETCH".into(), "1".into());
        self.environment.insert("GIT_DIR".into(), dir.as_os_str().into());
        Ok(())
    }
}

fn observation_config(settings: &str) -> io::Result<String> {
    let mut core = String::from("[core]\n");
    let mut extensions = String::from("[extensions]\n");
    for entry in settings.split_terminator('\0') {
        // For a valueless boolean Git prints just the key.
        let (key, value) = entry.split_once('\n').unwrap_or((entry, "true"));
        let valid = match key {
            "core.repositoryformatversion" => matches!(value, "0" | "1"),
            "core.filemode" | "core.symlinks" | "core.ignorecase" | "core.precomposeunicode"
            | "extensions.preciousobjects" => {
                matches!(value.to_ascii_lowercase().as_str(),
                    "true" | "false" | "yes" | "no" | "on" | "off" | "0" | "1")
            }
            "extensions.objectformat" => matches!(value, "sha1" | "sha256"),
            "extensions.refstorage" => matches!(value, "files" | "reftable"),
            // No include, filter, config.worktree extension, external diff,
            // hook, fsmonitor, submodule or remote configuration crosses.
            _ => continue,
        };
        if !valid {
            return Err(unavailable("unsupported_git_observation_format"));
        }
        let (section, name) = key.split_once('.').unwrap();
        let output = if section == "core" { &mut core } else { &mut extensions };
        output.push_str(&format!("\t{name} = {value}\n"));
    }
    core.push_str(&extensions);
    Ok(core)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_config_copies_only_typed_format_settings() {
        let safe = observation_config(concat!(
            "core.repositoryformatversion\n1\0core.filemode\nfalse\0",
            "extensions.objectformat\nsha256\0extensions.worktreeconfig\ntrue\0",
            "include.path\nconfig.extra\0includeif.gitdir:./.path\nconditional\0",
            "filter.synthetic.clean\n/usr/bin/false\0core.fsmonitor\n/usr/bin/false\0",
        )).unwrap();
        assert_eq!(
            safe,
            "[core]\n\trepositoryformatversion = 1\n\tfilemode = false\n[extensions]\n\tobjectformat = sha256\n"
        );
        assert!(observation_config("core.filemode\ntrue\n[filter \"untrusted\"]\0").is_err());
    }
}
