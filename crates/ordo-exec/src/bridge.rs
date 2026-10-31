//! Waiting for a store thread from synchronous code.
//!
//! The single-connection stores (keys, credentials, directory, tickets,
//! passkeys and the rest) keep one connection on a thread of their own and
//! answer through a channel. Their traits are synchronous, and some callers
//! are async handlers that reach them without a blocking task.
//!
//! On a store built over the deployment's shared pool, the store's answer
//! travels over a connection whose driver task lives on the door's runtime.
//! A runtime worker that waits for that answer with a plain blocking
//! receive takes itself out of the runtime while the answer needs the
//! runtime to arrive. One such worker is enough if it is the one polling
//! the socket, and as many waiting workers as there are workers is enough
//! whichever they are. Every thread then sits idle, the listener is never
//! polled, and the connections queue in the kernel (checklist 25f).
//!
//! [`in_place`] does the waiting through `block_in_place` when the caller is
//! a worker of a multi-thread runtime. That hands the worker's share of the
//! runtime, including the I/O driver, to another thread for as long as the
//! wait lasts.

use std::sync::mpsc::{Receiver, RecvError};

use tokio::runtime::{Handle, RuntimeFlavor};

/// Run `f`, which blocks its thread, without stopping the runtime the
/// caller is running on.
///
/// From a thread outside any runtime, from a blocking-pool thread and from a
/// current-thread runtime it is a plain call: none of those holds a share of
/// a multi-thread runtime that `f` could be waiting on.
pub fn in_place<R>(f: impl FnOnce() -> R) -> R {
    match Handle::try_current() {
        Ok(h) if h.runtime_flavor() == RuntimeFlavor::MultiThread => tokio::task::block_in_place(f),
        _ => f(),
    }
}

/// `rx.recv()`, through [`in_place`].
pub fn wait<T>(rx: &Receiver<T>) -> Result<T, RecvError> {
    in_place(|| rx.recv())
}
