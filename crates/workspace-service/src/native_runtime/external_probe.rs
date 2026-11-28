use serde_json::Value;
use std::io;

/// One public address for this attempt, or refuse the entire DNS answer if
/// it contains a non-public destination. Do not reinterpret mapped/tunnel
/// addresses or scope identifiers as public IPv6 addresses.
fn pinned_external_address(
    addresses: impl IntoIterator<Item = std::net::SocketAddr>,
) -> Result<std::net::SocketAddr, &'static str> {
    use std::net::SocketAddr;
    let mut first = None;
    for address in addresses {
        let public = match address {
            SocketAddr::V4(addr) => {
                let ip = addr.ip();
                let [a, b, c, _] = ip.octets();
                !ip.is_private()
                    && !ip.is_loopback()
                    && !ip.is_link_local()
                    && !ip.is_documentation()
                    && !matches!(
                        (a, b, c),
                        (0 | 224..=255, _, _)
                            | (100, 64..=127, _)
                            | (192, 0, 0)
                            | (192, 88, 99)
                            | (198, 18..=19, _)
                    )
            }
            SocketAddr::V6(addr) => {
                let [a, b, ..] = addr.ip().segments();
                addr.scope_id() == 0
                    && (0x2000..=0x3fff).contains(&a)
                    // Protocol assignments (incl. Teredo), documentation, and 6to4.
                    && !matches!((a, b), (0x2001, 0..=0x01ff | 0x0db8) | (0x2002, _) | (0x3fff, _))
            }
        };
        if !public || address.port() != 443 {
            return Err("outage probe refused a non-public or mismatched DNS address");
        }
        first.get_or_insert(address);
    }
    first.ok_or("outage probe DNS returned no addresses")
}

