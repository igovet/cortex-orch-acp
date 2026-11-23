//! Shared wire format for desktop release signing. Only the public key ships.
use ed25519_dalek::{Signature, VerifyingKey};

pub const TRUSTED_KEY: &str = include_str!("release-ed25519.pub");
pub const CHECKSUM_LIMIT: usize = 1024;
pub const SIGNATURE_LEN: usize = 64;

pub fn public_key(hex: &str) -> Option<VerifyingKey> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.bytes().all(is_hex) {
        return None;
    }
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    VerifyingKey::from_bytes(&bytes)
        .ok()
        .filter(|key| !key.is_weak())
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

pub fn checksum_digest<'a>(checksum: &'a [u8], archive_name: &str) -> Option<&'a str> {
    let text = std::str::from_utf8(checksum).ok()?;
    let line = text.strip_suffix('\n').unwrap_or(text);
    let (digest, name) = line.split_once("  ")?;
    (checksum.len() <= CHECKSUM_LIMIT
        && name == archive_name
        && digest.len() == 64
        && digest.bytes().all(is_hex))
    .then_some(digest)
}

pub fn authenticate<'a>(
    key: &VerifyingKey,
    checksum: &'a [u8],
    signature: &[u8],
    archive_name: &str,
) -> Option<&'a str> {
    if checksum.len() > CHECKSUM_LIMIT {
        return None;
    }
    let signature = Signature::from_slice(signature).ok()?;
    key.verify_strict(checksum, &signature).ok()?;
    checksum_digest(checksum, archive_name)
}
