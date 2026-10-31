//! The seam a timing crosses to reach the metrics registry (5f).
//!
//! # Why a trait here rather than a call to the registry
//!
//! The registry is `ordo_host::metrics::Metrics`, and `ordo-host` depends
//! on `ordo-exec`. The instrumented code — the per-space chain-append lock
//! — is in [`crate::commandlog`]. Calling the registry directly would
//! invert the graph and drag the deployment surface, the door, and every
//! sibling crate `ordo-host` pulls in behind a `+= 1`.
//!
//! So the dependency runs the way it already does and the timing is pushed
//! through a trait: `ordo-exec` states what it can report, and the crate
//! that owns a metrics registry implements it. It is the same shape
//! `ordo_host::metrics::MeteredKeys` takes over `KeyProvider` — a decorator
//! seam, chosen for the same reason. A deployment that wants no metering
//! passes nothing, and the observation compiles down to an `Option` that is
//! always `None`.
//!
//! # Why the pool carries it and not the transaction
//!
//! [`crate::TxConn`] already holds its `Pool`, and a `Pool` clone is a
//! handle over shared connections. Hanging the observer on the handle means
//! it is inherited by every transaction begun from it, costs no lock to
//! read, and can differ between two handles onto the same connections —
//! which is what lets a test observe one pool while the deployment's own
//! handle stays unobserved.
//!
//! # What an implementation must not do
//!
//! Both methods are called **inside an open transaction**, one of them
//! while the append lock is held. An implementation that blocks, does I/O,
//! or panics widens the critical section it exists to measure —
//! `implementation.md` names widening it as the write path's failure mode,
//! and an observer that caused it would be the worst possible instrument.
//! `Metrics::observe` is a `Mutex` and a `Vec` index, which is the budget.

/// A sink for the chain-append lock's timings.
pub trait AppendObserver: Send + Sync {
    /// Seconds spent **blocked** in `pg_advisory_xact_lock` — the question
    /// `implementation.md` calls *"is this biting yet"*.
    ///
    /// This is the wait alone. It excludes everything before the lock
    /// (parse, plan, access check, statement execution, all of which happen
    /// outside it by design) and everything after it.
    fn lock_wait(&self, space: &str, seconds: f64);

    /// Seconds spent in the critical section *after* the lock was granted:
    /// head read, duplicate-correlation check, hash, insert.
    ///
    /// Not the whole time the lock is held — it is transaction-scoped, so
    /// the caller keeps it until commit and this function cannot see that
    /// far. It is the part of the section this crate owns and the part a
    /// change to this file can widen, which is the part worth watching.
    fn append_work(&self, space: &str, seconds: f64);

    /// Seconds one checkout spent waiting for admission to the pool: its
    /// space's share plus the pool's own bound, which share one budget.
    ///
    /// The healthy path, and the one that costs a series per space, so the
    /// implementation is where the per-space cardinality decision is made.
    /// Reported only for a handle charged to a space
    /// ([`crate::Pool::for_space`]); a checkout charged to none is silent
    /// rather than reported under an invented name.
    ///
    /// Default no-op, like the two below: an implementation that only ever
    /// wanted the append lock's timings keeps compiling and keeps meaning
    /// what it meant.
    fn pool_wait(&self, _space: &str, _seconds: f64) {}

    /// This space reached its own cap and waited on itself (20a). The
    /// series that answers *"which space is holding the connections"*,
    /// because the space that is capped is the space that is holding them.
    fn pool_capped(&self, _space: &str) {}

    /// A checkout for this space was refused: the acquire budget elapsed
    /// against the pool's bound or against the space's share of it.
    fn pool_saturated(&self, _space: &str) {}

    /// One try of a serializable write lost a race with another write
    /// (`40001` or `40P01`), whether or not a retry followed. The series
    /// that shows a hot row before its writers see `serialization_conflict`.
    /// `space` is empty for a handle charged to none.
    fn write_conflict(&self, _space: &str) {}
}
