use std::path::PathBuf;
use workspace_service::{Config, LaunchSpec, Service};

fn main() {
    workspace_service::native_codex_runtime::dispatch();
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
    // Accept a source checkout or this channel's own installed bundle only.
    if args.len() != 2 {
        return Err(std::io::Error::other("expected allowed-root runtime-root"));
    }
    let checkout = PathBuf::from(&args[1]);
    if !checkout.is_absolute() || !checkout.is_dir() {
        return Err(std::io::Error::other("explicit_checkout_required"));
    }
    let checkout = checkout.canonicalize()?;
    let bundled = PathBuf::from("/usr/lib")
        .join(&workspace_protocol::channel::current().package)
        .join("runtime");
    if !checkout.join(".git").exists()
        && !(checkout == bundled && checkout.join("runtime-payload.json").is_file())
    {
        return Err(std::io::Error::other("channel_runtime_required"));
    }
    let config = Config::for_user(PathBuf::from(&args[0]), LaunchSpec::native(checkout)?)?;
    let service = {
        let mut environment = workspace_service::native_routes::RouteEnvironment::for_user()
            .map_err(|e| std::io::Error::other(e.code()))?;
        if workspace_protocol::channel::current().is_dev() {
            environment.config = config.launch_config_root().unwrap_or(environment.config);
            environment.state = config.state_dir.join("state");
        }
        Service::bind_native(
            config,
            std::sync::Arc::new(workspace_service::native_routes::ConfiguredRoutes {
                environment,
                relay: std::env::current_exe()?,
            }),
        )
        .await?
    };
    let server = tokio::spawn(service.clone().serve());
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result=server=>result.map_err(|_|std::io::Error::other("server_failed"))?,
        _=term.recv()=>service.stop().await,
        _=tokio::signal::ctrl_c()=>service.stop().await,
    }
}
