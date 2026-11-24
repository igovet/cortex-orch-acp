//! Cooperative exclusion for the local Code editor and managed writers.
//! These are runtime-only primitives, not a filesystem lock or transport authorization.
//! Mutation/admission calls belong in Store::write; the caller owns process observation,
//! the final bounded effect, authenticated session identity and post-commit wakeups.
use crate::{digest, error::refuse, ports::ProcessObservation, CoreError, CoreResult};
use rusqlite::{Connection, OptionalExtension, Transaction};
use std::{
    ffi::CString,
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::Path,
};

pub(crate) const SCHEMA: &str = include_str!("owner_file_leases.sql");
pub const RENEW_INTERVAL_SECONDS: u64 = 20;
pub const TTL_SECONDS: u64 = 60;
const TTL_MS: i64 = TTL_SECONDS as i64 * 1000;
const UNCONFIRMED: CoreError = CoreError::Refused("writer_unconfirmed");
const LOST: CoreError = CoreError::Refused("edit_lease_lost");
const MAX_ACTIVE: usize = 10_000;

// The boot-qualified epoch is copied into each lease row and preserved on
// renewal. Legacy unqualified epochs are expired, without a format change.
fn boot_id() -> CoreResult<&'static str> {
    static BOOT_ID: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    BOOT_ID
        .get_or_init(|| {
            let raw = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
            Some(uuid::Uuid::parse_str(raw.trim()).ok()?.simple().to_string())
        })
        .as_deref()
        .ok_or(UNCONFIRMED)
}

// Suspend counts towards expiry and a wall-clock step cannot revive a token.
// Only durations (not these boot-relative timestamps) should leave the runtime.
fn now_ms() -> CoreResult<i64> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) } != 0 {
        return Err(UNCONFIRMED);
    }
    ts.tv_sec
        .checked_mul(1000)
        .and_then(|ms| ms.checked_add(ts.tv_nsec / 1_000_000))
        .ok_or(UNCONFIRMED)
}

fn entry(parent: &File, name: &str) -> std::io::Result<File> {
    let name = CString::new(name)?;
    // O_PATH never opens a device/FIFO for I/O. Metadata on the fd detects a link
    // when O_NOFOLLOW returns a descriptor referring to the link itself.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    canonical: String,
    device: i64,
    inode: i64,
}
impl Checkout {
    /// Call with the trusted, resolved effect root, not a path from an editor DTO.
    pub fn identify(root: &Path) -> CoreResult<Self> {
        let canonical = root.canonicalize().map_err(|_| UNCONFIRMED)?;
        let canonical = canonical.to_str().ok_or(UNCONFIRMED)?.to_owned();
        let dir = Self::open_dir(&canonical)?;
        let metadata = dir.metadata().map_err(|_| UNCONFIRMED)?;
        Ok(Self {
            canonical,
            device: metadata.dev() as i64,
            inode: metadata.ino() as i64,
        })
    }

    fn open_dir(canonical: &str) -> CoreResult<File> {
        // Walk the canonical name with no-follow on *every* component.
        let mut dir = File::open("/").map_err(|_| UNCONFIRMED)?;
        let rest = canonical.strip_prefix('/').ok_or(UNCONFIRMED)?;
        if !rest.is_empty() {
            for name in rest.split('/') {
                if name.is_empty() || matches!(name, "." | "..") {
                    return Err(UNCONFIRMED);
                }
                dir = entry(&dir, name).map_err(|_| UNCONFIRMED)?;
                if !dir.metadata().map_err(|_| UNCONFIRMED)?.is_dir() {
                    return Err(UNCONFIRMED);
                }
            }
        }
        Ok(dir)
    }

    fn recheck(&self) -> CoreResult<File> {
        let dir = Self::open_dir(&self.canonical)?;
        let m = dir.metadata().map_err(|_| UNCONFIRMED)?;
        if (m.dev() as i64, m.ino() as i64) != (self.device, self.inode) {
            return Err(UNCONFIRMED);
        }
        Ok(dir)
    }

