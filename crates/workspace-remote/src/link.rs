//! Pairing links, fingerprints and QR matrices.
//!
//! # Link format (version 1)
//!
//! `cortex-pair:` followed by unpadded base64url (RFC 4648 §5) of this binary payload:
//!
//! | bytes | field |
//! |---|---|
//! | 1 | format version, `1` |
//! | 32 | host endpoint id (Ed25519 public key) |
//! | 32 | single-use pairing secret |
//! | 1 + n | machine name: length `n` (1..=64), UTF-8 |
//! | 1 + n | relay URL: length `n` (0 = no relay, ..=255), UTF-8 |
//! | 1 | direct address count (0..=8) |
//! | 7 or 19 each | `4` + IPv4 (4) + port (2, BE), or `6` + IPv6 (16) + port (2, BE) |
//! | 4 | first 4 bytes of SHA-256 over all preceding bytes (typo check) |
//!
//! Anything after the checksum, a bad checksum, or an out-of-range field makes the
//! link malformed. A different first byte is reported as an unsupported version.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use data_encoding::BASE64URL_NOPAD;
use iroh::{EndpointId, RelayUrl};
use sha2::{Digest, Sha256};

/// Fixed prefix of every pairing link.
pub const LINK_PREFIX: &str = "cortex-pair:";
/// The only link format version this crate writes and reads.
pub const LINK_VERSION: u8 = 1;
/// Longest machine or device name in bytes (UTF-8).
pub const NAME_BYTES: usize = 64;
/// Longest relay URL in bytes.
pub const RELAY_URL_BYTES: usize = 255;
/// Most direct addresses carried in a link or hello.
pub const LINK_ADDRS: usize = 8;
/// Longest accepted link text (prefix included).
pub const LINK_TEXT_BYTES: usize = 1024;

/// Why a pairing link was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// Not a `cortex-pair:` link, bad encoding, bad checksum or an invalid field.
    Malformed(&'static str),
    /// A well-formed prefix with a format version this build does not know.
    UnsupportedVersion(u8),
}

impl LinkError {
    /// Stable refusal code, without diagnostic detail.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "invalid_pairing_link",
            Self::UnsupportedVersion(_) => "unsupported_pairing_link_version",
        }
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Malformed(why) => write!(f, "{}: {why}", self.code()),
            LinkError::UnsupportedVersion(v) => write!(f, "{}: {v}", self.code()),
        }
    }
}

impl std::error::Error for LinkError {}

/// The decoded content of a pairing link.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingLink {
    /// The host's endpoint id.
    pub host_id: EndpointId,
    /// The relay the host uses, if any.
    pub relay: Option<RelayUrl>,
    /// The host's direct socket addresses at the time the link was made.
    pub addrs: Vec<SocketAddr>,
    /// The single-use pairing secret.
    pub secret: [u8; 32],
    /// The machine's display name.
    pub machine_name: String,
}

impl std::fmt::Debug for PairingLink {
    // The secret is deliberately omitted.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingLink")
            .field("host_id", &self.host_id)
            .field("relay", &self.relay)
            .field("addrs", &self.addrs)
            .field("machine_name", &self.machine_name)
            .finish_non_exhaustive()
    }
}

/// Validates a machine or device name: 1..=64 bytes of UTF-8 after trimming, no
/// control characters. Returns the trimmed name.
pub fn validate_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > NAME_BYTES || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.to_string())
}

fn checksum(bytes: &[u8]) -> [u8; 4] {
    let digest = Sha256::digest(bytes);
    [digest[0], digest[1], digest[2], digest[3]]
}

impl PairingLink {
    /// Encodes the link as `cortex-pair:<base64url>`. At most [`LINK_ADDRS`] addresses
    /// are written. Fails when the name or relay URL is out of bounds.
    pub fn encode(&self) -> Result<String, LinkError> {
        let name = validate_name(&self.machine_name).ok_or(LinkError::Malformed("machine name"))?;
        let relay = self
            .relay
            .as_ref()
            .map(|r| r.to_string())
            .unwrap_or_default();
        if relay.len() > RELAY_URL_BYTES {
            return Err(LinkError::Malformed("relay url"));
        }
        let mut out = vec![LINK_VERSION];
        out.extend_from_slice(self.host_id.as_bytes());
        out.extend_from_slice(&self.secret);
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.push(relay.len() as u8);
        out.extend_from_slice(relay.as_bytes());
        let addrs: Vec<_> = self.addrs.iter().take(LINK_ADDRS).collect();
        out.push(addrs.len() as u8);
        for addr in addrs {
            match addr.ip() {
                IpAddr::V4(ip) => {
                    out.push(4);
                    out.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    out.push(6);
                    out.extend_from_slice(&ip.octets());
                }
            }
            out.extend_from_slice(&addr.port().to_be_bytes());
        }
        let sum = checksum(&out);
        out.extend_from_slice(&sum);
        Ok(format!("{LINK_PREFIX}{}", BASE64URL_NOPAD.encode(&out)))
    }

