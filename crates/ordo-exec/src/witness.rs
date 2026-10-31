//! `$chainwitness`: the record of a space's signed heads that a restore of
//! that space cannot roll back (`plan-production-ops.md` R2d).
//!
//! # Why `$checkpoints` cannot witness a restore
//!
//! A signed checkpoint says the chain reached `upto` with head `head`, and
//! [`crate::undo::continuity`] uses that to notice a chain that has moved
//! backwards. The rows live in `$checkpoints`, which is a platform table in
//! the **space's own database** (`COMMANDLOG_DDL`; `plan-space-routing.md`).
//! A point-in-time restore of that database therefore rolls the checkpoints
//! back with the chain. Afterwards the stored checkpoints agree with the
//! stored chain, nothing is orphaned, and a boot check reading them alone
//! reports `Intact` on a deployment that has just lost history.
//!
//! What R2b detects is the narrower case: a rewind a checkpoint row
//! survived, which is a `DELETE`, a bundle re-import or a per-table
//! `pg_restore`. The whole-database case is the one PITR produces, and it is
//! the one the boot check has to see.
//!
//! Ruled 2026-09-19 (`restore-witness`, option C): **every signed checkpoint
//! head is also recorded in the control-plane database**, and the boot check
//! compares against that record.
//!
//! # Where the record lives, and what each deployment shape gets
//!
//! `$chainwitness` is created on demand by this module against the **control
//! pool only**, the way `$attestsinks` and `$spacedb` are, and never by
//! `ensure_platform_tables`, so a target database never grows one.
//!
//! - *Single database.* The control pool is the pool the binary booted
//!   with, and it is the same database the space's `$checkpoints` are in.
//!   The witness then survives every restore that acts on rows or tables and
//!   does **not** survive a PITR of the whole database, because there is
//!   only one. That PITR is the control plane being restored, which the
//!   ruling assigns to the attestation receivers (R2d-iv). The howto says
//!   this rather than hiding it.
//! - *Routed.* An attached space's chain and checkpoints are on its target
//!   database; the witness stays on the control pool. A PITR of the target
//!   alone rolls `$commandlog` and `$checkpoints` back together and leaves
//!   the witness standing, which is the case the boot check exists for.
//!
//! # What a row says, and why `seq` is the ordering that matters
//!
//! Every row is one statement: *this space's chain reached position `upto`
//! with head `head`.* A `checkpoint` row is a copy of a `$checkpoints` row,
//! signature included, so a reader holding the public key verifies it the
//! way it verifies the original. A `restore` row is written by `ordo restore
//! --record` for the marker itself: `upto` is the marker's index plus one,
//! `head` is the marker entry's chain hash, `key_id` is the operator string
//! and `signature` is empty.
//!
//! `seq` orders the two kinds against each other, and that order is the
//! reading the boot check depends on. A checkpoint witnessed **before** the
//! newest `restore` row was signed over history that restore may have
//! discarded; one witnessed **after** it was signed over the history the
//! chain holds now. A bound rule over the markers cannot see that
//! difference, which is how a fresh rewind below an attested head would
//! otherwise read as explained.
//!
//! The two writers never race for a position: the checkpoint worker is
//! leader-only, and a restore is recorded while the space is refused.
//!
//! # The write order, and the invariant it buys
//!
//! [`crate::undo::sign_checkpoint`] writes the witness row **before** the
//! checkpoint row, inside the append-lock critical section but on the
//! control pool. The two writes are ordered and not atomic, because a routed
//! deployment has no transaction across two databases, so one of the two
//! failure directions has to be chosen. The order chosen gives **no stored
//! checkpoint without a witness row**, unless the control database was
//! itself restored:
//!
//! - *The witness write fails.* `sign_checkpoint` returns
//!   `CheckpointFailure::Unwitnessed`, the transaction rolls back and
//!   nothing is signed. The worker reports it and the next tick tries again.
//! - *The checkpoint commit fails after the witness landed.* The witness
//!   names a head the chain holds, because it was read under the lock from
//!   committed rows. The boot check finds it `Current` and harmless, the
//!   next signing lands the same or a later position, and the unique key
//!   absorbs the repeat.