    fn check_path(&self, path: &RelativePath, file_only: bool) -> CoreResult<()> {
        let mut dir = self.recheck()?;
        let mut parts = path.0.split('/').peekable();
        while let Some(name) = parts.next() {
            let last = parts.peek().is_none();
            let next = match entry(&dir, name) {
                Ok(next) => next,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && (last || !file_only) => {
                    return Ok(())
                }
                Err(_) => return refuse("unsafe_edit_path"),
            };
            let m = next.metadata().map_err(|_| UNCONFIRMED)?;
            if last && m.is_file() && m.nlink() == 1 {
                return Ok(());
            }
            if !m.is_dir() || last && file_only {
                return refuse("unsafe_edit_path");
            }
            if last {
                return Ok(());
            }
            dir = next;
        }
        refuse("unsafe_edit_path")
    }

    pub fn path(&self) -> &Path {
        Path::new(&self.canonical)
    }

    fn same_directory(&self, other: &Self) -> bool {
        (self.device, self.inode) == (other.device, other.inode)
    }

    fn comparable(&self, other: &Self) -> CoreResult<()> {
        // Replacement isn't proof of disjoint authority (an opaque writer can
        // still use the old textual path).
        if self.canonical == other.canonical && !self.same_directory(other) {
            return Err(UNCONFIRMED);
        }
        Ok(())
    }
}

/// Canonical UTF-8 checkout-relative path (not a glob). Validation of the live
/// filesystem is separate and repeated under the transaction.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize)]
#[serde(transparent)]
pub struct RelativePath(String);
impl RelativePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn new(path: &str) -> CoreResult<Self> {
        if path.len() > 4096
            || path.contains(['\\', '\0'])
            || path.split('/').any(|part| {
                matches!(
                    part,
                    "" | "."
                        | ".."
                        | ".git"
                        | ".cortex"
                        | ".cortex-dev"
                        | ".cortex-dev-owner.json"
                        | ".cortex-workspace-dev.json"
                )
            })
        {
            return refuse("unsafe_edit_path");
        }
        Ok(Self(path.to_owned()))
    }
}

// Only a single current editor authority may activate a project. The service
// must serialize activation against its own lifecycle (never activate lazily
// from an editor request), and pass the same value for that service lifetime.
pub struct EditorEpoch(uuid::Uuid);
impl EditorEpoch {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
    fn key(&self) -> CoreResult<String> {
        Ok(format!("{}:{}", boot_id()?, self.0.simple()))
    }
}
impl Default for EditorEpoch {
    fn default() -> Self {
        Self::new()
    }
}

pub fn activate_editor_tx(db: &Transaction<'_>, epoch: &EditorEpoch) -> CoreResult<()> {
    let key = epoch.key()?;
    db.execute(
        "INSERT INTO code_editor_epoch VALUES (1,1,?)
         ON CONFLICT(singleton) DO UPDATE SET epoch=excluded.epoch WHERE epoch!=excluded.epoch",
        [&key],
    )?;
    db.execute(
        "UPDATE code_owner_file_leases SET state='released',released_generation=acquired_generation+1
         WHERE state='active' AND epoch!=?",
        [&key],
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
pub struct Editor<'a> {
    pub epoch: &'a EditorEpoch,
    /// Trusted session ID, not an actor ID supplied by the remote peer.
    pub session: &'a str,
}
impl Editor<'_> {
    fn check(self, db: &Connection) -> CoreResult<String> {
        let key = self.epoch.key()?;
        if !valid_id(self.session)
            || db
                .query_row(
                    "SELECT epoch FROM code_editor_epoch WHERE singleton=1 AND record_version=1",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .as_ref()
                != Some(&key)
        {
            return Err(LOST);
        }
        Ok(key)
    }
}

// Deliberately neither Debug nor Serialize: only the authenticated acquire
// reply should expose the bearer value, never tracing or receipts.
pub struct LeaseToken(String);
impl LeaseToken {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Available,
    Yours { remaining_ms: u64 },
    OtherEditor { remaining_ms: u64 },
}

#[derive(Clone, Debug)]
pub struct OwnerBlocker {
    pub path: RelativePath,
    pub remaining_ms: u64,
}

struct Active {
    checkout: Checkout,
    path: RelativePath,
    expiry: i64,
    holder: String,
}

fn active(db: &Connection, now: i64) -> CoreResult<Vec<Active>> {
    let mut query = db.prepare(
        "SELECT checkout,device,inode,path,expires_at,holder FROM code_owner_file_leases
         WHERE state='active' AND epoch=(SELECT epoch FROM code_editor_epoch WHERE singleton=1)
         AND epoch GLOB ? AND expires_at>? ORDER BY expires_at, checkout, path LIMIT ?",
    )?;
    let mut rows = query.query((format!("{}:*", boot_id()?), now, (MAX_ACTIVE + 1) as i64))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        if out.len() == MAX_ACTIVE {
            return Err(UNCONFIRMED);
        }
        out.push(Active {
            checkout: Checkout {
                canonical: r.get(0)?,
                device: r.get(1)?,
                inode: r.get(2)?,
            },
            path: RelativePath::new(&r.get::<_, String>(3)?).map_err(|_| UNCONFIRMED)?,
            expiry: r.get(4)?,
            holder: r.get(5)?,
        });
    }
    Ok(out)
}

