//! `repo.list`: the Code tree's on-demand folder reads. Each folder is read alone, one level,
//! by descriptor and without following links; nothing is walked into. Git-ignored files and
//! dot-files are listed like any other and marked `ignored`; `.git` and Cortex's private names
//! never are. The overview names only what the snapshot's tree leaves out, collapsed.
use super::{super::files, Git, UNWALKED};
use crate::machine::native::*;
use serde_json::{json, Value};
use std::{ffi::CStr, os::fd::IntoRawFd, path::Path};

/// Entries shown per folder; the rest are counted.
pub const MAX_ENTRIES: usize = 500;
/// Folders read in one call.
pub const MAX_FOLDERS: usize = 32;
/// Names one folder read looks at; past it the count of the rest is a lower bound.
const MAX_SCAN: usize = 100_000;
/// Overview entries kept, and the most Git output read for them.
const MAX_OVERVIEW: usize = 5000;
const OVERVIEW_BYTES: usize = 1024 * 1024;

struct Entry {
    name: String,
    dir: bool,
    link: bool,
}
struct Folder {
    entries: Vec<Entry>,
    more: usize,
    exact: bool,
}

/// One level of one folder, sorted by name and capped at `MAX_ENTRIES`.
fn read_folder(checkout: &Path, dir: &str) -> Result<Folder> {
    let fd = files::open_folder(checkout, dir)?.into_raw_fd();
    // SAFETY: `fd` is an owned open directory descriptor; `closedir` below closes it.
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(error.into());
    }
    let (mut found, mut hidden, mut scanned) = (Vec::new(), 0usize, 0usize);
    let mut exact = true;
    loop {
        // SAFETY: `stream` stays open until `closedir`; each entry is copied before the next read.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let (name, kind) = unsafe { (CStr::from_ptr((*entry).d_name.as_ptr()), (*entry).d_type) };
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        scanned += 1;
        if scanned > MAX_SCAN {
            exact = false;
            break;
        }
        // A name that is not UTF-8 cannot be named back to the machine; it is counted, not shown.
        let Ok(text) = std::str::from_utf8(bytes) else {
            hidden += 1;
            continue;
        };
        if RESERVED.contains(&text) {
            continue;
        }
        let kind = if kind == libc::DT_UNKNOWN {
            let mut info: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: plain `fstatat` on the open folder; it never follows the final link.
            let rc = unsafe {
                libc::fstatat(
                    libc::dirfd(stream),
                    name.as_ptr(),
                    &mut info,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            match (rc, info.st_mode & libc::S_IFMT) {
                (0, libc::S_IFDIR) => libc::DT_DIR,
                (0, libc::S_IFLNK) => libc::DT_LNK,
                _ => libc::DT_REG,
            }
        } else {
            kind
        };
        found.push(Entry {
            name: text.to_owned(),
            dir: kind == libc::DT_DIR,
            link: kind == libc::DT_LNK,
        });
    }
    unsafe { libc::closedir(stream) };
    found.sort_by(|a, b| a.name.cmp(&b.name));
    let more = found.len().saturating_sub(MAX_ENTRIES) + hidden;
    found.truncate(MAX_ENTRIES);
    Ok(Folder {
        entries: found,
        more,
        exact,
    })
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// `{dirs: [folder, ...], overview: bool}` for one checkout. `""` names the checkout's own folder.
pub async fn list(checkout: &Path, p: &Value, git: &mut Git) -> Result<Value> {
    let dirs = match &p["dirs"] {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .map(|d| {
                d.as_str()
                    .ok_or_else(|| refuse("invalid_request", "Folder names are text."))
            })
            .collect::<Result<Vec<_>>>()?,
        _ => return Err(refuse("invalid_request", "Folder names are a list.")),
    };
    if dirs.len() > MAX_FOLDERS {
        return Err(refuse("invalid_request", "Too many folders in one read."));
    }
    // A malformed or private name is a wrong request, not a folder that went away.
    for dir in &dirs {
        if !dir.is_empty() {
            relative(dir)?;
            if dir.split('/').any(|p| RESERVED.contains(&p)) {
                return Err(refuse("invalid_path", "Private helper path."));
            }
        }
    }
    let repo = git.is_repo(checkout).await?;
    let mut folders = Vec::new();
    let mut asked = Vec::new();
    for dir in &dirs {
        match read_folder(checkout, dir) {
            Ok(folder) => {
                asked.extend(folder.entries.iter().map(|e| join(dir, &e.name)));
                folders.push((*dir, Ok(folder)));
            }
            Err(e) => folders.push((*dir, Err(e))),
        }
    }
    // One Git read marks every listed entry; a folder Git cannot judge (inside a submodule)
    // leaves the marks unknown rather than failing the listing.
    let all = if repo {
        git.ignored(checkout, &asked).await.ok()
    } else {
        Some(
            asked
                .iter()
                .filter(|p| p.split('/').any(|c| UNWALKED.contains(&c)))
                .cloned()
                .collect(),
        )
    };
    let mut replies = Vec::new();
    for (dir, folder) in folders {
        let f = match folder {
            Ok(f) => f,
            Err(e) => {
                replies.push(json!({"dir":dir,"code":e.code,"message":e.message}));
                continue;
            }
        };
        // Only after the joint read failed: each folder alone, so one bad folder blanks only its own marks.
        let own = match &all {
            Some(_) => None,
            None if f.entries.is_empty() => Some(Default::default()),
            None => {
                let paths: Vec<_> = f.entries.iter().map(|e| join(dir, &e.name)).collect();
                git.ignored(checkout, &paths).await.ok()
            }
        };
        let ignored = all.as_ref().or(own.as_ref());
        replies.push(json!({"dir":dir,"more":f.more,"exact":f.exact,"entries":f.entries.iter().map(|e| {
            let path = join(dir, &e.name);
            json!({"name":e.name,"dir":e.dir,"link":e.link,"ignored":ignored.map(|set| set.contains(&path))})
        }).collect::<Vec<_>>()}));
    }
    let folders = replies;
    let overview = if p["overview"] == true {
        let found = if repo {
            git.outside_tree(checkout, OVERVIEW_BYTES).await?
        } else {
            let mut skipped = Vec::new();
            super::folder_tree_with(checkout, &mut skipped)?;
            Some(skipped.into_iter().map(|s| (s, true, true)).collect())
        };
        Some(match found {
            Some(mut entries) => {
                entries.sort();
                entries.dedup_by(|a, b| a.0 == b.0);
                let truncated = entries.len() > MAX_OVERVIEW;
                entries.truncate(MAX_OVERVIEW);
                json!({"truncated":truncated,"entries":entries.into_iter()
                    .map(|(path, dir, ignored)| json!({"path":path,"dir":dir,"ignored":ignored}))
                    .collect::<Vec<_>>()})
            }
            None => json!({"truncated":true,"entries":[]}),
        })
    } else {
        None
    };
    let reply = json!({"overview":overview,"folders":folders});
    if serde_json::to_vec(&reply).map_or(true, |b| b.len() > FRAME_BUDGET) {
        return Err(refuse(
            "listing_too_large",
            "These folders are too large to list in one read.",
        ));
    }
    Ok(reply)
}
