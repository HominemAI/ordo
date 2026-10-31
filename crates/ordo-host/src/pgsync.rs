//! `PgSync`: one connection on a thread of its own, behind a synchronous
//! `sql` call.
//!
//! `PgCredentialStore` and `PgDirectoryStore` each carry a private copy of
//! this bridge, because the traits they implement are synchronous and are
//! called from inside a blocking task in the door's handlers. The stores
//! enrolment adds (tickets, registrations) need the same bridge and would
//! otherwise be a third and fourth copy. They share this one.
//!
//! The rules are the same as theirs. One connection, so a store's footprint
//! stays off the request pool. No cache, so a change is visible to the very
//! next read on any replica. The error text names the table and nothing
//! else, because a `PoolError` can carry the statement, and the statement
//! carries an identifier.

use std::sync::mpsc::SyncSender;

use ordo_exec::{Pool, PoolConfig, PoolError, TextRow};
use ordo_identity::StoreFault;

struct Job {
    sql: String,
    reply: SyncSender<Result<Vec<TextRow>, PoolError>>,
}

/// A connection and the thread that drives it.
pub struct PgSync {
    /// `Option` so [`Drop`] can close the channel before joining the
    /// thread. Joining while still holding a sender is a deadlock.
    jobs: Option<tokio::sync::mpsc::UnboundedSender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The store's name, for the one error sentence it may say.
    name: &'static str,
}

/// Where the connection comes from: one dialled here, or one the
/// deployment already holds for its single-connection stores.
pub enum Source {
    Dial(PoolConfig),
    Shared(Pool),
}

impl PgSync {
    /// Stand the bridge up. Synchronous, and safe to call from inside a
    /// runtime, because the dial happens on a thread this function creates.
    pub fn start(source: Source, name: &'static str) -> Result<PgSync, StoreFault> {
        let (jobs, mut rx) = tokio::sync::mpsc::unbounded_channel::<Job>();
        let (ready, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), StoreFault>>(1);
        let thread = std::thread::Builder::new()
            .name(format!("ordo-{name}"))
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready.send(Err(StoreFault::unavailable(e.to_string())));
                        return;
                    }
                };
                rt.block_on(async move {
                    let pool = match source {
                        Source::Shared(p) => p.labelled(name),
                        Source::Dial(cfg) => {
                            match Pool::connect(PoolConfig { size: 1, ..cfg }).await {
                                Ok(p) => p,
                                Err(e) => {
                                    let _ = ready
                                        .send(Err(StoreFault::of_class(e.class(), e.to_string())));
                                    return;
                                }
                            }
                        }
                    };
                    let _ = ready.send(Ok(()));
                    while let Some(job) = rx.recv().await {
                        let out = pool.rows(&job.sql).await;
                        let _ = job.reply.send(out);
                    }
                });
            })
            .map_err(|e| StoreFault::unavailable(format!("{name} thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(StoreFault::unavailable(format!("{name} thread died"))),
        }
        Ok(PgSync {
            jobs: Some(jobs),
            thread: Some(thread),
            name,
        })
    }

    /// Run one statement and wait for it. The error is one sentence that
    /// names the store and never the statement, with the pool's class
    /// (`PoolError::class`) and, when busy, the pool error as its cause.
    pub fn sql(&self, sql: &str) -> Result<Vec<TextRow>, StoreFault> {
        let closed = || StoreFault::unavailable(format!("the {} store is closed", self.name));
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.jobs
            .as_ref()
            .ok_or_else(closed)?
            .send(Job {
                sql: sql.to_string(),
                reply,
            })
            .map_err(|_| closed())?;
        match ordo_exec::bridge::wait(&rx) {
            Ok(Ok(rows)) => Ok(rows),
            Ok(Err(e)) => Err(crate::auth::store_failure::pool_fault(
                e,
                &format!("the {} table could not be read", self.name),
            )),
            Err(_) => Err(closed()),
        }
    }
}

impl Drop for PgSync {
    fn drop(&mut self) {
        self.jobs.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A cell as text.
pub fn cell(row: &TextRow, i: usize) -> Option<String> {
    row.get(i).and_then(|c| c.clone())
}