/// Broad includes opaque writers, even if their assignment declares paths.
/// FileConfined is only for runtime-enforced destinations (no opaque capability).
#[derive(Clone, Copy)]
pub enum Coverage<'a> {
    Broad,
    FileConfined(&'a [RelativePath]),
}

fn tree_overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn covers(
    checkout: &Checkout,
    coverage: Coverage<'_>,
    file_root: &Checkout,
    file: &RelativePath,
) -> CoreResult<bool> {
    checkout.comparable(file_root)?;
    let alias = checkout.same_directory(file_root);
    let file_path = file_root.path().join(&file.0);
    Ok(match coverage {
        Coverage::Broad => alias || tree_overlap(checkout.path(), &file_path),
        Coverage::FileConfined(paths) => paths.iter().any(|p| {
            if alias {
                tree_overlap(Path::new(&p.0), Path::new(&file.0))
            } else {
                tree_overlap(&checkout.path().join(&p.0), &file_path)
            }
        }),
    })
}

fn find_owner(
    db: &Connection,
    checkout: &Checkout,
    coverage: Coverage<'_>,
    now: i64,
) -> CoreResult<Option<Active>> {
    for row in active(db, now)? {
        if covers(checkout, coverage, &row.checkout, &row.path)? {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

fn check_coverage(checkout: &Checkout, coverage: Coverage<'_>) -> CoreResult<()> {
    checkout.recheck()?;
    if let Coverage::FileConfined(paths) = coverage {
        if paths.is_empty() || paths.len() > 64 {
            return Err(UNCONFIRMED);
        }
        for path in paths {
            checkout.check_path(path, false)?;
        }
    }
    Ok(())
}

/// Same-transaction structured-effect fence (also catches ancestors/descendants).
/// Call immediately before the effect; it does not replace worker permissions.
pub fn check_worker_effect_tx(
    db: &Transaction<'_>,
    checkout: &Checkout,
    coverage: Coverage<'_>,
) -> CoreResult<()> {
    if let Some(held) = writer_blocker(db, checkout, coverage)? {
        return Err(CoreError::OwnerFileLeaseHeld { path: held.path });
    }
    Ok(())
}

/// Fence exact structured destinations, including parent directories the
/// caller may create or remove. The caller validates and pins its effect paths;
/// this does not impose editor-only filesystem rules on worker tools.
pub fn validate_worker_paths_tx(
    db: &Transaction<'_>,
    checkout: &Checkout,
    paths: &[RelativePath],
) -> CoreResult<()> {
    checkout.recheck()?;
    if paths.is_empty() || paths.len() > 64 {
        return Err(UNCONFIRMED);
    }
    if let Some(row) = find_owner(db, checkout, Coverage::FileConfined(paths), now_ms()?)? {
        return Err(CoreError::OwnerFileLeaseHeld { path: row.path });
    }
    Ok(())
}

/// Remaining time until the earliest conflicting lease expires. Re-evaluate
/// following a post-commit release/renewal notification or at this deadline.
pub fn writer_blocker(
    db: &Connection,
    checkout: &Checkout,
    coverage: Coverage<'_>,
) -> CoreResult<Option<OwnerBlocker>> {
    check_coverage(checkout, coverage)?;
    let now = now_ms()?;
    Ok(
        find_owner(db, checkout, coverage, now)?.map(|row| OwnerBlocker {
            remaining_ms: row.expiry.saturating_sub(now) as u64,
            path: row.path,
        }),
    )
}

// With no live leases there is nothing to arbitrate. In particular, do
// not turn a scheduler projection for an unmaterialized checkout into a
// filesystem operation solely because the optional schema is installed.
pub(crate) fn scheduler_blocker(
    db: &Connection,
    root: &Path,
    overlaps_scope: impl Fn(&Path, &RelativePath) -> bool,
) -> CoreResult<Option<OwnerBlocker>> {
    if !installed(db)? {
        return Ok(None);
    }
    let now = now_ms()?;
    let leases = active(db, now)?;
    if leases.is_empty() {
        return Ok(None);
    }
    let checkout = Checkout::identify(root)?;
    for row in leases {
        checkout.comparable(&row.checkout)?;
        let lease_root = if checkout.same_directory(&row.checkout) {
            checkout.path()
        } else {
            row.checkout.path()
        };
        if overlaps_scope(lease_root, &row.path) {
            return Ok(Some(OwnerBlocker {
                remaining_ms: row.expiry.saturating_sub(now) as u64,
                path: row.path,
            }));
        }
    }
    Ok(None)
}

fn installed(db: &Connection) -> CoreResult<bool> {
    let n: u8 = db.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN
         ('code_owner_file_leases','code_editor_epoch','code_writer_reservations')",
        [],
        |r| r.get(0),
    )?;
    match n {
        0 => Ok(false),
        3 => Ok(true),
        _ => Err(UNCONFIRMED),
    }
}

/// Passive status never grants or renews. Does not expose other session IDs.
pub fn status(
    db: &Connection,
    editor: Editor<'_>,
    checkout: &Checkout,
    path: &RelativePath,
) -> CoreResult<Status> {
    editor.check(db)?;
    checkout.check_path(path, true)?;
    let now = now_ms()?;
    match find_owner(
        db,
        checkout,
        Coverage::FileConfined(std::slice::from_ref(path)),
        now,
    )? {
        Some(row) => {
            let remaining_ms = row.expiry.saturating_sub(now) as u64;
            if row.holder == editor.session && row.checkout == *checkout && row.path == *path {
                Ok(Status::Yours { remaining_ms })
            } else {
                Ok(Status::OtherEditor { remaining_ms })
            }
        }
        None => {
            check_writers(db, checkout, path)?;
            Ok(Status::Available)
        }
    }
}

pub fn acquire_tx(
    db: &Transaction<'_>,
    editor: Editor<'_>,
    checkout: &Checkout,
    path: &RelativePath,
) -> CoreResult<LeaseToken> {
    acquire_at(db, editor, checkout, path, now_ms()?)
}
fn acquire_at(
    db: &Transaction<'_>,
    editor: Editor<'_>,
    checkout: &Checkout,
    path: &RelativePath,
    now: i64,
) -> CoreResult<LeaseToken> {
    let epoch = editor.check(db)?;
    checkout.check_path(path, true)?;
    if find_owner(
        db,
        checkout,
        Coverage::FileConfined(std::slice::from_ref(path)),
        now,
    )?
    .is_some()
    {
        return refuse("owner_file_lease_held");
    }
    check_writers(db, checkout, path)?;
    let token = LeaseToken(format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    ));
    db.execute(
        "INSERT INTO code_owner_file_leases VALUES (1,?,?,?,?,?,?,?,1,NULL,?,'active')
         ON CONFLICT(checkout,path) DO UPDATE SET device=excluded.device,inode=excluded.inode,
         holder=excluded.holder,epoch=excluded.epoch,token_hash=excluded.token_hash,
         acquired_generation=coalesce(released_generation,acquired_generation)+1,
         released_generation=NULL,expires_at=excluded.expires_at,state='active'",
        (
            &checkout.canonical,
            checkout.device,
            checkout.inode,
            &path.0,
            editor.session,
            epoch,
            digest(token.0.as_bytes()),
            now.checked_add(TTL_MS).ok_or(UNCONFIRMED)?,
        ),
    )?;
    Ok(token)
}

/// Check the exact live token without extending its deadline.
pub fn status_with_token(
    db: &Connection,
    editor: Editor<'_>,
    checkout: &Checkout,
    path: &RelativePath,
    token: &str,
) -> CoreResult<Status> {
    checkout.check_path(path, true)?;
    let now = now_ms()?;
    let (_, expiry) = live_generation(db, editor, checkout, path, token, now)?;
    Ok(Status::Yours {
        remaining_ms: expiry.saturating_sub(now) as u64,
    })
}

fn live_generation(
    db: &Connection,
    editor: Editor<'_>,
    checkout: &Checkout,
    path: &RelativePath,
    token: &str,
    now: i64,
) -> CoreResult<(i64, i64)> {
    let epoch = editor.check(db)?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(LOST);
    }
    db.query_row(
        "SELECT acquired_generation,expires_at FROM code_owner_file_leases WHERE checkout=? AND device=? AND inode=?
         AND path=? AND holder=? AND epoch=? AND token_hash=? AND state='active' AND expires_at>?",
        (&checkout.canonical, checkout.device, checkout.inode, &path.0, editor.session, epoch,
         digest(token.as_bytes()), now), |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional()?.ok_or(LOST)
}

/// Keep the IMMEDIATE transaction through final path/revision validation and
/// the bounded disk publish. This only validates lease ownership, not contents.
pub fn validate_save_tx(
    db: &Transaction<'_>,
    editor: Editor<'_>,
    checkout: &Checkout,
    path: &RelativePath,
    token: &str,
) -> CoreResult<()> {
    checkout.check_path(path, true)?;
    check_writers(db, checkout, path)?;
    live_generation(db, editor, checkout, path, token, now_ms()?)?;
    Ok(())
}

pub fn renew_tx(
    db: &Transaction<'_>,
    editor: Editor<'_>,
    checkout: &Checkout,
    path: &RelativePath,
    token: &str,
) -> CoreResult<()> {
    let now = now_ms()?;
    checkout.check_path(path, true)?;
    let (generation, _) = live_generation(db, editor, checkout, path, token, now)?;
    db.execute(
        "UPDATE code_owner_file_leases SET expires_at=? WHERE checkout=? AND path=? AND acquired_generation=?",
        (now.checked_add(TTL_MS).ok_or(UNCONFIRMED)?, &checkout.canonical, &path.0, generation),
    )?;
    Ok(())
}

pub fn release_tx(
    db: &Transaction<'_>,
    editor: Editor<'_>,
    checkout: &Checkout,
    path: &RelativePath,
    token: &str,
) -> CoreResult<()> {
    let (generation, _) = live_generation(db, editor, checkout, path, token, now_ms()?)?;
    db.execute(
        "UPDATE code_owner_file_leases SET state='released',released_generation=acquired_generation+1
         WHERE checkout=? AND path=? AND acquired_generation=?",
        (&checkout.canonical, &path.0, generation),
    )?;
    Ok(())
}

/// Trusted local session teardown; release only that session's leases.
pub fn revoke_session_tx(db: &Transaction<'_>, editor: Editor<'_>) -> CoreResult<usize> {
    let epoch = editor.check(db)?;
    Ok(db.execute(
        "UPDATE code_owner_file_leases SET state='released',released_generation=acquired_generation+1
         WHERE holder=? AND epoch=? AND state='active'",
        (editor.session, epoch),
    )?)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
}

fn check_writers(db: &Connection, checkout: &Checkout, path: &RelativePath) -> CoreResult<()> {
    if let Some(task) =
        crate::scheduler::policy::file_write_holder(db, checkout.path(), path.as_str())?
    {
        return Err(CoreError::TaskFileLeaseHeld { task });
    }
    // The retired start: namespace recorded no actual structured write scope.
    let mut query = db.prepare(
        "SELECT checkout,device,inode,paths,state FROM code_writer_reservations
         WHERE state IN ('reserved','ambiguous') AND id NOT GLOB 'start:*' LIMIT ?",
    )?;
    let mut rows = query.query([(MAX_ACTIVE + 1) as i64])?;
    let mut n = 0;
    let mut code = None;
    while let Some(r) = rows.next()? {
        n += 1;
        if n > MAX_ACTIVE {
            return Err(UNCONFIRMED);
        }
        let root = Checkout {
            canonical: r.get(0)?,
            device: r.get(1)?,
            inode: r.get(2)?,
        };
        let paths: Option<String> = r.get(3)?;
        let paths = paths
            .map(|s| {
                serde_json::from_str::<Vec<String>>(&s)
                    .map_err(|_| UNCONFIRMED)?
                    .iter()
                    .map(|path| RelativePath::new(path).map_err(|_| UNCONFIRMED))
                    .collect::<CoreResult<Vec<_>>>()
            })
            .transpose()?;
        if paths.as_ref().is_some_and(|p| p.is_empty() || p.len() > 64) {
            return Err(UNCONFIRMED);
        }
        if covers(
            &root,
            paths
                .as_deref()
                .map_or(Coverage::Broad, Coverage::FileConfined),
            checkout,
            path,
        )? {
            if r.get::<_, String>(4)? == "ambiguous" {
                return Err(UNCONFIRMED);
            }
            if paths.is_none() {
                code = Some("checkout_busy");
            } else if code.is_none() {
                code = Some("task_file_lease_held");
            }
        }
    }
    match code {
        Some(code) => refuse(code),
        None => Ok(()),
    }
}

/// Commit before launching write-capable work. Use one unique ID per authority
/// episode/effect and reserve *all* reachable managed roots (or refuse to launch).
/// Reusing an exact live authority is idempotent; a settled/ambiguous one cannot
/// launch a replacement. This does not loosen existing writer/writer policy.
pub fn reserve_writer_tx(
    db: &Transaction<'_>,
    id: &str,
    checkout: &Checkout,
    coverage: Coverage<'_>,
) -> CoreResult<()> {
    if !valid_id(id) || id.starts_with("start:") {
        return Err(UNCONFIRMED);
    }
    check_worker_effect_tx(db, checkout, coverage)?;
    let paths = match coverage {
        Coverage::Broad => None,
        Coverage::FileConfined(paths) => {
            let sorted: std::collections::BTreeSet<_> = paths.iter().collect();
            Some(serde_json::to_string(&sorted)?)
        }
    };
    let changed = db.execute(
        "INSERT INTO code_writer_reservations VALUES (1,?,?,?,?,?,'reserved') ON CONFLICT(id) DO NOTHING",
        (id, &checkout.canonical, checkout.device, checkout.inode, &paths),
    )?;
    if changed == 0 && !db.query_row(
        "SELECT EXISTS(SELECT 1 FROM code_writer_reservations WHERE id=? AND checkout=? AND device=?
         AND inode=? AND paths IS ? AND state='reserved')",
        (id, &checkout.canonical, checkout.device, checkout.inode, paths), |r| r.get::<_, bool>(0)
    )? {
        return Err(UNCONFIRMED);
    }
    Ok(())
}

/// No TTL. Use ConfirmedExited only after observing that the exact reserved
/// authority/effect is gone (or definitely never launched). Pause, expiry of
/// editor tokens and a terminal task row are not such evidence.
pub fn settle_writer_tx(
    db: &Transaction<'_>,
    id: &str,
    observation: ProcessObservation,
) -> CoreResult<bool> {
    let state = match observation {
        ProcessObservation::ConfirmedExited => "released",
        ProcessObservation::Unknown => "ambiguous",
        ProcessObservation::Alive => return Ok(false),
    };
    Ok(db.execute(
        "UPDATE code_writer_reservations SET state=? WHERE id=? AND state IN ('reserved','ambiguous') AND state!=?",
        (state, id, state),
    )? == 1)
}