    /// Parses and validates a link. Surrounding whitespace is ignored.
    pub fn decode(text: &str) -> Result<Self, LinkError> {
        let text = text.trim();
        if text.len() > LINK_TEXT_BYTES {
            return Err(LinkError::Malformed("too long"));
        }
        let body = text
            .strip_prefix(LINK_PREFIX)
            .ok_or(LinkError::Malformed("missing cortex-pair: prefix"))?;
        let bytes = BASE64URL_NOPAD
            .decode(body.as_bytes())
            .map_err(|_| LinkError::Malformed("not base64url"))?;
        let version = *bytes.first().ok_or(LinkError::Malformed("empty"))?;
        if version != LINK_VERSION {
            return Err(LinkError::UnsupportedVersion(version));
        }
        if bytes.len() < 5 {
            return Err(LinkError::Malformed("truncated"));
        }
        let (content, sum) = bytes.split_at(bytes.len() - 4);
        if checksum(content) != sum {
            return Err(LinkError::Malformed("checksum mismatch"));
        }
        let mut r = Reader {
            bytes: &content[1..],
        };
        let host_id = EndpointId::from_bytes(&r.array::<32>()?)
            .map_err(|_| LinkError::Malformed("host id"))?;
        let secret = r.array::<32>()?;
        let name_len = r.byte()? as usize;
        let name = std::str::from_utf8(r.take(name_len)?)
            .map_err(|_| LinkError::Malformed("machine name"))?;
        let machine_name = validate_name(name)
            .filter(|n| n == name)
            .ok_or(LinkError::Malformed("machine name"))?;
        let relay_len = r.byte()? as usize;
        let relay = std::str::from_utf8(r.take(relay_len)?)
            .map_err(|_| LinkError::Malformed("relay url"))?;
        let relay = if relay.is_empty() {
            None
        } else {
            Some(parse_relay(relay).ok_or(LinkError::Malformed("relay url"))?)
        };
        let count = r.byte()? as usize;
        if count > LINK_ADDRS {
            return Err(LinkError::Malformed("too many addresses"));
        }
        let mut addrs = Vec::with_capacity(count);
        for _ in 0..count {
            let ip = match r.byte()? {
                4 => IpAddr::V4(Ipv4Addr::from(r.array::<4>()?)),
                6 => IpAddr::V6(Ipv6Addr::from(r.array::<16>()?)),
                _ => return Err(LinkError::Malformed("address family")),
            };
            let port = u16::from_be_bytes(r.array::<2>()?);
            if port == 0 {
                return Err(LinkError::Malformed("address port"));
            }
            addrs.push(SocketAddr::new(ip, port));
        }
        if !r.bytes.is_empty() {
            return Err(LinkError::Malformed("trailing bytes"));
        }
        Ok(PairingLink {
            host_id,
            relay,
            addrs,
            secret,
            machine_name,
        })
    }
}

/// Parses an `http`/`https` relay URL.
pub fn parse_relay(text: &str) -> Option<RelayUrl> {
    if text.len() > RELAY_URL_BYTES
        || !(text.starts_with("https://") || text.starts_with("http://"))
    {
        return None;
    }
    text.parse().ok()
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], LinkError> {
        if self.bytes.len() < n {
            return Err(LinkError::Malformed("truncated"));
        }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Ok(head)
    }
    fn byte(&mut self) -> Result<u8, LinkError> {
        Ok(self.take(1)?[0])
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], LinkError> {
        Ok(self.take(N)?.try_into().expect("length checked"))
    }
}

/// The pairing fingerprint both sides display: SHA-256 over a domain tag, the host
/// id, the device id and the secret; the first 6 bytes as 12 uppercase hex
/// characters in three groups of four, e.g. `4F2A 91C7 D03E`.
pub fn fingerprint(host: &EndpointId, device: &EndpointId, secret: &[u8; 32]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"cortex-remote-fingerprint-v1");
    hash.update(host.as_bytes());
    hash.update(device.as_bytes());
    hash.update(secret);
    let hex = data_encoding::HEXUPPER.encode(&hash.finalize()[..6]);
    format!("{} {} {}", &hex[0..4], &hex[4..8], &hex[8..12])
}

/// The QR matrix of `text` as rows of `'1'` (dark) and `'0'` (light) modules, square,
/// without the quiet zone (draw at least four light modules around it).
pub fn qr_rows(text: &str) -> Vec<String> {
    let Ok(code) = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
    else {
        return Vec::new();
    };
    let width = code.width();
    code.to_colors()
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' })
                .collect()
        })
        .collect()
}