fn configure_external_head(
    command: &mut tokio::process::Command,
    host: &str,
    address: std::net::SocketAddr,
) {
    let port = address.port();
    let ip = match address.ip() {
        std::net::IpAddr::V4(ip) => ip.to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
    };
    command
        .env_clear()
        .args([
            "-q", // Must be first: even with no HOME, curl may otherwise read curlrc.
            "--silent",
            "--head",
            "--output",
            "/dev/null",
            "--max-time",
            "10",
            "--proto",
            "=https",
            "--noproxy",
            "*",
            "--no-location",
            "--resolve",
            &format!("{host}:{port}:{ip}"),
            "--write-out",
            "%{http_code}",
            "--",
            &format!("https://{host}:{port}/"),
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
}

/// A wait (including a retained wait from a previous version) is not proof of
/// destination authority. Validate before any DNS lookup and bound the whole
/// probe, including DNS, to 10 seconds.
pub(crate) async fn probe_external(wait: &Value) -> (bool, String) {
    probe_external_with_lookup(wait, |host| tokio::net::lookup_host((host, 443))).await
}

async fn probe_external_with_lookup<A, L, F>(wait: &Value, lookup: L) -> (bool, String)
where
    A: IntoIterator<Item = std::net::SocketAddr>,
    L: FnOnce(&'static str) -> F,
    F: std::future::Future<Output = io::Result<A>>,
{
    probe_external_with_tcp(wait, lookup, |address| async move {
        tokio::net::TcpStream::connect(address).await.map(drop)
    }).await
}

pub(super) async fn probe_external_with_tcp<A, L, F, D, C>(wait: &Value, lookup: L, dial: D) -> (bool, String)
where
    A: IntoIterator<Item = std::net::SocketAddr>,
    L: FnOnce(&'static str) -> F,
    F: std::future::Future<Output = io::Result<A>>,
    D: FnOnce(std::net::SocketAddr) -> C,
    C: std::future::Future<Output = io::Result<()>>,
{
    use cortex_core::lifecycle::obstacles::external_probe_host;
    let host = wait["host"].as_str().unwrap_or("");
    let scheme = wait["scheme"].as_str().unwrap_or("");
    let port = wait["port"].as_u64().and_then(|p| u16::try_from(p).ok());
    let Some(host) = port.and_then(|p| external_probe_host(host, scheme, p)) else {
        return (false, "outage probe destination unsupported".into());
    };
    let Some(probe @ ("tcp" | "http")) = wait["probe"].as_str() else {
        return (false, "outage probe kind unsupported".into());
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let addresses = lookup(host).await.map_err(|_| "outage probe DNS failed")?;
        let address = pinned_external_address(addresses)?;
        if probe == "tcp" {
            dial(address).await.map_err(|_| "outage probe connect failed")?;
            return Ok((true, format!("{host}:443 accepts connections")));
        }
        // The URL retains the fixed TLS server name; --resolve supplies the
        // exact checked address, so curl neither looks it up again nor uses a proxy.
        let mut command = tokio::process::Command::new("/usr/bin/curl");
        configure_external_head(&mut command, host, address);
        let out = command
            .output()
            .await
            .map_err(|_| "outage HEAD unavailable")?;
        if !out.status.success() {
            return Err("outage HEAD failed");
        }
        let code = std::str::from_utf8(&out.stdout)
            .ok()
            .and_then(|status| status.trim().parse::<u16>().ok())
            .filter(|code| (200..600).contains(code))
            .ok_or("outage HEAD returned no final HTTP status")?;
        Ok((code < 500 && code != 429, format!("HTTP {code}")))
    })
    .await;
    match result {
        Ok(Ok(result)) => result,
        Ok(Err(detail)) => (false, detail.into()),
        Err(_) => (false, "outage probe timed out".into()),
    }
}

#[cfg(test)]
mod external_probe_tests {
    use serde_json::json;
    use super::*;
    use std::{future::ready, net::SocketAddr};

    #[tokio::test]
    async fn unsupported_stored_targets_do_not_reach_dns() {
        for (host, scheme, port, probe) in [
            ("untrusted.example", "https", 443, "http"),
            ("api.github.com.untrusted.example", "https", 443, "tcp"),
            ("api.github.com", "http", 443, "http"),
            ("api.github.com", "https", 8443, "tcp"),
            ("api.github.com", "https", 65_979, "http"),
            ("api.github.com", "https", 443, "unknown"),
        ] {
            let wait = json!({"host":host,"scheme":scheme,"port":port,"probe":probe});
            let result = probe_external_with_lookup(
                &wait,
                |_| -> std::future::Ready<io::Result<Vec<SocketAddr>>> {
                    panic!("unsupported wait must not resolve");
                },
            )
            .await;
            assert!(!result.0);
            assert!(result.1.ends_with("unsupported"));
        }
    }

    #[tokio::test]
    async fn allowed_hostname_with_non_public_dns_cannot_connect() {
        let wait = json!({"host":"api.github.com","scheme":"https","port":443,"probe":"http"});
        for address in [
            "0.0.0.0:443",
            "10.0.0.1:443",
            "127.0.0.1:443",
            "169.254.169.254:443",
            "172.16.0.1:443",
            "192.168.0.1:443",
            "100.64.0.1:443",
            "[::]:443",
            "[::1]:443",
            "[::ffff:127.0.0.1]:443",
            "[fe80::1]:443",
            "[fc00::1]:443",
            "[ff02::1]:443",
            "[64:ff9b::a00:1]:443",
        ] {
            let result = probe_external_with_lookup(&wait, |host| {
                assert_eq!(host, "api.github.com");
                // Even a mixed response cannot authorize a connection.
                ready(Ok([
                    "1.1.1.1:443".parse::<SocketAddr>().unwrap(),
                    address.parse::<SocketAddr>().unwrap(),
                ]))
            })
            .await;
            assert_eq!(
                result,
                (
                    false,
                    "outage probe refused a non-public or mismatched DNS address".into()
                ),
                "{address}"
            );
        }
    }

    #[test]
    fn external_probe_selects_only_a_numeric_public_address() {
        let first: SocketAddr = "[2606:4700::1]:443".parse().unwrap();
        let second: SocketAddr = "1.1.1.1:443".parse().unwrap();
        assert_eq!(pinned_external_address([first, second]), Ok(first));
        assert!(pinned_external_address([]).is_err());
        assert!(pinned_external_address(["1.1.1.1:8443".parse().unwrap()]).is_err());
    }

    #[tokio::test]
    async fn head_uses_a_config_free_client_with_an_exact_dns_pin() {
        for (address, pin) in [
            ("1.1.1.1:443", "api.github.com:443:1.1.1.1"),
            ("[2606:4700::1]:443", "api.github.com:443:[2606:4700::1]"),
        ] {
            let mut command = tokio::process::Command::new("/usr/bin/curl");
            configure_external_head(&mut command, "api.github.com", address.parse().unwrap());
            let command = command.as_std();
            assert_eq!(command.get_program(), "/usr/bin/curl");
            assert_eq!(command.get_envs().count(), 0);
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [
                    "-q",
                    "--silent",
                    "--head",
                    "--output",
                    "/dev/null",
                    "--max-time",
                    "10",
                    "--proto",
                    "=https",
                    "--noproxy",
                    "*",
                    "--no-location",
                    "--resolve",
                    pin,
                    "--write-out",
                    "%{http_code}",
                    "--",
                    "https://api.github.com:443/",
                ]
            );
        }

        // Execute only a local environment oracle, not curl. printenv treats
        // our HEAD arguments as further names after its own end-of-options.
        let mut oracle = tokio::process::Command::new("/usr/bin/printenv");
        oracle
            .args(["--", "HOME", "PATH", "HTTPS_PROXY", "CORTEX_TEST_SENTINEL"])
            .env("CORTEX_TEST_SENTINEL", "synthetic");
        configure_external_head(
            &mut oracle,
            "api.github.com",
            "1.1.1.1:443".parse().unwrap(),
        );
        let out = oracle.output().await.unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
    }
}
