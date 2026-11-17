//! Bounded native directory search. Directory metadata only; never file contents.
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
const FAR: usize = usize::MAX / 4;
const IGNORED: &[&str] = &[
    "node_modules",
    "venv",
    "env",
    "virtualenv",
    "dist",
    "build",
    "target",
    "out",
    "coverage",
    "vendor",
    "__pycache__",
    ".git",
];
pub struct Search {
    pub query: String,
    pub home: PathBuf,
    pub allowed: PathBuf,
    pub roots: Vec<PathBuf>,
    pub excluded: Vec<PathBuf>,
    pub limit: usize,
    pub cancelled: Arc<AtomicBool>,
    pub max_depth: usize,
    pub max_folders: usize,
    pub max_entries: usize,
    pub max_directory: usize,
    pub confident_after: usize,
    pub deadline: Instant,
    ranked: BTreeMap<PathBuf, (usize, usize, usize, usize, usize, PathBuf)>,
    visited: BTreeSet<PathBuf>,
    folders: usize,
    entries: usize,
    stopped: Option<&'static str>,
    capped: bool,
    exact: Option<PathBuf>,
    needle: String,
    as_path: bool,
    hidden: bool,
}
impl Search {
    pub fn new(
        query: String,
        home: PathBuf,
        allowed: PathBuf,
        roots: Vec<PathBuf>,
        excluded: Vec<PathBuf>,
        limit: usize,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            query,
            home,
            allowed,
            roots,
            excluded,
            limit: limit.clamp(1, 100),
            cancelled,
            max_depth: 12,
            max_folders: 20000,
            max_entries: 200000,
            max_directory: 50000,
            confident_after: 5000,
            deadline: Instant::now() + Duration::from_secs(2),
            ranked: BTreeMap::new(),
            visited: BTreeSet::new(),
            folders: 0,
            entries: 0,
            stopped: None,
            capped: false,
            exact: None,
            needle: String::new(),
            as_path: false,
            hidden: false,
        }
    }
    fn refused(&self, path: &Path) -> bool {
        !path.starts_with(&self.allowed)
            || path
                .components()
                .any(|c| c.as_os_str() == ".cortex" || c.as_os_str() == ".cortex-dev")
            || self.excluded.iter().any(|p| path.starts_with(p))
    }
    fn expired(&mut self) -> bool {
        if self.cancelled.load(Ordering::Relaxed) {
            self.stopped = Some("cancelled");
        } else if Instant::now() >= self.deadline {
            self.stopped = Some("deadline");
        }
        self.stopped.is_some()
    }
    fn children(
        &mut self,
        real: &Path,
        visible: &Path,
        hidden: bool,
    ) -> Vec<(PathBuf, PathBuf, bool)> {
        let mut found = Vec::new();
        let Ok(list) = fs::read_dir(real) else {
            return found;
        };
        for (index, entry) in list.enumerate() {
            self.entries += 1;
            if self.entries >= self.max_entries {
                self.stopped = Some("limit");
                break;
            }
            if index + 1 >= self.max_directory {
                self.capped = true;
                break;
            }
            if index % 256 == 0 && self.expired() {
                break;
            }
            let Ok(entry) = entry else { continue };
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if IGNORED.contains(&name)
                || [".cortex", ".cortex-dev"].contains(&name)
                || (name.starts_with('.') && !hidden)
            {
                continue;
            }
            let Ok(meta) = entry.file_type() else {
                continue;
            };
            let link = meta.is_symlink();
            let target = if link {
                let Ok(path) = fs::canonicalize(entry.path()) else {
                    continue;
                };
                path
            } else {
                entry.path()
            };
            if !(meta.is_dir() || link && target.is_dir()) || self.refused(&target) {
                continue;
            }
            found.push((visible.join(name), target, link));
        }
        found.sort();
        found
    }
    fn walk(&mut self, roots: Vec<(PathBuf, PathBuf)>, depth_limit: usize) {
        let mut pending = VecDeque::new();
        for (visible, real) in &roots {
            if self.visited.insert(real.clone()) {
                pending.push_back((visible.clone(), real.clone(), visible.clone(), 1));
            }
        }
        while let Some((visible, real, root, depth)) = pending.pop_front() {
            if self.expired() {
                break;
            }
            for (shown, target, link) in self.children(&real, &visible, self.hidden && depth == 1) {
                self.folders += 1;
                let relative = shown
                    .strip_prefix(&root)
                    .unwrap_or(&shown)
                    .to_string_lossy()
                    .to_lowercase();
                let rank = if self.needle.is_empty() {
                    Some((0, FAR, FAR, FAR))
                } else if self.as_path {
                    path_rank(&self.needle, &relative).map(|(a, b, c)| (a.min(4), FAR, b, c))
                } else {
                    text_rank(&self.needle, &relative)
                };
                if let Some((a, b, c, d)) = rank {
                    let key = (a, b, c, d, depth, shown.clone());
                    let saved = self.ranked.entry(shown.clone()).or_insert(key.clone());
                    if key < *saved {
                        *saved = key;
                    }
                }
                if self.folders >= self.max_folders {
                    self.stopped = Some("limit");
                    break;
                }
                if self.folders >= self.confident_after
                    && !self.needle.is_empty()
                    && self
                        .ranked
                        .values()
                        .any(|k| k.0 < 4 || k.0 == 4 && k.3 <= 2 * self.needle.len())
                {
                    self.stopped = Some("confident");
                    break;
                }
                let outside = link && !roots.iter().any(|(_, r)| target.starts_with(r));
                if depth < depth_limit
                    && !self.visited.contains(&target)
                    && !["/proc", "/sys", "/dev"]
                        .iter()
                        .any(|p| target == Path::new(p))
                    && !outside
                {
                    self.visited.insert(target.clone());
                    pending.push_back((shown, target, root.clone(), depth + 1));
                }
            }
            if self.stopped.is_some() {
                break;
            }
        }
    }
    fn roots(&self) -> Vec<(PathBuf, PathBuf)> {
        self.roots
            .iter()
            .filter_map(|p| {
                fs::canonicalize(p)
                    .ok()
                    .filter(|r| r.is_dir() && !self.refused(r))
                    .map(|r| (p.clone(), r))
            })
            .collect()
    }
    pub fn run(mut self) -> Value {
        let query = self.query.trim().to_owned();
        if !query.is_empty() {
            let typed = if query == "~" || query == "." {
                Some(self.home.clone())
            } else if query.starts_with("~/") || query.starts_with("./") {
                Some(self.home.join(&query[2..]))
            } else if query.starts_with('/') {
                Some(PathBuf::from(&query))
            } else {
                None
            };
            if let Some(path) = typed {
                let path = normalize(&path);
                let real = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if !self.refused(&real) {
                    self.as_path = true;
                    if path.is_dir() {
                        self.exact = Some(path.clone());
                    }
                    if query.ends_with('/') || query == "~" || query == "." {
                        if let Ok(real) = fs::canonicalize(&path) {
                            if real.is_dir() {
                                self.walk(vec![(path, real)], 1);
                            }
                        }
                    } else if let Some(parent) = path.parent() {
                        if let Ok(real) = fs::canonicalize(parent) {
                            if real.is_dir() {
                                self.needle = path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_lowercase();
                                self.hidden = self.needle.starts_with('.');
                                self.walk(vec![(parent.to_path_buf(), real)], self.max_depth);
                            }
                        } else if path.starts_with(&self.home) {
                            self.needle = path
                                .strip_prefix(&self.home)
                                .unwrap()
                                .to_string_lossy()
                                .to_lowercase();
                            self.walk(self.roots(), self.max_depth);
                        }
                    }
                }
            } else {
                self.needle = query.to_lowercase();
                self.walk(self.roots(), self.max_depth);
            }
        }
        let mut ranked = self.ranked.values().cloned().collect::<Vec<_>>();
        ranked.sort();
        let mut paths = ranked.into_iter().map(|k| k.5).collect::<Vec<_>>();
        if let Some(exact) = self.exact {
            paths.retain(|p| p != &exact);
            paths.insert(0, exact);
        }
        paths.truncate(self.limit);
        let stopped = self
            .stopped
            .or(if self.capped { Some("limit") } else { None });
        json!({"entries":paths.iter().map(|p|json!({"path":p,"git":fs::symlink_metadata(p.join(".git")).is_ok()})).collect::<Vec<_>>(),
            "complete":matches!(stopped,None|Some("confident")),"stopped":stopped,"scanned":self.folders})
    }
}
fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => (),
            _ => result.push(part),
        }
    }
    result
}
fn fuzzy(q: &str, text: &str, reset: bool) -> Option<(usize, usize)> {
    let query = q.chars().collect::<Vec<_>>();
    let mut index = 0;
    let mut first = 0;
    let mut last = 0;
    for (i, ch) in text.chars().enumerate() {
        if index == query.len() {
            break;
        }
        if reset && ch.is_whitespace() {
            index = 0;
            continue;
        }
        if query.get(index) == Some(&ch) {
            if index == 0 {
                first = i;
            }
            last = i;
            index += 1;
        }
    }
    if !query.is_empty() && index == query.len() {
        Some((first, last - first + 1))
    } else {
        None
    }
}
fn text_score(q: &str, text: &str) -> Option<(usize, usize, usize)> {
    if q == text {
        return Some((0, 0, 0));
    }
    let word = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let best = text
        .match_indices(q)
        .map(|(i, _)| {
            let starts = i == 0 || !text[..i].chars().next_back().is_some_and(word);
            let end = i + q.len();
            let ends = !text[end..].chars().next().is_some_and(word);
            (
                if starts && ends {
                    1
                } else if i == 0 {
                    2
                } else if starts {
                    3
                } else {
                    4
                },
                i,
                0,
            )
        })
        .min();
    best.or_else(|| fuzzy(q, text, true).map(|(a, b)| (5, a, b)))
}
fn path_rank(q: &str, text: &str) -> Option<(usize, usize, usize)> {
    text_score(q, text).or_else(|| {
        let compact_q = q
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>();
        let kept = text
            .char_indices()
            .filter(|(_, c)| c.is_ascii_alphanumeric())
            .collect::<Vec<_>>();
        let compact_text = kept.iter().map(|(_, c)| c).collect::<String>();
        if compact_q.is_empty() {
            return None;
        }
        text_score(&compact_q, &compact_text)
            .map(|(a, b, c)| (a, kept.get(b).map_or(b, |x| x.0), c))
    })
}
fn text_rank(q: &str, text: &str) -> Option<(usize, usize, usize, usize)> {
    let segments = text.split('/').collect::<Vec<_>>();
    let offset = text.find(q).unwrap_or(FAR);
    let spread = fuzzy(q, segments.last().copied().unwrap_or(""), false)
        .map(|(first, len)| first + len - q.chars().count())
        .unwrap_or(FAR);
    for tier in 0..3 {
        if let Some(index) = segments.iter().position(|s| match tier {
            0 => *s == q,
            1 => s.starts_with(q),
            _ => s.contains(q),
        }) {
            return Some((tier, index, offset, spread));
        }
    }
    if offset < FAR {
        return Some((3, FAR, offset, spread));
    }
    if spread < FAR {
        return Some((4, FAR, offset, spread));
    }
    path_rank(q, text).map(|(a, b, c)| (a.min(4), FAR, b, c))
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "cw-search-parity-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            for path in [
                "Code/cedar-notes/.git",
                "Code/cedar-app",
                "Code/maple",
                "Documents/cedar",
                "Documents/deep/deeper/thing",
            ] {
                fs::create_dir_all(root.join(path)).unwrap();
            }
            Self(root)
        }
        fn search(&self, q: &str) -> Search {
            Search::new(
                q.into(),
                self.0.clone(),
                self.0.clone(),
                vec![self.0.clone()],
                vec![],
                30,
                Arc::new(AtomicBool::new(false)),
            )
        }
        fn paths(&self, q: &str) -> Vec<PathBuf> {
            self.search(q).run()["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| PathBuf::from(r["path"].as_str().unwrap()))
                .collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn typed_paths_hidden_names_and_scattered_fragments_match_the_helper() {
        let f = Fixture::new();
        assert_eq!(
            f.paths("cedar"),
            ["Documents/cedar", "Code/cedar-app", "Code/cedar-notes"].map(|p| f.0.join(p))
        );
        assert_eq!(f.paths("CEDAR-N"), vec![f.0.join("Code/cedar-notes")]);
        assert_eq!(f.paths("cdrnts"), vec![f.0.join("Code/cedar-notes")]);
        for q in ["~", "~/", "."] {
            assert_eq!(
                f.paths(q),
                vec![f.0.clone(), f.0.join("Code"), f.0.join("Documents")]
            );
        }
        assert_eq!(
            f.paths("~/Code/ced"),
            vec![f.0.join("Code/cedar-app"), f.0.join("Code/cedar-notes")]
        );
        assert_eq!(
            f.paths("~/Code/"),
            ["Code", "Code/cedar-app", "Code/cedar-notes", "Code/maple"].map(|p| f.0.join(p))
        );
        assert!(f.paths("~/Cod/maple").contains(&f.0.join("Code/maple")));
        fs::create_dir_all(f.0.join(".config/cedar-hidden")).unwrap();
        assert!(f.paths("cedar-hidden").is_empty());
        assert_eq!(f.paths("~/.conf")[0], f.0.join(".config"));
        assert!(f
            .paths("~/.config/")
            .contains(&f.0.join(".config/cedar-hidden")));
        let empty = f.search("  ").run();
        assert_eq!(empty["scanned"], 0);
        assert_eq!(empty["complete"], true);
    }
    #[test]
    fn symlinks_private_paths_and_build_trees_preserve_confinement() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        symlink(&f.0, f.0.join("Code/maple/back")).unwrap();
        symlink(f.0.join("Code/maple/self"), f.0.join("Code/maple/self")).unwrap();
        symlink(f.0.join("missing"), f.0.join("Code/maple/broken")).unwrap();
        for name in [
            "node_modules",
            "target",
            "dist",
            "__pycache__",
            ".cortex",
            ".cortex-dev",
        ] {
            fs::create_dir_all(f.0.join("Code/maple").join(name).join("cedar-buried")).unwrap();
        }
        assert!(f.paths("cedar-buried").is_empty());
        assert_eq!(f.paths("cedar").len(), 3);
        let private = f.0.join("Code/private");
        fs::create_dir(&private).unwrap();
        let mut s = f.search("private");
        s.excluded.push(private.clone());
        assert_eq!(s.run()["entries"], json!([]));
        let mut s = f.search(&format!("{}/", private.display()));
        s.excluded.push(private);
        assert_eq!(s.run()["entries"], json!([]));
        let mut s = f.search("cedar");
        s.allowed = f.0.join("Code");
        s.roots = vec![s.allowed.clone()];
        assert_eq!(s.run()["entries"].as_array().unwrap().len(), 2);
        let outside = Fixture::new();
        symlink(&outside.0, f.0.join("away")).unwrap();
        let mut s = f.search("away");
        s.allowed = std::env::temp_dir();
        assert_eq!(s.run()["entries"].as_array().unwrap().len(), 1);
        let mut s = f.search("thing");
        s.allowed = std::env::temp_dir();
        assert_eq!(s.run()["entries"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn directory_depth_entry_count_confidence_and_result_limits_are_explicit() {
        let f = Fixture::new();
        for n in 0..40 {
            fs::create_dir_all(f.0.join(format!("Many/cedar-{n:02}"))).unwrap();
        }
        assert_eq!(
            f.search("cedar").run()["entries"].as_array().unwrap().len(),
            30
        );
        let mut s = f.search("cedar");
        s.limit = 5;
        assert_eq!(s.run()["entries"].as_array().unwrap().len(), 5);
        let mut s = f.search("nothing");
        s.max_folders = 10;
        let r = s.run();
        assert_eq!(r["scanned"], 10);
        assert_eq!(r["stopped"], "limit");
        let mut s = f.search("cedar");
        s.confident_after = 5;
        assert_eq!(s.run()["stopped"], "confident");
        let mut s = f.search("thing");
        s.max_directory = 8;
        let r = s.run();
        assert_eq!(r["complete"], false);
        assert!(!r["entries"].as_array().unwrap().is_empty());
        let mut s = f.search("thing");
        s.max_depth = 2;
        assert_eq!(s.run()["entries"], json!([]));
        let mut s = f.search("nothing");
        s.max_entries = 3;
        assert_eq!(s.run()["complete"], false);
    }
    #[test]
    fn ranking_parity() {
        assert!(
            text_rank("cedar", "Documents/cedar".to_lowercase().as_str())
                < text_rank("cedar", "code/cedar-notes")
        );
        assert!(text_rank("cdrnts", "code/cedar-notes").is_some());
        assert!(path_rank("cod/maple", "code/maple").is_some());
        assert!(text_rank("thing", "documents/deep/deeper/thing").is_some());
    }
    #[test]
    fn limits_cancellation_and_private_paths() {
        let root = std::env::temp_dir().join(format!("cw-search-{}", std::process::id()));
        fs::create_dir_all(root.join("code/cedar/.git")).unwrap();
        fs::create_dir_all(root.join(".cortex/cedar-secret")).unwrap();
        let make = || {
            Search::new(
                "cedar".into(),
                root.clone(),
                root.clone(),
                vec![root.clone()],
                vec![],
                30,
                Arc::new(AtomicBool::new(false)),
            )
        };
        let reply = make().run();
        assert_eq!(
            reply["entries"],
            json!([{"path":root.join("code/cedar"),"git":true}])
        );
        let mut search = make();
        search.max_folders = 1;
        assert_eq!(search.run()["complete"], false);
        let search = make();
        search.cancelled.store(true, Ordering::Relaxed);
        assert_eq!(search.run()["stopped"], "cancelled");
        let mut search = make();
        search.deadline = Instant::now();
        assert_eq!(search.run()["stopped"], "deadline");
        fs::remove_dir_all(root).unwrap();
    }
}
