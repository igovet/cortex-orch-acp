//! Local Code leases. The Unix socket authenticates the owner UID; the
//! opaque, per-tab bearer token distinguishes editors using that account.
use super::{context, files, git::Git, native::*, settings::Lock};
use cortex_core::{
    store::owner_file_leases::{self as leases, Checkout, Editor, EditorEpoch, RelativePath},
    CoreError, CoreResult, OpenMode, Store,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
    sync::Mutex,
};

#[derive(Default)]
pub(crate) struct Authority {
    epoch: EditorEpoch,
    // Elect one service for each existing logical store directory. Hold the
    // directory descriptor, never a second descriptor for the SQLite file.
    // In particular, a second service may not activate over an active epoch.
    stores: Mutex<BTreeMap<(u64, u64), File>>,
}

impl Authority {
    fn prepare(&self, store: &Store) -> CoreResult<()> {
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(store.key().directory())?;
        let m = dir.metadata()?;
        let key = (m.dev(), m.ino());
        let mut stores = self
            .stores
            .lock()
            .map_err(|_| CoreError::Refused("writer_unconfirmed"))?;
        if stores.contains_key(&key) {
            return Ok(());
        }
        if unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(CoreError::Refused("writer_unconfirmed"));
        }
        store.write(|db| leases::activate_editor_tx(db, &self.epoch))?;
        stores.insert(key, dir);
        Ok(())
    }
}

pub(super) fn handles(op: &str) -> bool {
    matches!(
        op,
        "repo.file"
            | "repo.file.save"
            | "repo.file.lease.acquire"
            | "repo.file.lease.renew"
            | "repo.file.lease.release"
            | "repo.file.lease.status"
    )
}

pub(super) fn lease_error(error: CoreError) -> Refused {
    match error {
        CoreError::TaskFileLeaseHeld { task } => {
            refuse("task_file_lease_held", "A task is writing this file.")
                .with(json!({"holderTaskId":task}))
        }
        e if e.code() == "edit_lease_lost" => refuse(
            "edit_lease_lost",
            "The edit lease is no longer valid. Choose Edit again.",
        ),
        e if e.code() == "owner_file_lease_held" => refuse(
            "owner_file_lease_held",
            "This file is already open for editing.",
        ),
        e => context::core(e),
    }
}

