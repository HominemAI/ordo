//! `ordo-console`: read the settings, boot, serve. It starts no process:
//! `ordo-supervisor` runs the deployment, and the console manages it over
//! the supervisor's socket.
//!
//! `ordo-console recover [--owner <id>]` is the lockout failsafe, the same
//! as `ordo console recover`, for a console without the `ordo` CLI. `ordo-console forget-door-key` forgets the console key, so a
//! console with `ORDO_IDENTITY_URL` set can start (`crate::recover`).

use std::process::exit;

use ordo_console::{BootOptions, Console, ConsoleConfig, ConsoleError};

fn report(e: &ConsoleError) -> String {
    let mut out = e.to_string();
    let mut cause = std::error::Error::source(e);
    while let Some(c) = cause {
        out.push_str(&format!("\n  caused by: {c}"));
        cause = c.source();
    }
    out
}

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if matches!(
        args.get(1).and_then(|a| a.to_str()),
        Some("recover" | "forget-door-key")
    ) {
        let rest: Vec<String> = args
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let (code, report) = ordo_console::recover::run_cli(&rest);
        if code == 0 {
            print!("{report}");
        } else {
            eprint!("{report}");
        }
        exit(code);
    }
    ordo_console::harden::not_dumpable();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ordo-console: the async runtime did not start: {e}");
            exit(1);
        }
    };
    runtime.block_on(serve());
}

async fn serve() {
    let cfg = match ConsoleConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ordo-console: refusing to start. {}", report(&e));
            exit(2);
        }
    };
    let console = match Console::boot(cfg, BootOptions::default()).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ordo-console: refusing to start. {}", report(&e));
            exit(2);
        }
    };
    if let Err(e) = console.serve().await {
        eprintln!("ordo-console: {}", report(&e));
        exit(1);
    }
}
