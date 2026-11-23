//! Synthetic token authority and pinned native peers, entirely inside the offline guest.
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use cortex_engine::wire::{read_frame, write_frame};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::{Child, ChildStdin, ChildStdout},
};

struct AppServer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl AppServer {
    async fn start(home: &Path, endpoint: &str, confined: bool) -> Self {
        let mut command = tokio::process::Command::new("/usr/bin/codex");
        command
            .args(crate::native_codex_runtime::BASE)
            .arg("app-server")
            .current_dir(home.join("checkout"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home)
            .env("CODEX_HOME", home.join(".codex"))
            .env("CODEX_REFRESH_TOKEN_URL_OVERRIDE", endpoint)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if confined {
            on_exec(command.as_std_mut(), "engine", home).unwrap();
        }
        let mut child = command.spawn().unwrap();
        let mut peer = AppServer {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
        };
        peer.send(json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"clientInfo":{"name":"cortex-offline-fixture","version":"1"}}
        }))
        .await;
        assert!(peer.reply(1).await["result"].is_object());
        if confined {
            label(peer.child.id().unwrap(), "engine", home);
        }
        peer.send(json!({"jsonrpc":"2.0","method":"initialized"}))
            .await;
        peer
    }

    async fn send(&mut self, body: Value) {
        write_frame(&mut self.input, &body).await.unwrap();
    }

    async fn reply(&mut self, id: u64) -> Value {
        tokio::time::timeout(ACP_TIMEOUT, async {
            loop {
                let value = read_frame(&mut self.output)
                    .await
                    .unwrap()
                    .expect("app-server EOF");
                if value["id"] == id {
                    return value;
                }
            }
        })
        .await
        .expect("missing app-server reply")
    }

    async fn request_refresh(&mut self, id: u64) {
        self.send(json!({
            "jsonrpc":"2.0", "id": id, "method":"account/read","params":{"refreshToken":true}
        }))
        .await;
    }
}

async fn serve_refresh(listener: &TcpListener, access_token: &str) {
    tokio::time::timeout(ACP_TIMEOUT, async {
        let (socket, address) = listener.accept().await.unwrap();
        assert!(address.ip().is_loopback());
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("POST /oauth/token HTTP/1."));
        let mut content_length = None;
        let mut header_bytes = line.len();
        loop {
            line.clear();
            let count = reader.read_line(&mut line).await.unwrap();
            assert!(count > 0);
            header_bytes += count;
            assert!(header_bytes <= 16_384);
            if line == "\r\n" {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                if key.eq_ignore_ascii_case("content-length") {
                    assert!(content_length.is_none());
                    content_length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
        }
        let size = content_length.unwrap();
        assert!(size <= 4096);
        let mut content = vec![0; size];
        reader.read_exact(&mut content).await.unwrap();
        let request: Value = serde_json::from_slice(&content).unwrap();
        assert_eq!(request["refresh_token"], "fixture-refresh-only");
        assert_eq!(request["grant_type"], "refresh_token");
        let body = json!({"access_token": access_token}).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        reader.get_mut().write_all(response.as_bytes()).await.unwrap();
        reader.get_mut().shutdown().await.unwrap();
    }).await.expect("no complete synthetic OAuth exchange");
}

fn access_token(path: &Path) -> String {
    let data: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    data["tokens"]["access_token"].as_str().unwrap().to_owned()
}

#[tokio::test]
#[ignore = "requires a NIC-less synthetic QEMU guest with the pinned Codex CLI"]
async fn guest_codex_refresh() {
    let home = home();
    let auth = home.join(".codex/auth.json");
    let lock = home.join(".codex/auth.lock");
    assert!(!auth.exists() && !lock.exists());
    let claims = URL_SAFE_NO_PAD.encode(
        json!({"email":"fixture@example.invalid","https://api.openai.com/auth":{
            "chatgpt_account_id":"00000000-0000-4000-8000-000000000002",
            "chatgpt_user_id":"fixture-user","chatgpt_plan_type":"plus"
        }})
        .to_string(),
    );
    fs::write(
        &auth,
        json!({
            "auth_mode":"chatgpt",
            "tokens":{
                "id_token":format!("e30.{claims}.fixture"),
                "access_token":"initial-synthetic-access",
                "refresh_token":"fixture-refresh-only",
                "account_id":"00000000-0000-4000-8000-000000000002"
            },
            "last_refresh":"2099-01-01T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();
    let auth_id = fs::metadata(&auth).unwrap().ino();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/oauth/token", listener.local_addr().unwrap());
    let mut service = AppServer::start(&home, &endpoint, true).await;
    service.request_refresh(2).await;
    serve_refresh(&listener, "first-synthetic-access").await;
    let _ = service.reply(2).await;
    assert_eq!(access_token(&auth), "first-synthetic-access");
    assert_eq!(fs::metadata(&auth).unwrap().ino(), auth_id);
    let lock_id = fs::metadata(&lock).unwrap().ino();

    let mut native = AppServer::start(&home, &endpoint, false).await;
    let barrier = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock)
        .unwrap();
    barrier.lock().unwrap();
    service.request_refresh(3).await;
    native.request_refresh(3).await;
    serve_refresh(&listener, "concurrent-synthetic-a").await;
    serve_refresh(&listener, "concurrent-synthetic-b").await;
    assert_eq!(access_token(&auth), "first-synthetic-access");
    barrier.unlock().unwrap();
    let _ = tokio::join!(service.reply(3), native.reply(3));
    assert!(matches!(
        access_token(&auth).as_str(),
        "concurrent-synthetic-a" | "concurrent-synthetic-b"
    ));
    assert_eq!(fs::metadata(&auth).unwrap().ino(), auth_id);
    assert_eq!(fs::metadata(&lock).unwrap().ino(), lock_id);
    for peer in [&mut service, &mut native] {
        peer.child.kill().await.unwrap();
    }
    println!("CORTEX_CODEX_REFRESH_SHARED_LOCK_PASS");
}
