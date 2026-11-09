use std::path::PathBuf;
use workspace_service::{Config, LaunchSpec, Service};

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        // Never publish child diagnostics, environment or command lines.
        eprintln!("workspace-service: startup_or_service_failed");
        std::process::exit(1);
    }
}
async fn run() -> std::io::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    // Trusted native/systemd launch only, never socket or webview arguments.
    // Accept a source checkout or this channel's own installed bundle only.
    if args.len() != 3 && args.len() != 5 {
        return Err(std::io::Error::other(
            "expected allowed-root python checkout [--fixture-launcher path]",
        ));
    }
    let checkout = PathBuf::from(&args[2]);
    if !checkout.is_absolute()
        || !checkout.join("src/cortex_acp.py").is_file()
        || !checkout
            .join("crates/workspace-service/agent_launcher.py")
            .is_file()
    {
        return Err(std::io::Error::other("explicit_checkout_required"));
    }
    let checkout = checkout.canonicalize()?;
    let bundled = PathBuf::from("/usr/lib")
        .join(&workspace_protocol::channel::current().package)
        .join("runtime");
    if args.len() != 5
        && !checkout.join(".git").exists()
        && !(checkout == bundled && checkout.join("runtime-payload.json").is_file())
    {
        return Err(std::io::Error::other("channel_runtime_required"));
    }
    let launcher = if args.len() == 5 {
        // Fake-engine qualification uses the same checkout and the real service.
        if args[3] != "--fixture-launcher" {
            return Err(std::io::Error::other("invalid_launcher"));
        }
        let path = PathBuf::from(&args[4]).canonicalize()?;
        if path != checkout.join("tests/test_workspace_journey.py")
            && path != checkout.join("tests/test_workspace_service_integration.py")
            && path != checkout.join("tests/test_workspace_pause_service.py")
        {
            return Err(std::io::Error::other("fixture_launcher_refused"));
        }
        path
    } else {
        checkout.join("crates/workspace-service/agent_launcher.py")
    };
    let config = Config::for_user(
        PathBuf::from(&args[0]),
        LaunchSpec::new(
            PathBuf::from(&args[1]),
            vec!["-B".into(), launcher.to_string_lossy().into_owned()],
        )?
        .with_tools(checkout.join("crates/workspace-service/machine_tools.py"))?,
    )?;
    let service = Service::bind(config).await?;
    let server = tokio::spawn(service.clone().serve());
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result=server=>result.map_err(|_|std::io::Error::other("server_failed"))?,
        _=term.recv()=>service.stop().await,
        _=tokio::signal::ctrl_c()=>service.stop().await,
    }
}
