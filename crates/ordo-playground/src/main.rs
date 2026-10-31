//! The Ordo playground: a local web page over the compiler.
//!
//! Forward (Ordo → SQL) is the real pipeline — parse, bind, plan. Reverse
//! (SQL → Ordo) is the v0 decompiler, and every decompilation is verified by
//! compiling its output forward before it is shown; with "run" ticked the
//! page executes BOTH your SQL and the recompiled Ordo and says so out loud
//! when the two disagree.
//!
//! Reads run **as a chosen member**, so the guard narrowing is something you
//! watch rather than read about; writes run the real Stage-5 protocol and
//! show the command group they appended; and the manifest panel renders
//! exactly what the catalog publishes for an op.
//!
//! All three databases are the playground's own and are rebuilt at boot, so
//! a session never touches — or depends on — the conformance suites beside
//! it.
//!
//!   cargo run -p ordo-playground        # http://127.0.0.1:7878
//!
//! Everything above lives in `ordo_playground::server`; this file opens the
//! socket.

use std::sync::Arc;

use ordo_playground::server::{router, App};

#[tokio::main]
async fn main() {
    let app = App::load().expect("fixtures compile");
    for (db, r) in app.seed() {
        match r {
            Ok(()) => println!("seeded: {db}"),
            Err(e) => println!("UNAVAILABLE: {db} ({e}) — panes over it will not run"),
        }
    }

    // `ORDO_PLAYGROUND_ADDR`, defaulting to loopback so `cargo run` is
    // unchanged. A container sets it to `0.0.0.0:7878` — a process bound to
    // loopback answers nothing from outside the container it runs in.
    let addr = std::env::var("ORDO_PLAYGROUND_ADDR").unwrap_or_else(|_| "127.0.0.1:7878".into());
    println!("ordo playground: http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    axum::serve(listener, router(Arc::new(app)))
        .await
        .expect("serve");
}
