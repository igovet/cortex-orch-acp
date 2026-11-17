//! Secret-free stdio relay; no runtime/store creation.
#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 2 {
        std::process::exit(64)
    }
    if cortex_engine::broker::relay(std::path::Path::new(&args[1]))
        .await
        .is_err()
    {
        eprintln!("cortex MCP relay refused");
        std::process::exit(65)
    }
}
