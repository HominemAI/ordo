//! `ordo-supervisor`: read the settings, bind the socket and the health
//! port, start the 3 children, and stop them in order on SIGTERM.
//!
//! Started as `ordo-supervisor --supervised-child ...` it is the launcher
//! each child is started through ([`ordo_supervisor::child`]): it runs
//! before any thread or runtime exists, and execs the child's program.

use std::process::exit;

use ordo_supervisor::child::{harden, launcher_main, LAUNCH_ARG};
use ordo_supervisor::error::report;
use ordo_supervisor::{health, socket, Supervisor, SupervisorConfig, Timings};

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).and_then(|a| a.to_str()) == Some(LAUNCH_ARG) {
        launcher_main(args);
    }
    harden();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ordo-supervisor: the async runtime did not start: {e}");
            exit(1);
        }
    };
    exit(runtime.block_on(serve()));
}

fn refuse(e: &dyn std::error::Error) -> i32 {
    eprintln!("ordo-supervisor: refusing to start. {}", report(e));
    2
}

async fn serve() -> i32 {
    let mut cfg = match SupervisorConfig::from_env() {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    // The run folder first: the socket lives in it.
    if let Err(e) = cfg.settle_run_dir() {
        return refuse(&e);
    }
    let launcher = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ordo-supervisor: its own binary could not be found: {e}");
            return 1;
        }
    };
    // Make the run folder first: the socket lives in it.
    if let Err(e) = ordo_supervisor::child::run_dir(&cfg.run_dir) {
        let e = ordo_supervisor::SupervisorError::setting_caused(
            ordo_supervisor::config::RUN_DIR_ENV,
            format!(
                "is `{}`, and the supervisor could not make it",
                cfg.run_dir.display()
            ),
            "Mount a tmpfs there owned by the supervisor's user, for example --tmpfs \
             /run/ordo:uid=65530,mode=0711",
            e,
        );
        return refuse(&e);
    }
    let listener = match socket::bind(&cfg.socket, cfg.console.user) {
        Ok(l) => l,
        Err(e) => return refuse(&e),
    };
    let health_listener = match health::bind(cfg.listen).await {
        Ok(l) => l,
        Err(e) => return refuse(&e),
    };
    let sup = match Supervisor::start(&cfg, launcher, Timings::plan(cfg.grace, cfg.ready_timeout))
        .await
    {
        Ok(s) => s,
        Err(e) => return refuse(&e),
    };
    tokio::spawn(socket::serve(
        listener,
        socket::Callers::of(cfg.console.user),
        sup.clone(),
    ));
    tokio::spawn(health::serve(health_listener, sup.clone()));
    eprintln!(
        "ordo-supervisor: running; socket {}, /health/live on {}",
        cfg.socket.display(),
        cfg.listen
    );
    wait_for_stop().await;
    sup.shutdown().await;
    let _ = std::fs::remove_file(&cfg.socket);
    eprintln!("ordo-supervisor: stopped");
    0
}

async fn wait_for_stop() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => return std::future::pending().await,
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
