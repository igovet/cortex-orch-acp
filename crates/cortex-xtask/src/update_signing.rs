//! Offline desktop update-signing commands. Private seed bytes are never formatted.
use crate::{json::Res, Args};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};
use zeroize::Zeroizing;

#[path = "../../../packaging/update-signing/wire.rs"]
mod wire;

const KEY_PATH: &str = "CORTEX_UPDATE_SIGNING_KEY";
const PRIVATE_FILE: &str =
    "Signing key must be a regular, owner-only 0600 file in a 0700 directory outside the checkout.";
const USAGE: &str = "Expected update-key init, update-sign <deb> [--public-key F], or update-verify <deb> [--public-key F].";

fn private_path(root: &Path, create: bool) -> Res<PathBuf> {
    let path = match env::var_os(KEY_PATH) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?)
            .join(".config/cortex-workspace/update-signing/release-ed25519.key"),
    };
    let file = path.file_name().ok_or(PRIVATE_FILE)?;
    let parent = path.parent().ok_or(PRIVATE_FILE)?;
    let root = root.canonicalize().map_err(|_| PRIVATE_FILE)?;
    let built_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let built_root = built_root.canonicalize().ok();
    let in_checkout = |p: &Path| {
        p.starts_with(&root)
            || built_root.as_ref().is_some_and(|root| p.starts_with(root))
            || p.ancestors().any(|dir| {
                let git = dir.join(".git");
                git.is_file() || git.join("HEAD").symlink_metadata().is_ok()
            })
    };
    if !path.is_absolute()
        || path.components().any(|c| c == Component::ParentDir)
        || in_checkout(parent)
    {
        return Err(PRIVATE_FILE.into());
    }
    if create {
        let mut ancestor = parent;
        loop {
            match ancestor.symlink_metadata() {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    ancestor = ancestor.parent().ok_or(PRIVATE_FILE)?;
                }
                Err(_) => return Err(PRIVATE_FILE.into()),
            }
        }
        if in_checkout(&ancestor.canonicalize().map_err(|_| PRIVATE_FILE)?) {
            return Err(PRIVATE_FILE.into());
        }
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|_| PRIVATE_FILE)?;
    }
    let dir = parent.canonicalize().map_err(|_| PRIVATE_FILE)?;
    let metadata = dir.metadata().map_err(|_| PRIVATE_FILE)?;
    if in_checkout(&dir)
        || !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(PRIVATE_FILE.into());
    }
    Ok(dir.join(file))
}

fn read_key(path: &Path) -> Res<SigningKey> {
    let mut f = open_regular(path).map_err(|_| PRIVATE_FILE)?;
    let m = f.metadata().map_err(|_| PRIVATE_FILE)?;
    if m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o7777 != 0o600
        || m.nlink() != 1
        || m.len() != 32
    {
        return Err(PRIVATE_FILE.into());
    }
    let mut seed = Zeroizing::new([0u8; 32]);
    f.read_exact(&mut *seed).map_err(|_| PRIVATE_FILE)?;
    Ok(SigningKey::from_bytes(&seed))
}

fn init(root: &Path) -> Res<()> {
    let key_path = private_path(root, true)?;
    let mut seed = Zeroizing::new([0u8; 32]);
    File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut *seed))
        .map_err(|_| "Could not get randomness for an Ed25519 signing key.")?;
    let signing_key = SigningKey::from_bytes(&seed);
    let mut f = OpenOptions::new()
        .write(true)
        .mode(0o600)
        .create_new(true)
        .open(key_path)
        .map_err(|_| "Could not create the private key; an existing key is never overwritten.")?;
    f.write_all(&*seed)
        .and_then(|()| f.sync_all())
        .map_err(|_| "Could not save the private key; inspect the private file before retrying.")?;
    let hex: String = signing_key
        .verifying_key()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    println!("{hex}");
    Ok(())
}

fn open_regular(path: &Path) -> std::io::Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !f.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(f)
}

fn read_bounded(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_regular(path)?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::other("sidecar size limit exceeded"));
    }
    Ok(bytes)
}

fn trust(args: &Args) -> Res<VerifyingKey> {
    if let Some(path) = args.path("public-key") {
        let bytes = read_bounded(&path, 128).map_err(|_| "Could not read the public key.")?;
        return std::str::from_utf8(&bytes)
            .ok()
            .and_then(wire::public_key)
            .ok_or_else(|| "Invalid Ed25519 public key.".into());
    }
    wire::public_key(wire::TRUSTED_KEY)
        .ok_or_else(|| "No trusted desktop update key is embedded.".into())
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

fn archive_digest(archive: &Path) -> Res<String> {
    let mut file = open_regular(archive).map_err(|_| "Could not open the Debian archive.")?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).map_err(|_| "Could not read the Debian archive.")?;
    Ok(format!("{:x}", hash.finalize()))
}

pub fn run(args: &Args) -> Res<()> {
    match args.positional.as_slice() {
        [command, action]
            if command == "update-key" && action == "init" && args.flags.is_empty() =>
        {
            return init(&args.root()?);
        }
        [command, _]
            if matches!(command.as_str(), "update-sign" | "update-verify")
                && (args.flags.is_empty()
                    || args.flags.len() == 1 && args.flag("public-key").is_some()) => {}
        _ => return Err(USAGE.into()),
    }
    let archive = Path::new(&args.positional[1]);
    let archive_name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            name.ends_with(".deb")
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._+~-".contains(&b))
        })
        .ok_or("Expected a Debian archive filename.")?;
    let checksum_path = archive.with_file_name(format!("{archive_name}.sha256"));
    let sig_path = archive.with_file_name(format!("{archive_name}.sha256.sig"));
    let key = trust(args)?;

    if args.positional[0] == "update-sign" {
        let signing_key = read_key(&private_path(&args.root()?, false)?)?;
        if signing_key.verifying_key() != key {
            return Err("The signing key does not match the trusted public key.".into());
        }
        let digest = archive_digest(archive)?;
        let checksum = match read_bounded(&checksum_path, wire::CHECKSUM_LIMIT) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let bytes = format!("{digest}  {archive_name}\n").into_bytes();
                write_new(&checksum_path, &bytes)
                    .map_err(|_| "Could not create checksum sidecar.")?;
                bytes
            }
            Err(_) => return Err("Could not read checksum sidecar.".into()),
        };
        if wire::checksum_digest(&checksum, archive_name) != Some(digest.as_str()) {
            return Err("Checksum sidecar does not match the archive.".into());
        }
        let signature = signing_key.sign(&checksum).to_bytes();
        match write_new(&sig_path, &signature) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists
                && read_bounded(&sig_path, wire::SIGNATURE_LEN).ok().as_deref() == Some(&signature) => {}
            Err(_) => return Err("Could not write signature sidecar; an existing different signature is never overwritten.".into()),
        }
        println!("Desktop update checksum signed.");
    } else {
        let failed = "Desktop update verification failed.";
        let checksum = read_bounded(&checksum_path, wire::CHECKSUM_LIMIT).map_err(|_| failed)?;
        let signature = read_bounded(&sig_path, wire::SIGNATURE_LEN).map_err(|_| failed)?;
        let expected =
            wire::authenticate(&key, &checksum, &signature, archive_name).ok_or(failed)?;
        if expected != archive_digest(archive)? {
            return Err(failed.into());
        }
        println!("Desktop update signature and archive digest verified.");
    }
    Ok(())
}