use ordo_commandlog::{ChainHash, Checkpoint, SpaceId, Timestamp};

use crate::commandlog::{bytea_lit, bytea_of, chain_hash_of};
use crate::{sql_lit, Pool, PoolError};

/// The control-plane table this module owns.
pub const WITNESS_TABLE: &str = "$chainwitness";

/// The DDL, run on demand against the control pool and nowhere else.
pub const WITNESS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$chainwitness\" ( \
  \"seq\"       bigserial PRIMARY KEY, \
  \"space\"     text   NOT NULL, \
  \"kind\"      text   NOT NULL, \
  \"upto\"      bigint NOT NULL, \
  \"head\"      bytea  NOT NULL, \
  \"at\"        bigint NOT NULL, \
  \"key_id\"    text   NOT NULL, \
  \"signature\" bytea  NOT NULL, \
  UNIQUE (\"space\", \"kind\", \"upto\", \"head\") \
)";

/// Which of the two statements a row makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessKind {
    /// A copy of a signed `$checkpoints` row.
    Checkpoint,
    /// A restore marker, recorded by `ordo restore --record`.
    Restore,
}

impl WitnessKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WitnessKind::Checkpoint => "checkpoint",
            WitnessKind::Restore => "restore",
        }
    }

    fn parse(s: &str) -> Option<WitnessKind> {
        match s {
            "checkpoint" => Some(WitnessKind::Checkpoint),
            "restore" => Some(WitnessKind::Restore),
            _ => None,
        }
    }
}

impl std::fmt::Display for WitnessKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One row, read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Witnessed {
    pub seq: u64,
    pub kind: WitnessKind,
    pub upto: u64,
    pub head: ChainHash,
    pub at: Timestamp,
    pub key_id: String,
    pub signature: Vec<u8>,
}

impl Witnessed {
    /// This row as a [`Checkpoint`] for the space it names, so a reader
    /// holding the public key checks it with the routine that checks the
    /// original. Only meaningful for [`WitnessKind::Checkpoint`]; a restore
    /// row carries an operator string where a key id goes and no signature.
    pub fn as_checkpoint(&self, space: &SpaceId) -> Checkpoint {
        Checkpoint {
            space: space.clone(),
            upto: self.upto,
            head: self.head,
            at: self.at,
            key_id: self.key_id.clone(),
            signature: self.signature.clone(),
        }
    }
}

/// A handle on the control database's `$chainwitness`.
#[derive(Clone)]
pub struct Witness {
    control: Pool,
}

impl Witness {
    pub fn new(control: Pool) -> Witness {
        Witness { control }
    }

    /// The control pool this witness writes to, for a caller that needs to
    /// say which database holds the record.
    pub fn pool(&self) -> &Pool {
        &self.control
    }

    /// Create the table if it is not there.
    ///
    /// Retried once in the shape `cadence::ensure_tables` uses: two
    /// processes racing `CREATE TABLE IF NOT EXISTS` can collide in the
    /// catalog, and the second attempt then finds the table and stands down.
    pub async fn ensure_table(&self) -> Result<(), PoolError> {
        if self.control.rows(WITNESS_DDL).await.is_err() {
            self.control.rows(WITNESS_DDL).await?;
        }
        Ok(())
    }

    /// Record a signed checkpoint, signature included.
    pub async fn record_checkpoint(&self, cp: &Checkpoint) -> Result<(), PoolError> {
        self.insert(
            &cp.space,
            WitnessKind::Checkpoint,
            cp.upto,
            cp.head,
            cp.at,
            &cp.key_id,
            &cp.signature,
        )
        .await
    }

