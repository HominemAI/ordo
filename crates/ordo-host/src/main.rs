//! `ordo-host` — the server.
//!
//! Three lines, and that is the design. Everything the entrypoint could get
//! wrong lives in [`ordo_host::runtime`], where `ordo dev` and the
//! conformance harness can reach the same assembly: `from_env` is the only
//! environment reader, `assemble` works over the validated struct, and
//! `serve` owns the socket and the shutdown order.
//!
//! A refusal is a non-zero exit with one line on stderr. There is no
//! `--help`, no flags and no config file: all configuration is environment,
//! which is what an orchestrator supplies and what an image manifest makes
//! reviewable.
//!
//! One subcommand: `ordo-host check-config` (G17, `plan-console.md` 7)
//! reads every setting and refuses as boot would, then exits 0 without
//! dialling or serving anything. The console runs it before every apply.

use ordo_host::runtime::{assemble, serve, BootRefusal, RuntimeConfig};

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("check-config") {
        match RuntimeConfig::from_env() {
            Ok(_) => {
                println!("ordo-host: the settings would boot");
                return;
            }
            Err(e) => {
                eprintln!("ordo-host: {e}");
                std::process::exit(1);
            }
        }
    }
    if let Err(e) = run().await {
        eprintln!("ordo-host: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), BootRefusal> {
    // Every environment refusal happens here, before the first dial.
    let cfg = RuntimeConfig::from_env()?;
    let rt = assemble(cfg).await?;
    rt.print_startup();
    serve(rt).await
}