fn token(params: &Value) -> Result<&str> {
    params["leaseToken"]
        .as_str()
        .ok_or_else(|| lease_error(CoreError::Refused("edit_lease_lost")))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    authority: &Authority,
    op: &str,
    root: &Path,
    state: &Path,
    project: &str,
    params: &Value,
    git: &mut Git,
) -> Result<Value> {
    let (wanted, checkout) = context::file_checkout(root, project, params, git).await?;
    let name = string(params, "file")?;
    relative(name)?;
    // Renew and release must work even if a Git attribute has changed since
    // acquisition. Save still checks live filter policy before writing.
    if matches!(
        op,
        "repo.file" | "repo.file.save" | "repo.file.lease.acquire"
    ) && git.is_repo(&checkout).await?
    {
        if let Some(driver) = git.filters(&checkout, &[name.to_owned()]).await?.get(name) {
            if op != "repo.file" {
                return Err(refuse(
                    "filtered_file",
                    "Filtered files cannot be edited here.",
                ));
            }
            return Ok(json!({"file":name,"text":null,"revision":null,
                "reason":format!("Git filter '{driver}'-managed file. Editing is unavailable; no filter is run.")}));
        }
    }
    if op == "repo.file" {
        // Preserve the refusal for an invalid existing store; a passive read
        // never initializes or activates one.
        context::open(root, OpenMode::LiveReadOnly)?;
        return files::read(&checkout, params);
    }
    let path = RelativePath::new(name).map_err(|_| refuse("invalid_path", "Invalid path."))?;
    let checkout = Checkout::identify(&checkout).map_err(lease_error)?;
    let actor = crate::actor();
    let editor = Editor {
        epoch: &authority.epoch,
        session: &actor,
    };
    let store = if op == "repo.file.lease.acquire" {
        // Explicit Edit, not a passive Code read, can establish the channel's
        // store for a registered folder that has never run a task.
        let key = context::key(root)?;
        crate::project_guard::activate(key.root())?;
        let store = Store::open(key, OpenMode::Initialize).map_err(lease_error)?;
        authority.prepare(&store).map_err(lease_error)?;
        store
    } else {
        let readonly = op == "repo.file.lease.status";
        match context::open(
            root,
            if readonly {
                OpenMode::LiveReadOnly
            } else {
                OpenMode::RuntimeValidate
            },
        )? {
            Some(store) => store,
            None if readonly && params.get("leaseToken").is_none() => {
                return Ok(json!({"status":"available"}))
            }
            None => return Err(lease_error(CoreError::Refused("edit_lease_lost"))),
        }
    };
    match op {
        "repo.file.lease.acquire" => store.write(|db| {
            check_binding(db, &store, project, &wanted, &checkout)?;
            let token = leases::acquire_tx(db, editor, &checkout, &path)?;
            Ok(json!({"leaseToken":token.expose(),"expiresInMs":leases::TTL_SECONDS * 1000,
                "renewAfterMs":leases::RENEW_INTERVAL_SECONDS * 1000}))
        }).map_err(lease_error),
        "repo.file.lease.renew" | "repo.file.lease.release" => {
            let token = token(params)?;
            store.write(|db| {
                if op == "repo.file.lease.renew" {
                    check_binding(db, &store, project, &wanted, &checkout)?;
                    leases::renew_tx(db, editor, &checkout, &path, token)?;
                    Ok(json!({"expiresInMs":leases::TTL_SECONDS * 1000}))
                } else {
                    leases::release_tx(db, editor, &checkout, &path, token)?;
                    Ok(json!({"released":true}))
                }
            }).map_err(lease_error)
        }
        "repo.file.lease.status" => store.read(|db| {
            if let Some(token) = params.get("leaseToken") {
                let token = token.as_str().ok_or(CoreError::Refused("edit_lease_lost"))?;
                let leases::Status::Yours { remaining_ms } = leases::status_with_token(db, editor, &checkout, &path, token)? else {
                    return Err(CoreError::Refused("edit_lease_lost"));
                };
                return Ok(json!({"status":"yours","expiresInMs":remaining_ms}));
            }
            // An existing format-12 store may predate the optional lease
            // schema. Writable Initialize installs it; status never does.
            let installed: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='code_owner_file_leases')",
                [], |r| r.get(0),
            )?;
            if !installed {
                return Ok(json!({"status":"available"}));
            }
            let coverage = leases::Coverage::FileConfined(std::slice::from_ref(&path));
            Ok(match leases::writer_blocker(db, &checkout, coverage)? {
                Some(lease) => json!({"status":"held","expiresInMs":lease.remaining_ms}),
                None => json!({"status":"available"}),
            })
        }).map_err(lease_error),
        "repo.file.save" => {
            let token = token(params)?;
            // Start takes machine.lock before BEGIN IMMEDIATE. Match that
            // order, then keep both guards through the revision check and
            // publish. Recheck the lease after fsync because its TTL can
            // elapse even while another writer cannot acquire it.
            let lock = Lock::new(state, "machine.lock")?;
            store
                .write(|db| {
                    check_binding(db, &store, project, &wanted, &checkout)?;
                    let validate = || leases::validate_save_tx(db, editor, &checkout, &path, token);
                    validate()?;
                    Ok(files::save_locked_with(checkout.path(), params, &lock, || {
                        validate().map_err(lease_error)
                    }))
                })
                .map_err(lease_error)?
        }
        _ => unreachable!("only Code routes call this handler"),
    }
}

fn check_binding(
    db: &rusqlite::Connection,
    store: &Store,
    project: &str,
    wanted: &str,
    checkout: &Checkout,
) -> CoreResult<()> {
    use cortex_core::workspace::worktrees;
    if let Some(task) = worktrees::desktop_draft_for_checkout(db, project, wanted)? {
        let bound = worktrees::desktop_checkout_on(db, store.key(), project, Some(&task), None)?;
        if bound.as_ref().map(|c| c.path()) != Some(checkout.path()) {
            return Err(CoreError::Refused("worktree_binding_invalid"));
        }
    }
    Ok(())
}
