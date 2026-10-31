//! The Ordo command log: append-only, per-space, hash-chained history of
//! every write (spec §6 "Compilation" & "Atomicity", §8 policies).
//!
//! Every write compiles to per-row, actor-attributed commands through the
//! gate; commands from one invocation form a group committed in a single
//! transaction; groups chain per space over SHA-256 of a canonical byte
//! encoding. The log is the safety net under the whole write family — and
//! the event bus, today.
//!
//! Design companion: `docs/security-logging-design.md` §7. This crate holds
//! the types, the chain, an in-memory [`store::Store`], signed-shape
//! checkpoints, and revert *planning* (computing inverse commands — applying
//! them is the platform's job, through the normal gate).

pub mod chain;
pub mod command;
pub mod sign;
pub mod store;
pub mod value;

#[cfg(test)]
mod tests;

pub use chain::{
    decode_group, encode_group, hash_group, verify_chain_from, ChainHash, ChainedGroup,
    DecodeError, TamperAt, TamperKind,
};
pub use command::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, GroupShapeError,
    RestoreMarker, RestoreSource, SpaceId, Timestamp, RESTORE_COMPONENT,
};
pub use sign::{CheckpointVerifier, Ed25519Signer, SignerError};
pub use store::{
    checkpoint, invert_group, revert_correlation, revert_group, AppendError, Checkpoint,
    CheckpointError, InMemoryStore, RevertError, Signer, Store, VERIFY_BATCH,
};
pub use value::{Image, Value};
