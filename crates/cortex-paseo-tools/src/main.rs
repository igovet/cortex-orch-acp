//! `cortex-paseo-tools <command> <args...>`: one read-only store read per
//! process, for the Paseo plugin's Rust launch target.
use std::io::Write;

fn main() {
    // `setup` and `config` write the native Codex launcher naming this binary,
    // so it must also be that launcher's interpreter.
    workspace_service::native_codex_runtime::dispatch();
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    let Some((stdout, code)) = cortex_paseo_tools::run(&command, &rest) else {
        eprintln!(
            "usage: cortex-paseo-tools <{}> <args...>",
            cortex_paseo_tools::COMMANDS
                .iter()
                .chain(cortex_paseo_tools::paseo_setup::COMMANDS.iter())
                .chain(cortex_paseo_tools::dev_tools::COMMANDS.iter())
                .copied()
                .collect::<Vec<_>>()
                .join("|")
        );
        std::process::exit(2);
    };
    let mut out = std::io::stdout().lock();
    if out
        .write_all(stdout.as_bytes())
        .and_then(|()| out.flush())
        .is_err()
    {
        std::process::exit(1);
    }
    std::process::exit(code);
}
