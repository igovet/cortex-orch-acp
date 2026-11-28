//! File-backed publication domain. Callers supply transport-verified task/thread
//! identity; this in-process API is not an authentication or containment boundary.
//! Mirrors store.py:227-280,946-989,1116-1123,1202-1242,1250-1321,1512-1598.
use super::{
    locks::{self, AccessLock},
    publication::{self, file_digest, Identity, PublicationIntent},
    OpenMode, Store,
};
use crate::{error::refuse, CoreError, CoreResult};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Draft {
    pub draft_id: String,
    pub draft_path: PathBuf,
    pub kind: String,
    pub markdown: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Published {
    pub report_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub source_revision: i64,
    pub artifacts: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportMetadata {
    pub report_id: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub author: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub source_revision: i64,
    pub artifacts: serde_json::Value,
}
#[derive(Debug)]
pub struct Catalogue {
    pub reports: Vec<ReportMetadata>,
    pub next_cursor: Option<String>,
}
#[derive(Debug)]
pub struct ContentPage {
    pub markdown: String,
    pub sha256: String,
    pub total_characters: usize,
    pub next_cursor: Option<String>,
}
/// Runtime-owned metadata, not a model-selectable pipeline permission.
pub struct Publication<'a> {
    pub title: &'a str,
    pub summary: &'a str,
    pub author: &'a str,
    pub source_revision: i64,
    pub artifacts: serde_json::Value,
}
pub struct Publisher<'a> {
    store: &'a Store,
}
fn safe(s: &str) -> CoreResult<()> {
    if s.is_empty()
        || s == "."
        || s == ".."
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
    {
        return refuse("unsafe_publication_path");
    }
    Ok(())
}
fn marker(id: &str) -> String {
    format!("Cortex draft ID: `{id}`\n\n")
}
fn template(name: &str) -> CoreResult<&'static str> {
    Ok(match name {
        "general" => include_str!("../../../../report-templates/general.md"),
        "implementation" => include_str!("../../../../report-templates/implementation.md"),
        "investigation" => include_str!("../../../../report-templates/investigation.md"),
        "verification" => include_str!("../../../../report-templates/verification.md"),
        "documentation" => include_str!("../../../../report-templates/documentation.md"),
        "planning" => include_str!("../../../../report-templates/planning.md"),
        "synthesis" => include_str!("../../../../report-templates/synthesis.md"),
        "pipeline" => include_str!("../../../../report-templates/pipeline.md"),
        _ => return refuse("invalid_template"),
    })
}
fn bytes(path: &Path) -> CoreResult<Vec<u8>> {
    use std::io::Read;
    let mut f = locks::open_private(path, false, true)?;
    let mut data = Vec::new();
    f.read_to_end(&mut data)?;
    Ok(data)
}
fn text_file(path: &Path) -> CoreResult<String> {
    String::from_utf8(bytes(path)?).map_err(|_| CoreError::Refused("invalid_utf8"))
}
fn create_file(path: &Path, data: &[u8]) -> CoreResult<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}
fn binding(task: &str) -> String {
    crate::digest(format!("cortex-cursor\0{task}").as_bytes())[..12].into()
}
fn number(mut n: u64) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}
fn parse(s: &str) -> CoreResult<u64> {
    let n = u64::from_str_radix(s, 36).map_err(|_| CoreError::Refused("invalid_cursor"))?;
    if number(n) != s {
        return refuse("invalid_cursor");
    }
    Ok(n)
}
fn request_key(key: &str) -> CoreResult<()> {
    if key.is_empty() || key.chars().count() > 128 {
        return refuse("invalid_request_key");
    }
    Ok(())
}
fn metadata(meta: &Publication<'_>) -> CoreResult<()> {
    for (text, max) in [(meta.title, 200), (meta.summary, 320), (meta.author, 120)] {
        if text.trim().is_empty() || text.chars().count() > max {
            return refuse("invalid_publication");
        }
    }
    let Some(artifacts) = meta.artifacts.as_array() else {
        return refuse("invalid_publication");
    };
    if meta.source_revision < 0 || artifacts.len() > 100 {
        return refuse("invalid_publication");
    }
    let mut typed = std::collections::HashSet::new();
    for artifact in artifacts {
        if artifact.get("kind").is_some() || artifact.get("name").is_some() || artifact.get("data").is_some() {
            let output = crate::deliverables::artifact(artifact)
                .ok_or(CoreError::Refused("invalid_deliverable_artifact"))?;
            if typed.len() >= crate::deliverables::MAX_ITEMS
                || !typed.insert((output.name, output.version)) {
                return refuse("invalid_deliverable_artifact");
            }
            continue;
        }
        let Some(object) = artifact.as_object() else {
            return refuse("invalid_publication");
        };
        if object.len() != 2 {
            return refuse("invalid_publication");
        }
        for (name, max) in [("reference", 4096), ("version", 256)] {
            let Some(text) = object.get(name).and_then(|v| v.as_str()) else {
                return refuse("invalid_publication");
            };
            if text.trim().is_empty() || text.chars().count() > max {
                return refuse("invalid_publication");
            }
        }
    }
    Ok(())
}
fn limit(n: usize, max: usize) -> CoreResult<()> {
    if n == 0 || n > max {
        return refuse("invalid_limit");
    }
    Ok(())
}
fn replay<T: serde::de::DeserializeOwned>(
    db: &Connection,
    scope: &str,
    op: &str,
    key: &str,
    digest: &str,
) -> CoreResult<Option<T>> {
    let row: Option<(String,String)> = db.query_row(
        "SELECT digest,response FROM deliveries WHERE scope=? AND operation=? AND delivery_key=?",
        (scope,op,key), |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((old, response)) = row {
        if old != digest {
            return refuse("delivery_conflict");
        }
        return Ok(Some(serde_json::from_str(&response)?));
    }
    Ok(None)
}
fn receipt(
    db: &Connection,
    scope: &str,
    op: &str,
    key: &str,
    digest: &str,
    content: &str,
    value: &impl Serialize,
) -> CoreResult<()> {
    db.execute(
        "INSERT INTO deliveries VALUES (?,?,?,?,?,?)",
        (
            scope,
            op,
            key,
            digest,
            content,
            serde_json::to_string(value)?,
        ),
    )?;
    Ok(())
}

/// Stage the same crash intent for runtime and worker publication, and record
/// its exact installed digest in the caller's transaction.
fn record_edition(
    db: &Connection,
    task: &str,
    dir: &Path,
    kind: &str,
    markdown: &str,
    meta: &Publication<'_>,
    journal: &mut Option<PathBuf>,
) -> CoreResult<Published> {
    let revision: i64 = db.query_row(
        "SELECT coalesce(max(revision),0) FROM source_revisions WHERE task_id=?",
        [task],
        |row| row.get(0),
    )?;
    if meta.source_revision > revision {
        return refuse("invalid_source_revision");
    }
    let existing: Option<(String, String)> = if kind == "pipeline" {
        db.query_row(
            "SELECT r.id,e.digest FROM reports r JOIN editions e ON e.report_id=r.id
             WHERE r.task_id=? AND r.kind='pipeline' ORDER BY e.sequence DESC LIMIT 1",
            [task],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
    } else {
        None
    };
    if kind == "pipeline" && existing.is_none() {
        // A prior attempt may have created the task directory and stopped
        // before persisting its parent entry. Fence each first edition.
        locks::sync_directory(
            dir.parent()
                .ok_or(CoreError::Refused("unsafe_publication_path"))?,
        )?;
    }
    let report_id = existing.as_ref().map_or_else(
        || format!("r_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
        |(id, _)| id.clone(),
    );
    let filename = if kind == "pipeline" {
        "pipeline.md".to_string()
    } else {
        format!("{report_id}.md")
    };
    let (intent, path) = PublicationIntent::prepare(
        dir,
        &report_id,
        &filename,
        markdown,
        existing.as_ref().map(|(_, digest)| digest.as_str()),
    )?;
    *journal = Some(path);
    publication::checkpoint("after_prepare");
    intent.install(dir)?;
    publication::checkpoint("after_install");
    let size = fs::metadata(dir.join(&filename))?.len();
    if existing.is_none() {
        db.execute(
            "INSERT INTO reports VALUES (?,?,?,?,?)",
            (&report_id, task, kind, &filename, crate::now()),
        )?;
    }
    let size_bytes = i64::try_from(size).map_err(|_| CoreError::Refused("document_too_large"))?;
    let sha256 = intent.digest();
    db.execute(
        "INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES (?,?,?,?,?,?,?)",
        (&report_id, meta.title, meta.summary, meta.author, crate::now(), sha256, size_bytes),
    )?;
    db.execute(
        "INSERT INTO report_provenance VALUES (?,?,?)",
        (
            db.last_insert_rowid(),
            meta.source_revision,
            serde_json::to_string(&meta.artifacts)?,
        ),
    )?;
    db.execute(
        "INSERT INTO task_changes(task_id,kind,reference,created_at) VALUES (?,'report',?,?)",
        (task, &report_id, crate::now()),
    )?;
    Ok(Published {
        report_id,
        sha256: sha256.to_owned(),
        size_bytes: size,
        source_revision: meta.source_revision,
        artifacts: meta.artifacts.clone(),
    })
}

impl<'a> Publisher<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }
    fn directory(&self, task: &str, create: bool) -> CoreResult<PathBuf> {
        safe(task)?;
        self.store.read(|db| {
            if !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)",
                [task],
                |r| r.get::<_, bool>(0),
            )? {
                return refuse("task_not_bound");
            }
            Ok(())
        })?;
        let dir = self.store.key().directory().join(task);
        if create || dir.symlink_metadata().is_ok() {
            locks::private_directory(&dir, create)?;
        }
        Ok(dir)
    }
    /// Access lock precedes operation lock, which precedes SQLite. All domain
    /// reads share this ordering with publication and crash reconciliation.
    fn lock(&self, write: bool) -> CoreResult<(Option<AccessLock>, Option<AccessLock>)> {
        let inspect = self.store.mode == OpenMode::InspectImmutable;
        if inspect && write {
            return refuse("inspection_read_only");
        }
        // Immutable Store already owns the exclusive access lock; it must not
        // need or create the Rust-only publication lock in an older store.
        if inspect {
            return Ok((None, None));
        }
        let access = AccessLock::acquire(
            &self.store.key().directory().join(".access.lock"),
            false,
            false,
            Duration::from_secs(2),
        )?;
        let operation = AccessLock::acquire(
            &self.store.key().directory().join(".publication.lock"),
            true,
            true,
            Duration::from_secs(2),
        )?;
        Ok((Some(access), Some(operation)))
    }
    fn draft(
        &self,
        task: &str,
        owner: &str,
        id: &str,
        published_ok: bool,
    ) -> CoreResult<(PathBuf, String, String)> {
        let row: Option<(String,String,String,String,String,Option<String>)> = self.store.read(|db| Ok(db.query_row(
            "SELECT task_id,owner_thread_id,path,kind,template,published_report_id FROM drafts WHERE id=?",
            [id],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?))?;
        let Some((t, o, p, k, template, published)) = row else {
            return refuse("draft_not_found");
        };
        if t != task || o != owner {
            return refuse("draft_not_owned");
        }
        if published.is_some() && !published_ok {
            return refuse("draft_published");
        }
        let path = PathBuf::from(p);
        let expected = self.store.key().directory().join(if k == "pipeline" {
            "pipeline-drafts"
        } else {
            "draft-reports"
        });
        locks::private_directory(&expected, false)?;
        if path.parent() != Some(expected.as_path())
            || path.file_name().and_then(|n| n.to_str()) != Some(&format!("{id}.md"))
        {
            return refuse("invalid_draft_path");
        }
        locks::regular(&path)?;
        Ok((path, k, template))
    }
    fn reconcile_locked(&self, task: &str, dir: &Path) -> CoreResult<()> {
        if self.store.mode == OpenMode::InspectImmutable {
            return Ok(());
        }
        if !dir.exists() {
            return Ok(());
        }
        let mut staging = Vec::new();
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if name.starts_with(".publication_") && name.ends_with(".json") {
                self.store
                    .read(|db| PublicationIntent::reconcile(dir, &path, db))?;
            } else if self.store.mode != OpenMode::LiveReadOnly && publication::scratch_name(name) {
                staging.push(path);
            }
        }
        // Reconcile durable intents under the publication lock before taking
        // away any private staging file or backup left by an interrupted cleanup.
        let mut removed_staging = false;
        for path in staging {
            if path.try_exists()? {
                locks::regular(&path)?;
                fs::remove_file(path)?;
                removed_staging = true;
            }
        }
        if removed_staging {
            locks::sync_directory(dir)?;
        }
        let pending: Vec<(String, String, String)> = self.store.read(|db| {
            Ok(db
                .prepare(
                    "SELECT draft_id,path,digest FROM pending_source_deletions WHERE task_id=?",
                )?
                .query_map([task], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?)
        })?;
        for (id, p, digest) in pending {
            safe(&id)?;
            let path = PathBuf::from(p);
            let valid = ["draft-reports", "pipeline-drafts"].iter().any(|leaf| {
                path == self
                    .store
                    .key()
                    .directory()
                    .join(leaf)
                    .join(format!("{id}.md"))
            });
            if !valid {
                return refuse("invalid_draft_path");
            }
            if path.symlink_metadata().is_ok() {
                if file_digest(&path)? != digest {
                    return refuse("draft_conflict");
                }
                fs::remove_file(&path)?;
                locks::sync_directory(path.parent().unwrap())?;
            }
            self.store.write(|db| {
                db.execute(
                    "DELETE FROM pending_source_deletions WHERE task_id=? AND draft_id=?",
                    (task, &id),
                )?;
                Ok(())
            })?;
        }
        // Verify the mutable pipeline before further mutation. Immutable report
        // bodies are checked on their own read, not rescanned on catalogue reads.
        let files: Vec<(String,String)> = self.store.read(|db| Ok(db.prepare(
            "SELECT r.filename,e.digest FROM reports r JOIN editions e ON e.report_id=r.id WHERE r.task_id=? AND r.kind='pipeline' AND e.sequence=(SELECT max(sequence) FROM editions WHERE report_id=r.id)")?
            .query_map([task],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?))?;
        for (file, digest) in files {
            safe(&file)?;
            if file_digest(&dir.join(file))? != digest {
                return refuse("file_conflict");
            }
        }
        Ok(())
    }
    pub fn reconcile(&self, task: &str) -> CoreResult<()> {
        let _lock = self.lock(true)?;
        let dir = self.directory(task, false)?;
        self.reconcile_locked(task, &dir)
    }
    /// Publish a runtime-generated edition without making an intermediate editable
    /// draft durable. Existing worker drafts and older runtime receipts retain
    /// their ordinary validation and recovery path.
    pub(crate) fn publish_runtime_pipeline(
        &self,
        task: &str,
        key: &str,
        markdown: &str,
        meta: Publication<'_>,
    ) -> CoreResult<Published> {
        use crate::pipeline::OWNER;
        const OPERATION: &str = "publish_runtime_pipeline";
        if matches!(
            self.store.mode,
            OpenMode::InspectImmutable | OpenMode::LiveReadOnly
        ) {
            return refuse("inspection_read_only");
        }
        request_key(key)?;
        metadata(&meta)?;
        let scope = serde_json::to_string(&(task, OWNER))?;
        let draft_hash =
            crate::digest(serde_json::to_string(&(task, OWNER, "pipeline"))?.as_bytes());
        let legacy = self
            .store
            .read(|db| replay::<Draft>(db, &scope, "create_draft", key, &draft_hash))?;
        if let Some(draft) = legacy {
            self.reconcile(task)?;
            if draft.draft_path.symlink_metadata().is_ok() {
                self.write_draft(
                    task,
                    OWNER,
                    &draft.draft_id,
                    &(marker(&draft.draft_id) + markdown),
                )?;
            }
            return self.publish(
                task,
                OWNER,
                &draft.draft_id,
                &format!("{key}:publish"),
                meta,
            );
        }
        let content = crate::digest(markdown.as_bytes());
        let hash = crate::digest(
            serde_json::to_string(&(
                &content,
                meta.title,
                meta.summary,
                meta.author,
                meta.source_revision,
                &meta.artifacts,
            ))?
            .as_bytes(),
        );
        let _lock = self.lock(true)?;
        let dir = self.directory(task, false)?;
        if !dir.is_dir() {
            locks::private_directory(&dir, true)?;
        }
        self.reconcile_locked(task, &dir)?;
        if let Some(old) = self
            .store
            .read(|db| replay(db, &scope, OPERATION, key, &hash))?
        {
            return Ok(old);
        }
        let mut journal = None;
        let result = self.store.write(|db| {
            let published =
                record_edition(db, task, &dir, "pipeline", markdown, &meta, &mut journal)?;
            receipt(db, &scope, OPERATION, key, &hash, &content, &published)?;
            publication::checkpoint("before_commit");
            Ok(published)
        });
        self.finish_edition(&dir, journal, result.is_ok())?;
        result
    }

    fn finish_edition(
        &self,
        dir: &Path,
        journal: Option<PathBuf>,
        defer_committed_cleanup_sync: bool,
    ) -> CoreResult<()> {
        publication::checkpoint("after_commit");
        if let Some(path) = journal {
            self.store.read(|db| {
                if defer_committed_cleanup_sync {
                    PublicationIntent::reconcile_completed(dir, &path, db)
                } else {
                    PublicationIntent::reconcile(dir, &path, db)
                }
            })?;
        }
        publication::checkpoint("after_cleanup");
        Ok(())
    }
    /// Only the runtime calls this method. Worker routes must expose
    /// create_draft (which refuses the pipeline template), not this method.
    pub fn create_pipeline_draft(&self, task: &str, owner: &str, key: &str) -> CoreResult<Draft> {
        self.create(task, owner, "pipeline", key)
    }
    pub fn create_draft(
        &self,
        task: &str,
        owner: &str,
        template: &str,
        key: &str,
    ) -> CoreResult<Draft> {
        if template == "pipeline" {
            return refuse("runtime_owned_operation");
        }
        self.create(task, owner, template, key)
    }
    fn create(&self, task: &str, owner: &str, templ: &str, key: &str) -> CoreResult<Draft> {
        request_key(key)?;
        let body = template(templ)?;
        let _lock = self.lock(true)?;
        let dir = self.directory(task, true)?;
        self.reconcile_locked(task, &dir)?;
        let scope = serde_json::to_string(&(task, owner))?;
        let hash = crate::digest(serde_json::to_string(&(task, owner, templ))?.as_bytes());
        let drafts = self.store.key().directory().join(if templ == "pipeline" {
            "pipeline-drafts"
        } else {
            "draft-reports"
        });
        locks::private_directory(&drafts, true)?;
        let mut staged = None;
        let result=self.store.write(|db| {
            if let Some(old)=replay(db,&scope,"create_draft",key,&hash)? {return Ok(old);}
            let id=format!("d_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
            let path=drafts.join(format!("{id}.md"));
            let markdown=marker(&id)+body;
            create_file(&path,markdown.as_bytes())?;staged=Some(path.clone());locks::sync_directory(&drafts)?;
            let m=path.symlink_metadata()?;
            db.execute("INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode) VALUES (?,?,?,?,?,?,?,?,?)",
                (&id,task,owner,if templ=="pipeline" {"pipeline"} else {"report"},templ,path.to_str(),crate::now(),m.dev() as i64,m.ino() as i64))?;
            db.execute("INSERT INTO draft_revisions VALUES (?,(SELECT coalesce(max(revision),0) FROM source_revisions WHERE task_id=?))",(&id,task))?;
            let value=Draft {draft_id:id,draft_path:path,kind:if templ=="pipeline" {"pipeline".into()} else {"report".into()},markdown};
            receipt(db,&scope,"create_draft",key,&hash,&crate::digest(value.markdown.as_bytes()),&value)?;Ok(value)
        });
        if result.is_err() {
            if let Some(path) = staged {
                fs::remove_file(path)?;
                locks::sync_directory(&drafts)?;
            }
        }
        result
    }
    pub fn write_draft(&self, task: &str, owner: &str, id: &str, markdown: &str) -> CoreResult<()> {
        let _lock = self.lock(true)?;
        let (path, _, _) = self.draft(task, owner, id, false)?;
        if !markdown.starts_with(&marker(id)) {
            return refuse("draft_marker_missing");
        }
        let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        create_file(&temp, markdown.as_bytes())?;
        fs::rename(&temp, &path)?;
        locks::sync_directory(path.parent().unwrap())?;
        // Atomic editors may replace the inode; ownership/path, not the old
        // inode, is the authority (store.py:956-989).
        let m = path.symlink_metadata()?;
        self.store.write(|db| {
            db.execute(
                "UPDATE drafts SET device=?,inode=? WHERE id=?",
                (m.dev() as i64, m.ino() as i64, id),
            )?;
            Ok(())
        })
    }
    pub fn publish(
        &self,
        task: &str,
        owner: &str,
        id: &str,
        key: &str,
        meta: Publication<'_>,
    ) -> CoreResult<Published> {
        request_key(key)?;
        metadata(&meta)?;
        let _lock = self.lock(true)?;
        let dir = self.directory(task, true)?;
        self.reconcile_locked(task, &dir)?;
        let scope = serde_json::to_string(&(task, owner))?;
        let hash = crate::digest(
            serde_json::to_string(&(
                id,
                meta.title,
                meta.summary,
                meta.author,
                meta.source_revision,
                &meta.artifacts,
            ))?
            .as_bytes(),
        );
        if let Some(old) = self
            .store
            .read(|db| replay::<Published>(db, &scope, "write_report", key, &hash))?
        {
            let saved: (String, String, String, String) =
                self.store.read(|db| {
                    Ok(db.query_row(
                "SELECT path,published_digest,task_id,owner_thread_id FROM drafts WHERE id=?",
                [id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?)
                })?;
            if saved.2 != task || saved.3 != owner {
                return refuse("draft_not_owned");
            }
            let path = PathBuf::from(&saved.0);
            if path.symlink_metadata().is_ok() {
                let (path, _, _) = self.draft(task, owner, id, true)?;
                if file_digest(&path)? != saved.1 {
                    return refuse("draft_conflict");
                }
                fs::remove_file(&path)?;
                locks::sync_directory(path.parent().unwrap())?;
            }
            return Ok(old);
        }
        let (source, kind, templ) = self.draft(task, owner, id, false)?;
        let markdown = text_file(&source)?;
        if !markdown.starts_with(&marker(id)) {
            return refuse("draft_marker_missing");
        }
        for line in template(&templ)?.lines() {
            if ((line.starts_with("<!--") && line.ends_with("-->"))
                || (line.starts_with("{{") && line.ends_with("}}")))
                && markdown.contains(line)
            {
                return refuse("draft_guidance_remaining");
            }
        }
        let content = crate::digest(markdown.as_bytes());
        let mut journal = None;
        let result=self.store.write(|db| {
            let result = record_edition(db, task, &dir, &kind, &markdown, &meta, &mut journal)?;
            db.execute("UPDATE drafts SET published_report_id=?,published_digest=? WHERE id=?",(&result.report_id,&content,id))?;
            db.execute("INSERT INTO pending_source_deletions VALUES (?,?,?,?)",(task,id,source.to_str(),&content))?;
            receipt(db,&scope,"write_report",key,&hash,&content,&result)?;Ok(result)
        });
        // Reconcile both commit and rollback paths from the same persisted intent.
        self.finish_edition(&dir, journal, false)?;
        if result.is_ok() {
            self.reconcile_locked(task, &dir)?;
        }
        result
    }
    pub fn own_drafts(
        &self,
        task: &str,
        owner: &str,
        after: &str,
        count: usize,
    ) -> CoreResult<Vec<String>> {
        limit(count, 100)?;
        self.store.read(|db| Ok(db.prepare("SELECT id FROM drafts WHERE task_id=? AND owner_thread_id=? AND published_report_id IS NULL AND id>? ORDER BY id LIMIT ?")?
            .query_map((task,owner,after,count as i64),|r|r.get(0))?.collect::<Result<_,_>>()?))
    }
    pub fn list(&self, task: &str, cursor: Option<&str>, count: usize) -> CoreResult<Catalogue> {
        limit(count, 100)?;
        let _lock = self.lock(false)?;
        let dir = self.directory(task, false)?;
        self.reconcile_locked(task, &dir)?;
        self.catalogue(task, cursor, count, false)
    }
    /// Committed immutable reports only. A reader in another task cannot recover
    /// publication intents or clean the author's private drafts.
    pub fn immutable_reports(
        &self,
        task: &str,
        cursor: Option<&str>,
        count: usize,
    ) -> CoreResult<Catalogue> {
        limit(count, 100)?;
        self.catalogue(task, cursor, count, true)
    }
    fn catalogue(
        &self,
        task: &str,
        cursor: Option<&str>,
        count: usize,
        immutable_only: bool,
    ) -> CoreResult<Catalogue> {
        self.store.read(|db| {
            let ceiling:u64=db.query_row("SELECT coalesce(max(e.sequence),0) FROM editions e JOIN reports r ON r.id=e.report_id WHERE r.task_id=?",[task],|r|r.get::<_,i64>(0))? as u64;
            let (before,high)=if let Some(c)=cursor {
                let p:Vec<_>=c.split('.').collect();
                if p.len()!=4 || p[0]!="cl" || p[1]!=binding(task) {return refuse("invalid_cursor");}
                let b=parse(p[2])?;let h=parse(p[3])?;
                if b==0 || h>ceiling || b>h.saturating_add(1) {return refuse("invalid_cursor");} (b,h)
            } else {(ceiling+1,ceiling)};
            let mut stmt=db.prepare("SELECT r.id,r.kind,e.title,e.summary,e.author,e.digest,e.size_bytes,coalesce(p.source_revision,0),coalesce(p.artifacts,'[]'),e.sequence FROM reports r JOIN editions e ON e.report_id=r.id LEFT JOIN report_provenance p ON p.edition_sequence=e.sequence WHERE r.task_id=? AND r.kind IN ('report','pipeline') AND (NOT ? OR r.kind='report') AND e.sequence=(SELECT max(sequence) FROM editions WHERE report_id=r.id AND sequence<=?) AND e.sequence<? ORDER BY e.sequence DESC LIMIT ?")?;
            let mut rows=stmt.query((task,immutable_only,high as i64,before as i64,(count+1) as i64))?;let mut items=Vec::new();
            while let Some(r)=rows.next()? {
                items.push((ReportMetadata {report_id:r.get(0)?,kind:r.get(1)?,title:r.get(2)?,summary:r.get(3)?,author:r.get(4)?,sha256:r.get(5)?,size_bytes:r.get::<_,i64>(6)? as u64,source_revision:r.get(7)?,artifacts:serde_json::from_str(&r.get::<_,String>(8)?)?},r.get::<_,i64>(9)? as u64));
            }
            let more=items.len()>count;items.truncate(count);
            let next_cursor=if more {Some(format!("cl.{}.{}.{}",binding(task),number(items.last().unwrap().1),number(high)))} else {None};
            Ok(Catalogue {reports:items.into_iter().map(|v|v.0).collect(),next_cursor})
        })
    }
    pub fn read_report(
        &self,
        task: &str,
        id: &str,
        cursor: Option<&str>,
        count: usize,
    ) -> CoreResult<ContentPage> {
        limit(count, 4000)?;
        let _lock = self.lock(false)?;
        let dir = self.directory(task, false)?;
        self.reconcile_locked(task, &dir)?;
        self.published_page(task, id, cursor, count, &dir, false, None)
    }
    /// Read only a committed, digest-matching report; never follow a caller's
    /// filesystem path or perform recovery effects on the publishing task.
    pub fn immutable_report_page(
        &self,
        task: &str,
        id: &str,
        cursor: Option<&str>,
        count: usize,
    ) -> CoreResult<ContentPage> {
        limit(count, 4000)?;
        let dir = self.directory(task, false)?;
        self.published_page(task, id, cursor, count, &dir, true, None)
    }
    /// A bounded initial page positioned by Unicode character offset in an immutable report.
    pub fn immutable_report_from(
        &self, task: &str, id: &str, start: u64, count: usize,
    ) -> CoreResult<ContentPage> {
        limit(count, 4000)?;
        let dir = self.directory(task, false)?;
        self.published_page(task, id, None, count, &dir, true, Some(start))
    }
    /// Verify the bytes of the exact committed report selected in the caller's
    /// current SQLite snapshot. The normal immutable content reader validates
    /// every byte before returning the bounded first page.
    pub fn verify_immutable_report_on(
        &self, db: &Connection, task: &str, id: &str, expected: &str,
    ) -> CoreResult<()> {
        safe(task)?;
        let dir = self.store.key().directory().join(task);
        locks::private_directory(&dir, false)?;
        let (filename, digest): (String, String) = db.query_row(
            "SELECT r.filename,e.digest FROM reports r JOIN editions e ON e.report_id=r.id
             WHERE r.task_id=? AND r.id=? AND r.kind='report' AND
                   (SELECT count(*) FROM editions WHERE report_id=r.id)=1",
            (task, id), |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?.ok_or(CoreError::Refused("not_found"))?;
        if digest != expected {
            return refuse("file_conflict");
        }
        safe(&filename)?;
        page(task, id, "cr", &dir.join(filename), Some(expected), None, 1, "invalid_cursor")?;
        Ok(())
    }
    fn published_page(
        &self,
        task: &str,
        id: &str,
        cursor: Option<&str>,
        count: usize,
        dir: &Path,
        immutable_only: bool,
        start: Option<u64>,
    ) -> CoreResult<ContentPage> {
        let row:Option<(String,String,String)>=self.store.read(|db| Ok(db.query_row(
            "SELECT r.filename,r.kind,e.digest FROM reports r JOIN editions e ON e.report_id=r.id WHERE r.task_id=? AND r.id=? AND (NOT ? OR r.kind='report') ORDER BY e.sequence DESC LIMIT 1",
            (task,id,immutable_only),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?))?;
        let Some((filename, kind, digest)) = row else {
            return refuse("not_found");
        };
        safe(&filename)?;
        let positioned = start.map(|start| {
            Ok::<_, CoreError>(format!("cr.{}.{}.{}.{}", binding(task),
                id.strip_prefix("r_").ok_or(CoreError::Refused("invalid_cursor"))?,
                digest.get(..12).ok_or(CoreError::Refused("file_conflict"))?, number(start)))
        }).transpose()?;
        page(
            task,
            id,
            "cr",
            &dir.join(filename),
            Some(&digest),
            cursor.or(positioned.as_deref()),
            count,
            if kind == "pipeline" {
                "cursor_stale"
            } else {
                "invalid_cursor"
            },
        )
        .map_err(|error| {
            if error.code() == "invalid_utf8" {
                CoreError::Refused("file_conflict")
            } else {
                error
            }
        })
    }
    pub fn read_draft(
        &self,
        task: &str,
        owner: &str,
        id: &str,
        cursor: Option<&str>,
        count: usize,
    ) -> CoreResult<ContentPage> {
        limit(count, 4000)?;
        let _lock = self.lock(false)?;
        let (path, _, _) = self.draft(task, owner, id, false)?;
        page(
            task,
            id,
            "cd",
            &path,
            None,
            cursor,
            count,
            "draft_cursor_stale",
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lifecycle::TaskMachine,
        store::{ProjectKey, StoreChannel},
    };
    use std::{os::unix::fs::DirBuilderExt, process::Command};
    fn key(root: &Path) -> ProjectKey {
        ProjectKey::new(root, StoreChannel::Dev).unwrap()
    }
    fn meta() -> Publication<'static> {
        Publication {
            title: "title",
            summary: "summary",
            author: "runtime",
            source_revision: 0,
            artifacts: serde_json::json!([]),
        }
    }
    #[test]
    fn publication_crash_child() {
        let Ok(root) = std::env::var("CORTEX_CORE_PUBLICATION_CRASH_ROOT") else {
            return;
        };
        let root = Path::new(&root);
        let s = Store::open(key(root), OpenMode::Initialize).unwrap();
        let api = Publisher::new(&s);
        if std::env::var("CORTEX_CORE_PUBLICATION_RUNTIME").as_deref() == Ok("1") {
            api.publish_runtime_pipeline("t", "second", "new", meta()).unwrap();
            panic!("runtime checkpoint did not run");
        }
        let id = api
            .own_drafts("t", "owner", "", 100)
            .unwrap()
            .pop()
            .unwrap();
        api.publish("t", "owner", &id, "second", meta()).unwrap();
        panic!("checkpoint did not run");
    }
    // Mirrors test_markdown_store.py::test_atomic_rollback_and_immutable_reports,
    // test_pipeline_newest_first_cursor_and_snapshot and post-commit recovery,
    // with real abrupt process exit instead of only raised exceptions.
    #[test]
    fn publication_crash_reconciles_exact_edition_and_delivery() {
        for point in ["after_prepare", "after_install", "after_commit"] {
            let root = std::env::temp_dir()
                .join(format!("cortex-publication-crash-{}", uuid::Uuid::new_v4()));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let s = Store::open(key(&root), OpenMode::Initialize).unwrap();
            TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
            let api = Publisher::new(&s);
            let old = api.create_pipeline_draft("t", "owner", "first").unwrap();
            api.write_draft(
                "t",
                "owner",
                &old.draft_id,
                &(marker(&old.draft_id) + "old"),
            )
            .unwrap();
            let first = api
                .publish("t", "owner", &old.draft_id, "first", meta())
                .unwrap();
            let new = api.create_pipeline_draft("t", "owner", "second").unwrap();
            api.write_draft(
                "t",
                "owner",
                &new.draft_id,
                &(marker(&new.draft_id) + "new"),
            )
            .unwrap();
            drop(s);
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::documents::tests::publication_crash_child",
                    "--nocapture",
                ])
                .env("CORTEX_CORE_PUBLICATION_CRASH_ROOT", &root)
                .env("CORTEX_CORE_PUBLICATION_CRASH_POINT", point)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(98));
            let s = Store::open(key(&root), OpenMode::Initialize).unwrap();
            let api = Publisher::new(&s);
            api.reconcile("t").unwrap();
            let page = api.read_report("t", &first.report_id, None, 4000).unwrap();
            if point == "after_commit" {
                assert!(page.markdown.starts_with(&(marker(&new.draft_id) + "new")));
            } else {
                assert_eq!(page.markdown, marker(&old.draft_id) + "old");
            }
            let receipt = api
                .publish("t", "owner", &new.draft_id, "second", meta())
                .unwrap();
            assert_eq!(receipt.report_id, first.report_id);
            assert_eq!(
                s.read(|db| Ok(
                    db.query_row("SELECT count(*) FROM editions", [], |r| r.get::<_, i64>(0))?
                ))
                .unwrap(),
                2
            );
            drop(s);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn direct_runtime_publication_recovers_after_each_crash_boundary() {
        for point in [
            "before_staging",
            "after_staging",
            "after_intent_write",
            "after_intent_rename",
            "after_prepare",
            "after_backup",
            "after_replace",
            "after_install",
            "before_commit",
            "after_commit",
            "during_cleanup",
            "after_cleanup",
        ] {
            let root = std::env::temp_dir()
                .join(format!("cortex-pipeline-crash-{}", uuid::Uuid::new_v4()));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let first = {
                let store = Store::open(key(&root), OpenMode::Initialize).unwrap();
                TaskMachine::new(&store)
                    .create("t", "goal", "owner")
                    .unwrap();
                Publisher::new(&store)
                    .publish_runtime_pipeline("t", "first", "old", meta())
                    .unwrap()
            };
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::documents::tests::publication_crash_child",
                ])
                .env("CORTEX_CORE_PUBLICATION_CRASH_ROOT", &root)
                .env("CORTEX_CORE_PUBLICATION_RUNTIME", "1")
                .env("CORTEX_CORE_PUBLICATION_CRASH_POINT", point)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(98), "{point}");
            {
                let store = Store::open(key(&root), OpenMode::Initialize).unwrap();
                let api = Publisher::new(&store);
                let page = api.read_report("t", &first.report_id, None, 100).unwrap();
                let committed =
                    matches!(point, "after_commit" | "during_cleanup" | "after_cleanup");
                let updated = "new\n\n---\n\nold";
                assert_eq!(
                    page.markdown,
                    if committed { updated } else { "old" },
                    "{point}"
                );
                let result = api
                    .publish_runtime_pipeline("t", "second", "new", meta())
                    .unwrap();
                assert_eq!(result.report_id, first.report_id, "{point}");
                assert_eq!(result.sha256, crate::digest(updated.as_bytes()), "{point}");
                let replay = api
                    .publish_runtime_pipeline("t", "second", "new", meta())
                    .unwrap();
                assert_eq!(replay, result, "{point}");
                assert_eq!(
                    api.publish_runtime_pipeline("t", "second", "different", meta())
                        .unwrap_err()
                        .code(),
                    "delivery_conflict",
                    "{point}"
                );
                let counts = store.read(|db| {
                    Ok(db.query_row(
                        "SELECT (SELECT count(*) FROM reports), (SELECT count(*) FROM editions), (SELECT count(*) FROM drafts)",
                        [],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
                    )?)
                }).unwrap();
                assert_eq!(counts, (1, 2, 0), "{point}");
                let files: Vec<_> = fs::read_dir(store.key().directory().join("t"))
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect();
                assert_eq!(files, ["pipeline.md"], "{point}");
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn a_pending_legacy_runtime_draft_finishes_under_its_original_receipt() {
        let root =
            std::env::temp_dir().join(format!("cortex-pipeline-legacy-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        {
            let store = Store::open(key(&root), OpenMode::Initialize).unwrap();
            TaskMachine::new(&store)
                .create("t", "goal", "owner")
                .unwrap();
            let api = Publisher::new(&store);
            let old = api
                .create_pipeline_draft("t", crate::pipeline::OWNER, "legacy")
                .unwrap();
            let result = api
                .publish_runtime_pipeline("t", "legacy", "body", meta())
                .unwrap();
            assert!(!old.draft_path.exists());
            let page = api.read_report("t", &result.report_id, None, 100).unwrap();
            assert_eq!(page.markdown, marker(&old.draft_id) + "body");
            assert_eq!(
                api.publish_runtime_pipeline("t", "legacy", "body", meta())
                    .unwrap(),
                result
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
/// Offset already verified by the immutable page reader. Used in a page receipt.
pub fn report_cursor_offset(cursor: &str) -> CoreResult<u64> {
    parse(cursor.rsplit('.').next().ok_or(CoreError::Refused("invalid_cursor"))?)
}
fn page(
    task: &str,
    id: &str,
    tag: &str,
    path: &Path,
    committed: Option<&str>,
    cursor: Option<&str>,
    count: usize,
    stale: &'static str,
) -> CoreResult<ContentPage> {
    let mut start = 0;
    let mut expected = None;
    if let Some(c) = cursor {
        let p: Vec<_> = c.split('.').collect();
        if p.len() != 5
            || p[0] != tag
            || p[1] != binding(task)
            || format!("{}_{}", if tag == "cr" { "r" } else { "d" }, p[2]) != id
        {
            return refuse("invalid_cursor");
        }
        expected = Some(p[3].to_owned());
        start = usize::try_from(parse(p[4])?).map_err(|_| CoreError::Refused("invalid_cursor"))?;
    }
    // Validate/hash the entire document, retaining only one bounded Unicode page.
    // A multibyte character may cross a read boundary; retain at most three bytes.
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = locks::open_private(path, false, true)?;
    let identity = file.metadata()?;
    let fast = if committed.is_some() {
        cached_page(path, &mut file, &identity, committed, start, count)?
    } else {
        None
    };
    let (markdown, total, digest) = match fast {
        Some(v) => v,
        None => {
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut tail = Vec::new();
    let mut total = 0;
    let mut selected = 0;
    let mut markdown = String::new();
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        tail.extend_from_slice(&buffer[..n]);
        let valid = match std::str::from_utf8(&tail) {
            Ok(_) => tail.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return refuse("invalid_utf8"),
        };
        let text =
            std::str::from_utf8(&tail[..valid]).map_err(|_| CoreError::Refused("invalid_utf8"))?;
        for c in text.chars() {
            if total >= start && selected < count {
                markdown.push(c);
                selected += 1;
            }
            total += 1;
        }
        tail.drain(..valid);
    }
    if !tail.is_empty() {
        return refuse("invalid_utf8");
    }
    let after = file.metadata()?;
    let named = path.symlink_metadata()?;
    if identity.dev() != named.dev()
        || identity.ino() != named.ino()
        || identity.len() != after.len()
        || identity.mtime_nsec() != after.mtime_nsec()
        || identity.mtime() != after.mtime()
        || identity.ctime_nsec() != after.ctime_nsec()
        || identity.ctime() != after.ctime()
    {
        return refuse("file_conflict");
    }
    let digest = format!("{:x}", hash.finalize());
    if committed == Some(digest.as_str()) {
        publication::remember(path, Identity::of(&identity), digest.clone(), Some(total), false);
    }
    (markdown, total, digest)
        }
    };
    if committed.is_some_and(|d| d != digest) {
        return refuse("file_conflict");
    }
    if expected.as_deref().is_some_and(|d| d != &digest[..12]) {
        return refuse(stale);
    }
    if start > total {
        return refuse("invalid_cursor");
    }
    let end = start + markdown.chars().count();
    let next_cursor = if end < total {
        Some(format!(
            "{tag}.{}.{}.{}.{}",
            binding(task),
            id.strip_prefix(if tag == "cr" { "r_" } else { "d_" })
                .ok_or(CoreError::Refused("invalid_cursor"))?,
            &digest[..12],
            number(end as u64)
        ))
    } else {
        None
    };
    Ok(ContentPage {
        markdown,
        sha256: digest,
        total_characters: total,
        next_cursor,
    })
}
/// The page of a published document whose digest and length were verified in exactly its
/// current state (`publication::verified`): only the requested characters are read.
/// `None` when the file is not known to be in such a state.
fn cached_page(
    path: &Path,
    file: &mut fs::File,
    identity: &fs::Metadata,
    committed: Option<&str>,
    start: usize,
    count: usize,
) -> CoreResult<Option<(String, usize, String)>> {
    use std::io::Read;
    let state = Identity::of(identity);
    let Some((digest, Some(total))) = publication::verified(path, &state) else {
        return Ok(None);
    };
    if committed.is_some_and(|d| d != digest) || start > total {
        return Ok(None);
    }
    let (mut skipped, mut chosen, mut inside) = (0usize, 0usize, false);
    let mut page = Vec::new();
    let mut buffer = [0u8; 65536];
    'read: loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        let chunk = &buffer[..n];
        let starts = chunk.iter().filter(|b| (**b & 0xC0) != 0x80).count();
        if page.is_empty() && !inside && skipped + starts <= start {
            skipped += starts;
            continue;
        }
        for &b in chunk {
            if (b & 0xC0) != 0x80 {
                if skipped < start {
                    skipped += 1;
                    continue;
                }
                if chosen == count {
                    break 'read;
                }
                chosen += 1;
                inside = true;
            }
            if inside {
                page.push(b);
            }
        }
    }
    let after = file.metadata()?;
    if Identity::of(&after) != state || Identity::of(&path.symlink_metadata()?) != state {
        return Ok(None);
    }
    let markdown = String::from_utf8(page).map_err(|_| CoreError::Refused("invalid_utf8"))?;
    Ok(Some((markdown, total, digest)))
}
