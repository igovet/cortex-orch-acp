use std::sync::Arc;
use tokio::io::BufReader;
use workspace_service::native_routes::{ConfiguredRoutes, RouteEnvironment};

fn main() {
    // This binary writes the native Codex launcher naming itself (first run and
    // per-session catalogue refresh), so it must also be that interpreter.
    workspace_service::native_codex_runtime::dispatch();
    acp_main();
}

#[tokio::main]
async fn acp_main() {
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
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    // Same executable serves the ancestry-authenticated, secret-free MCP relay.
    let result = if args.len() == 1 {
        cortex_engine::broker::relay(std::path::Path::new(&args[0]))
            .await
            .map_err(|_| std::io::Error::other("relay_refused"))
    } else if args.is_empty() {
        if std::env::var("CORTEX_MODEL_EFFORTS").as_deref() != Ok("1") {
            if let Err(errors) =
                cortex_acp::startup::verify_pinned(&cortex_acp::startup::runtime_root()).await
            {
                for error in errors {
                    eprintln!("{error}");
                }
                std::process::exit(1);
            }
        }
        match (RouteEnvironment::for_user(), std::env::current_exe()) {
            (Ok(environment), Ok(relay)) => {
                let environment = cortex_acp::startup::channel_environment(environment);
                if std::env::var("CORTEX_MODEL_EFFORTS").as_deref() == Ok("1") {
                    match cortex_acp::startup::model_efforts(&environment) {
                        Ok(efforts) => {
                            println!("{efforts}");
                            std::process::exit(0);
                        }
                        Err(error) => {
                            eprintln!("cortex_acp: model efforts unavailable: {}", error.code());
                            std::process::exit(1);
                        }
                    }
                }
                // Detached like Python's daemon thread; it ends with the process.
                let _first_run = cortex_acp::startup::first_run(&environment);
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("signal handler");
                cortex_acp::serve_until(
                    BufReader::new(tokio::io::stdin()),
                    tokio::io::stdout(),
                    Arc::new(ConfiguredRoutes { environment, relay }),
                    async move {
                        tokio::select! {_=term.recv()=>{},_=tokio::signal::ctrl_c()=>{}}
                    },
                )
                .await
            }
            _ => Err(std::io::Error::other("route_environment_unavailable")),
        }
    } else {
        Err(std::io::Error::other("unexpected_arguments"))
    };
    if result.is_err() {
        eprintln!("cortex-acp: transport_or_host_failed");
        std::process::exit(1);
    }
    // All host processes, store lanes and protocol output have settled. Tokio's
    // stdin helper itself cannot cancel a blocking read while the parent keeps
    // its pipe open after SIGTERM; do not wait for that unrelated thread.
    std::process::exit(0);
}