    /// Record a restore marker: `upto` is the marker's index plus one, so it
    /// reads as the position the chain reached with the marker in it, and
    /// `head` is the marker entry's own chain hash. The operator string goes
    /// where a key id goes, because nothing signed this.
    pub async fn record_restore(
        &self,
        space: &SpaceId,
        marker_idx: u64,
        marker_hash: ChainHash,
        operator: &str,
        at: Timestamp,
    ) -> Result<(), PoolError> {
        self.insert(
            space,
            WitnessKind::Restore,
            marker_idx.saturating_add(1),
            marker_hash,
            at,
            operator,
            &[],
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert(
        &self,
        space: &SpaceId,
        kind: WitnessKind,
        upto: u64,
        head: ChainHash,
        at: Timestamp,
        key_id: &str,
        signature: &[u8],
    ) -> Result<(), PoolError> {
        self.ensure_table().await?;
        self.control
            .rows(&format!(
                "INSERT INTO \"$chainwitness\" \
                 (\"space\", \"kind\", \"upto\", \"head\", \"at\", \"key_id\", \"signature\") \
                 VALUES ({}, {}, {upto}, {}, {}, {}, {}) \
                 ON CONFLICT (\"space\", \"kind\", \"upto\", \"head\") DO NOTHING",
                sql_lit(&space.0),
                sql_lit(kind.as_str()),
                bytea_lit(&head.0),
                at.0,
                sql_lit(key_id),
                bytea_lit(signature),
            ))
            .await
            .map(|_| ())
    }

    /// Every row for a space, in `seq` order.
    ///
    /// A missing table is an empty answer rather than an error: a deployment
    /// that has never signed a checkpoint has nothing witnessed, and that is
    /// the same fact as a table that has not been created yet. It is the
    /// state every reader here already has to handle.
    pub async fn rows(&self, space: &SpaceId) -> Result<Vec<Witnessed>, PoolError> {
        let sql = format!(
            "SELECT \"seq\", \"kind\", \"upto\", \"head\", \"at\", \"key_id\", \"signature\" \
             FROM \"$chainwitness\" WHERE \"space\" = {} ORDER BY \"seq\"",
            sql_lit(&space.0)
        );
        let rows = match self.control.rows(&sql).await {
            Ok(rows) => rows,
            Err(_) => {
                self.ensure_table().await?;
                self.control.rows(&sql).await?
            }
        };
        rows.iter()
            .map(|row| {
                let num = |i: usize| -> Result<u64, PoolError> {
                    row.get(i)
                        .and_then(|c| c.as_deref())
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| PoolError::Db("malformed $chainwitness row".into()))
                };
                let kind = row
                    .get(1)
                    .and_then(|c| c.as_deref())
                    .and_then(WitnessKind::parse)
                    .ok_or_else(|| PoolError::Db("$chainwitness row has no known kind".into()))?;
                Ok(Witnessed {
                    seq: num(0)?,
                    kind,
                    upto: num(2)?,
                    head: chain_hash_of(row.get(3).and_then(|c| c.as_ref()))
                        .ok_or_else(|| PoolError::Db("$chainwitness row has no head".into()))?,
                    at: Timestamp(num(4)?),
                    key_id: row
                        .get(5)
                        .and_then(|c| c.clone())
                        .ok_or_else(|| PoolError::Db("$chainwitness row has no key id".into()))?,
                    signature: bytea_of(row.get(6).and_then(|c| c.as_ref())).unwrap_or_default(),
                })
            })
            .collect()
    }

    /// The newest witnessed **checkpoint** for a space, by `seq`.
    ///
    /// What `ordo restore --record` defaults `pre_restore_head` from: after
    /// a whole-database PITR this is exactly the head the restore discarded,
    /// which is the value that makes the orphan it left explainable.
    pub async fn latest_checkpoint(&self, space: &SpaceId) -> Result<Option<Witnessed>, PoolError> {
        Ok(self
            .rows(space)
            .await?
            .into_iter()
            .rfind(|w| w.kind == WitnessKind::Checkpoint))
    }
}
