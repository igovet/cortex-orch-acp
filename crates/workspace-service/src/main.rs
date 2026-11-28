use std::path::PathBuf;
use workspace_service::{dev_extraction::DevExtraction, Config, LaunchSpec, Service};

fn main() {
    // Before any thread exists: every process this service starts inherits it.
    workspace_service::launch::default_build_parallelism();
    workspace_service::native_codex_runtime::dispatch();
    // A side-chat session's read-only project files server.
    workspace_service::side_chat_files::dispatch();
    // `sudo`'s askpass helper and the `sudo` shim of host commands.
    workspace_service::native_askpass::dispatch();
    service_main();
}

#[tokio::main]
async fn service_main() {
    if std::env::args().nth(1).as_deref() == Some("--native-maintenance-worker") {
        let args: Vec<_> = std::env::args_os().skip(2).collect();
        let ok = if args.len() == 2 {
            workspace_service::maintenance_worker(
                std::path::Path::new(&args[0]),
                &args[1].to_string_lossy(),
            )
            .await
            .is_ok()
        } else {
            false
        };
        std::process::exit(if ok { 0 } else { 1 });
    }
    if std::env::args().nth(1).as_deref()
        == Some(workspace_service::native_command_supervisor::MODE)
    {
        std::process::exit(
            if workspace_service::native_command_supervisor::run().is_ok() {
                0
            } else {
                1
            },
        );
    }
    if run().await.is_err() {
        // Never publish child diagnostics, environment or command lines.
        eprintln!("workspace-service: startup_or_service_failed");
        std::process::exit(1);
    }
}
async fn run() -> std::io::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    // The same shipped binary is the ancestry-authenticated secret-free MCP relay.
    // Its broker validates endpoint ownership/token/engine ancestry before forwarding.
    if args.len() == 1 {
        return cortex_engine::broker::relay(std::path::Path::new(&args[0]))
            .await
            .map_err(|_| std::io::Error::other("native_relay_refused"));
    }
    // Trusted native/systemd launch only, never socket or webview arguments.
    if args.len() != 2 {
        return Err(std::io::Error::other("expected allowed-root runtime-root"));
    }
    let extraction = DevExtraction::from_env()?;
    let checkout = runtime_root(PathBuf::from(&args[1]), extraction.as_ref())?;
    let mut launch = LaunchSpec::native(checkout)?;
    if let Some(extraction) = &extraction {
        launch = launch.with_dev_extraction(extraction);
    }
    let config = Config::for_user(PathBuf::from(&args[0]), launch)?;
    // The owner's tool search path: read from their shell in the background, never
    // blocking or failing start.
    workspace_service::tool_path::start(&config.state_dir);
    if let Some(extraction) = &extraction {
        if std::env::var_os("CORTEX_WORKSPACE_QUALIFICATION_VERIFIER").is_some() {
            match extraction
                .qualification_verifier(&config.allowed_root)
                .await
            {
                Ok(receipt) => println!("qualification_nested_verifier={receipt}"),
                Err(_) => {
                    eprintln!("workspace-service: qualification_nested_verifier_failed");
                    return Err(std::io::Error::other(
                        "qualification_nested_verifier_failed",
                    ));
                }
            }
        }
    }
    // Host facts for task-start preflights, cached once per service start.
    workspace_service::native_preflight::warm();
    let service = {
        let environment =
            workspace_service::native_routes::RouteEnvironment::for_workspace(&config)
                .map_err(|e| std::io::Error::other(e.code()))?;
        workspace_service::store_locations::configure(&environment.state)
            .map_err(|e| std::io::Error::other(e.code()))?;
        Service::bind_native(
            config,
            std::sync::Arc::new(workspace_service::native_routes::ConfiguredRoutes {
                environment,
                relay: std::env::current_exe()?,
            }),
        )
        .await?
    };
    // Remote access never keeps the service from running: a host that cannot start is
    // reported by `remote.status`.
    service.start_remote().await;
    // Open owner actions (a missing login, sudo) are checked again in the background.
    service.start_owner_actions();
    service.start_mcp_signin();
    // Installed notifications can relaunch the fixed channel desktop when no window
    // claims a click. An extracted run must not use that installed activation path.
    if extraction.is_none() {
        service.start_notifications().await;
    }
    let server = tokio::spawn(service.clone().serve());
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result=server=>result.map_err(|_|std::io::Error::other("server_failed"))?,
        _=term.recv()=>service.stop().await,
        _=tokio::signal::ctrl_c()=>service.stop().await,
    }
}

fn runtime_root(
    requested: PathBuf,
    extraction: Option<&DevExtraction>,
) -> std::io::Result<PathBuf> {
    if let Some(extraction) = extraction {
        extraction.validate_runtime(&requested)?;
        return Ok(requested);
    }
    // With no Dev opt-in, preserve the source checkout and installed package paths.
    if !requested.is_absolute() || !requested.is_dir() {
        return Err(std::io::Error::other("explicit_checkout_required"));
    }
    let root = requested.canonicalize()?;
    let bundled = PathBuf::from("/usr/lib")
        .join(&workspace_protocol::channel::current().package)
        .join("runtime");
    if !root.join(".git").exists()
        && !(root == bundled && root.join("runtime-payload.json").is_file())
    {
        return Err(std::io::Error::other("channel_runtime_required"));
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::runtime_root;
    use std::{fs, os::unix::fs::DirBuilderExt};

    #[test]
    fn an_extracted_runtime_without_opt_in_is_not_an_installed_bundle() {
        let path = std::env::temp_dir().join(format!("cw-no-relocation-{}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        fs::write(path.join("runtime-payload.json"), b"{}").unwrap();
        let denied = runtime_root(path.clone(), None);
        fs::create_dir(path.join(".git")).unwrap();
        let source = runtime_root(path.clone(), None);
        fs::remove_dir_all(&path).unwrap();
        assert_eq!(denied.unwrap_err().to_string(), "channel_runtime_required");
        assert_eq!(source.unwrap(), path);
    }
}
