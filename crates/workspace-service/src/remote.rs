//! The embedded remote-access host ([`workspace_remote::Host`]) and the owner's `remote.*`
//! methods on the local socket.
//!
//! The host runs inside this process when the real service starts it
//! ([`crate::Service::start_remote`]). Every tunnelled stream becomes one fresh connection
//! to this service's own socket, and every tunnelled request is first checked by
//! [`crate::remote_policy::allowed`], which refuses every `remote.*` method: only the
//! local owner pairs, decides, revokes and configures remote access.
use crate::local::{connect, failure};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{io, net::SocketAddr, path::Path, sync::Arc};
use workspace_remote::{Host, HostConfig, HostStatus, LocalStream, RelaySource, RemoteError};

/// Bound on an id the owner's window sends back (request and device ids are 64 hex).
const ID_BYTES: usize = 128;
/// Bound on a relay URL the owner types.
const RELAY_BYTES: usize = 2048;
/// The machine name shown to a phone when the host name is unusable.
const FALLBACK_NAME: &str = "This machine";

/// The remote-access host of this service, or why it is not running.
pub(crate) enum Remote {
    Running(Arc<Host>),
    Unavailable(&'static str),
}

/// The owner's `remote.*` requests, parsed strictly.
enum Request {
    Status,
    Enable(bool),
    PairingStart,
    PairingCancel,
    Decide { request_id: String, allow: bool },
    Revoke(String),
    Relay(Option<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Enable {
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Decide {
    request_id: String,
    allow: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Revoke {
    device_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Relay {
    url: Option<String>,
}

fn bounded_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= ID_BYTES && id.bytes().all(|b| b.is_ascii_graphic())
}

fn parse(method: &str, params: Value) -> io::Result<Request> {
    let invalid = || failure("invalid_params");
    fn decode<T: serde::de::DeserializeOwned>(params: Value) -> io::Result<T> {
        serde_json::from_value(params).map_err(|_| failure("invalid_params"))
    }
    let empty = params == json!({});
    Ok(match method {
        "remote.status" if empty => Request::Status,
        "remote.pairing.start" if empty => Request::PairingStart,
        "remote.pairing.cancel" if empty => Request::PairingCancel,
        "remote.status" | "remote.pairing.start" | "remote.pairing.cancel" => return Err(invalid()),
        "remote.enable" => Request::Enable(decode::<Enable>(params)?.enabled),
        "remote.pairing.decide" => {
            let p: Decide = decode(params)?;
            if !bounded_id(&p.request_id) {
                return Err(invalid());
            }
            Request::Decide {
                request_id: p.request_id,
                allow: p.allow,
            }
        }
        "remote.devices.revoke" => {
            let p: Revoke = decode(params)?;
            if !bounded_id(&p.device_id) {
                return Err(invalid());
            }
            Request::Revoke(p.device_id)
        }
        "remote.relay.set" => {
            // `url` is required: `null` clears the override, a missing key is refused.
            if params.get("url").is_none() {
                return Err(invalid());
            }
            let p: Relay = decode(params)?;
            if p.url
                .as_ref()
                .is_some_and(|url| url.is_empty() || url.len() > RELAY_BYTES)
            {
                return Err(invalid());
            }
            Request::Relay(p.url)
        }
        _ => return Err(failure("unsupported_method")),
    })
}

/// The host's error as one of the service's short codes. Never key material or paths.
fn code(error: RemoteError) -> io::Error {
    failure(match error {
        RemoteError::Disabled => "remote_access_disabled",
        RemoteError::UnknownRequest => "unknown_pairing_request",
        RemoteError::UnknownDevice => "unknown_device",
        RemoteError::InvalidConfig(_) => "invalid_remote_relay",
        RemoteError::Bind(_) => "remote_bind_failed",
        RemoteError::Io(_) | RemoteError::InsecureState(_) => "remote_state_unavailable",
        RemoteError::UnknownMachine => "remote_operation_failed",
    })
}

/// The fixed status contract the owner's settings page reads.
pub(crate) fn status_json(status: HostStatus) -> Value {
    json!({
        "available": true,
        "enabled": status.enabled,
        "machineId": status.endpoint_id,
        "relay": {
            "url": status.relay.url,
            "source": match status.relay.source {
                RelaySource::Default => "default",
                RelaySource::Override => "override",
                RelaySource::None => "none",
            },
        },
        "pairing": status.pairing.map(|p| json!({
            "link": p.link,
            "qr": p.qr,
            "expiresAt": p.expires_at_ms,
        })),
        "pending": status.pending.into_iter().map(|p| json!({
            "requestId": p.request_id,
            "deviceName": p.device_name,
            "fingerprint": p.fingerprint,
            "requestedAt": p.requested_at_ms,
        })).collect::<Vec<_>>(),
        "devices": status.devices.into_iter().map(|d| json!({
            "deviceId": d.device_id,
            "name": d.name,
            "pairedAt": d.paired_at_ms,
            "lastSeenAt": d.last_seen_ms,
            "connected": d.connected,
        })).collect::<Vec<_>>(),
    })
}

fn unavailable_json(reason: &str) -> Value {
    json!({
        "available": false,
        "reason": reason,
        "enabled": false,
        "machineId": "",
        "relay": {"url": null, "source": "none"},
        "pairing": null,
        "pending": [],
        "devices": [],
    })
}

/// This machine's host name as a pairing-link machine name, or the fallback.
fn machine_name() -> String {
    let mut buffer = [0u8; 256];
    let rc = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    let name = (rc == 0)
        .then(|| {
            let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
            String::from_utf8(buffer[..end].to_vec()).ok()
        })
        .flatten();
    name.and_then(|n| workspace_remote::link::validate_name(&n))
        .unwrap_or_else(|| FALLBACK_NAME.to_owned())
}

/// A plain reason for the owner when the host could not start.
fn start_reason(error: &RemoteError) -> &'static str {
    match error {
        RemoteError::InsecureState(_) => {
            "Remote access could not start: its private state folder is not private to your account."
        }
        RemoteError::Io(_) => "Remote access could not start: its private state could not be read.",
        RemoteError::Bind(_) => "Remote access could not start: no network port could be opened.",
        RemoteError::InvalidConfig(_) => "Remote access could not start: its saved relay is invalid.",
        _ => "Remote access could not start.",
    }
}

/// Starts the host: state under `<state>/remote`, tunnel streams to `socket`, the remote
/// allowlist as its policy. `bind_addrs` empty binds the default addresses.
pub(crate) async fn start(
    state: &Path,
    socket: &Path,
    bind_addrs: Vec<SocketAddr>,
    default_relay: Option<String>,
) -> Remote {
    let socket = socket.to_path_buf();
    let mut config = HostConfig::new(
        state.join("remote"),
        machine_name(),
        move || {
            let socket = socket.clone();
            async move { Ok(Box::new(connect(&socket).await?) as LocalStream) }
        },
        crate::remote_policy::allowed,
    );
    config.default_relay = default_relay;
    config.bind_addrs = bind_addrs;
    match Host::start(config).await {
        Ok(host) => Remote::Running(Arc::new(host)),
        Err(error) => Remote::Unavailable(start_reason(&error)),
    }
}

/// Serves one owner `remote.*` request and answers the full status.
pub(crate) async fn call(
    remote: Option<&Remote>,
    method: &str,
    params: Value,
) -> io::Result<Value> {
    let request = parse(method, params)?;
    let host = match remote {
        Some(Remote::Running(host)) => host,
        Some(Remote::Unavailable(reason)) => {
            return match request {
                Request::Status => Ok(unavailable_json(reason)),
                _ => Err(failure("remote_unavailable")),
            }
        }
        None => {
            return match request {
                Request::Status => Ok(unavailable_json(
                    "Remote access is not running in this machine service.",
                )),
                _ => Err(failure("remote_unavailable")),
            }
        }
    };
    match request {
        Request::Status => (),
        Request::Enable(enabled) => host.set_enabled(enabled).await.map_err(code)?,
        Request::PairingStart => {
            host.pairing_start().await.map_err(code)?;
        }
        Request::PairingCancel => host.pairing_cancel(),
        Request::Decide { request_id, allow } => host.decide(&request_id, allow).map_err(code)?,
        Request::Revoke(device) => host.revoke(&device).map_err(code)?,
        Request::Relay(url) => host.set_relay(url.as_deref()).await.map_err(code)?,
    }
    Ok(status_json(host.status()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_parsed_strictly() {
        assert!(matches!(
            parse("remote.status", json!({})),
            Ok(Request::Status)
        ));
        assert!(parse("remote.status", json!({"x":1})).is_err());
        assert!(parse("remote.status", Value::Null).is_err());
        assert!(matches!(
            parse("remote.enable", json!({"enabled":true})),
            Ok(Request::Enable(true))
        ));
        assert!(parse("remote.enable", json!({"enabled":"yes"})).is_err());
        assert!(parse("remote.enable", json!({"enabled":true,"x":1})).is_err());
        assert!(parse(
            "remote.pairing.decide",
            json!({"requestId":"r","allow":true})
        )
        .is_ok());
        assert!(parse(
            "remote.pairing.decide",
            json!({"requestId":"","allow":true})
        )
        .is_err());
        assert!(parse(
            "remote.pairing.decide",
            json!({"requestId":"r".repeat(129),"allow":true})
        )
        .is_err());
        assert!(parse("remote.pairing.decide", json!({"requestId":"r"})).is_err());
        assert!(parse("remote.devices.revoke", json!({"deviceId":"d"})).is_ok());
        assert!(parse("remote.devices.revoke", json!({"deviceId":"a b"})).is_err());
        assert!(matches!(
            parse("remote.relay.set", json!({"url":null})),
            Ok(Request::Relay(None))
        ));
        assert!(parse("remote.relay.set", json!({})).is_err());
        assert!(parse("remote.relay.set", json!({"url":""})).is_err());
        assert!(parse("remote.relay.set", json!({"url":"x".repeat(2049)})).is_err());
        assert!(parse("remote.unknown", json!({})).is_err());
    }

    #[tokio::test]
    async fn a_service_without_a_host_reports_unavailable_and_refuses_changes() {
        let status = call(None, "remote.status", json!({})).await.unwrap();
        assert_eq!(status["available"], false);
        assert!(status["reason"].as_str().is_some_and(|r| !r.is_empty()));
        assert_eq!(status["relay"], json!({"url":null,"source":"none"}));
        assert_eq!(
            call(None, "remote.enable", json!({"enabled":true}))
                .await
                .unwrap_err()
                .to_string(),
            "remote_unavailable"
        );
        let refused = Remote::Unavailable("why");
        assert_eq!(
            call(Some(&refused), "remote.status", json!({}))
                .await
                .unwrap()["reason"],
            "why"
        );
    }

    #[test]
    fn the_machine_name_is_always_a_valid_link_name() {
        assert!(workspace_remote::link::validate_name(&machine_name()).is_some());
    }
}
