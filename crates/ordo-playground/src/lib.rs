//! Library surface of the playground: the reverse compiler AND the request
//! handlers, so both can be exercised without booting a web server.
//!
//! `main.rs` is now the socket and nothing else. Everything a browser can
//! ask the playground, a test can ask it too — which is the difference
//! between a surface with one test and a surface that has any.

/// SQL → Ordo. The reverse compiler now lives in [`ordo_decompile`], because
/// this crate's other half is a web server and the decompiler needs neither
/// a runtime nor a socket — only a `Schema`. Re-exported under its old path
/// so `ordo_playground::decompile::decompile` still resolves.
pub use ordo_decompile as decompile;
pub mod server;
