//! `ordo-identity-service`: read the settings, boot, and serve; or run one
//! host command (`invite`, `break-glass-token`, `rotate-keys`,
//! `audit verify`, [`ordo_identity_service::commands`]) and exit.

use std::process::exit;

use ordo_identity_service::commands::{self, COMMANDS};
use ordo_identity_service::{BootOptions, IdentityConfig, IdentityError, Service};

fn report(e: &IdentityError) -> String {
    let mut out = e.to_string();
    let mut cause = std::error::Error::source(e);
    while let Some(c) = cause {
        out.push_str(&format!("\n  caused by: {c}"));
        cause = c.source();
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = args.first() {
        if !COMMANDS.contains(&first.as_str()) {
            eprintln!(
                "ordo-identity-service: `{first}` is not a command. Commands: {}. Run with no \
                 arguments to serve.",
                COMMANDS.join(", ")
            );
            exit(2);
        }
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ordo-identity-service: the async runtime did not start: {e}");
            exit(1);
        }
    };
    runtime.block_on(start(args));
}

async fn start(args: Vec<String>) {
    let cfg = match IdentityConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ordo-identity-service: refusing to start. {}", report(&e));
            exit(2);
        }
    };
    let service = match Service::boot(cfg, BootOptions::default()).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ordo-identity-service: refusing to start. {}", report(&e));
            exit(2);
        }
    };
    if !args.is_empty() {
        let (code, out) = commands::run(&service, &args, &commands::unix_user()).await;
        if code == 0 {
            print!("{out}");
        } else {
            eprint!("{out}");
        }
        exit(code);
    }
    if let Err(e) = service.serve().await {
        eprintln!("ordo-identity-service: {}", report(&e));
        exit(1);
    }
}
