//! **Space deletion by crypto-erasure** — the other exit op.
//!
//! `definition-language.md` §8: *"Full space export and space deletion
//! always exist — and are owner-only, step-up gated, command-logged ops
//! (export = total disclosure; deletion = the crypto-erasure path, DEK
//! destruction after an explicit confirmation window)."* `migration.md` §6
//! restates it, and the compliance pack maps it to GDPR Art.17 /
//! CCPA delete.
//!
//! # Two acts, in this order, and the order is the argument
//!
//! 1. **Destroy the space's key material** —
//!    [`KeyProvider::destroy_space`]: every DEK and the SMK under them.
//! 2. **Delete the rows** — every space-scoped row in every table.
//!
//! Neither act alone is deletion. Row deletion answers the *live* database
//! and nothing else: every backup, every WAL segment, every replica and
//! every archive still holds the bytes, and a deployment that promised
//! erasure and delivered a `DELETE` has promised something it cannot keep.
//! Key destruction answers all of those at once — for the sealed fields —
//! and answers the live database not at all, because plaintext columns are
//! plaintext.
//!
//! The order is keys-first because the two failure modes are not
//! symmetrical. Keys destroyed and rows surviving is a space whose sealed
//! data is *already unrecoverable* and whose remaining rows a re-run
//! removes. Rows deleted and keys surviving is a space that *looks* deleted
//! while every backup of it stays openable — the state that reads as done
//! and is not. When a partial failure is possible, it should land on the
//! stronger guarantee.
//!
//! # What deletion refuses, and what it merely discloses
//!
//! The prune machinery already knows what a space's chain owes, and
//! deletion respects the same list read one way:
//!
//! - **A non-evictable consumer behind the head is a refusal.**
//!   [`ConsumerKind::evictable`] is true for `Mirror` and nothing else, and
//!   the three that are not evictable are not evictable for reasons that
//!   apply here verbatim: an `Archive` is *the copy of record* and history
//!   it has not received yet dies with the space; a `ClientFirstMirror` is
//!   the authoritative store, and erasing the keys strands an enrolled
//!   device rather than freeing it; a `Listener` behind the head has
//!   at-least-once deliveries outstanding, and deletion would turn them
//!   into zero-times. The remedy for each is the same one prune already
//!   documents — let it catch up, or de-register it, which is an
//!   attributable act somebody performs on purpose.
//! - **The undo horizon is disclosed, not enforced.** Prune refuses to cut
//!   inside the horizon because the platform guarantees the *owner* undo
//!   for that window. Deletion is the owner spending that guarantee, which
//!   is theirs to spend; refusing would make the platform the owner of
//!   their data's end. So the report carries how many groups are still
//!   inside the horizon, the op puts that number in front of the human, and
//!   the confirmation is what consumes it — the same shape
//!   `roadmap-checklist.md` 13e gives destructive migration ("explicit
//!   confirmation and the undo-horizon disclosure").
//! - **A stalled evictable mirror does not block.** It does not block a
//!   prune either, and for the identical reason: it is a cache.
//!
//! # Where the record goes, and why it is not the space's chain
//!
//! §8 calls both exit ops "command-logged". For export that is literal —
//! the owner cannot read the whole space without leaving a command in the
//! space's own chain. For deletion it cannot be, and the reason is not an
//! implementation gap: **the chain is one of the things deletion
//! destroys.** A final group appended to a log that is deleted in the same
//! transaction is not a record of anything, and a log that survives its
//! space is a space that was not deleted.
//!
//! So the deletion's record is the **deployment's security-event stream**,
//! which outlives the tenant by construction and is exactly where the
//! neighbouring custody acts already go — rotation, epoch destruction,
//! custody conversion, all `EventKind::CustodyAction`. Break-glass, when
//! built as the grant-gated operation the owner ruled (corpus-rulings.md,
//! 2026-09-07), lands here too. That stream is the operator's
//! tamper-evident record, and the owner is named in it as the subject.
//! Recording it is the *caller's* job (the op layer
//! holds the recorder); this module returns everything the record needs.
//!
//! The security stream for the space is, for the same reason, **the one
//! thing not erased**. See [`RETAINED`].

use ordo_crypto::{KeyProvider, SpaceId};
use ordo_schema::Schema;

use crate::cadence::LogPolicy;
use crate::commandlog::{space_lock_key, APPEND_LOCK_NS};
use crate::prune::{Consumer, ConsumerKind};
use crate::{sql_lit, OpClass, Pool, PoolError};

/// The space-scoped platform tables an erasure deliberately leaves alone.
///
/// One entry, and it needs its reasons stated because "delete everything
/// with this space's id in it" is the obvious rule and this is the
/// exception to it.
///
/// `$secevents` / `$seccheckpoints` hold the **security-event stream**,
/// which is:
///
/// - **A hash chain.** Deleting a prefix of one is the act `prune` refuses
///   as *"deleting it would destroy the evidence"*. Here the evidence in
///   question would include the deletion itself, which is the one entry
///   nobody may lose.
/// - **The operator's record, not the tenant's.** It carries decisions —
///   who was denied what, which step-up was demanded, when a key was
///   released — and never domain rows, never images, never a field value.
///   A space owner cannot revert it and could not before this op existed.
///
/// The honest cost, stated rather than hidden: the retained stream keeps
/// **principal identifiers and op names** associated with a deleted space.
/// That is a real residue and it is a deliberate one — an audit spine that
/// a subject can erase is not an audit spine. Retention of the stream is
/// governed by `policy retention`, which is where a jurisdiction's answer
/// belongs.
pub const RETAINED: &[&str] = &["$secevents", "$seccheckpoints"];

/// Why an erasure did not happen.
#[derive(Debug)]
#[non_exhaustive]
pub enum EraseRefusal {
    /// The caller did not confirm, or confirmed something else. Deletion is
    /// irreversible by design, so the confirmation is the space's own id
    /// typed back — not a boolean, which a retried request supplies for
    /// free, and not a token the platform hands out, which would make the
    /// platform the party that decided.
    NotConfirmed {
        expected: String,
        got: String,
    },
    /// The space is the deployment's own system space, which is not a
    /// tenant and has no exit.
    ///
    /// Every other space carries the opposite guarantee, so this refusal
    /// exists to keep "nobody erases the system space" a mechanism rather
    /// than a hope: exit ops always exist and are owner-only, so an owner
    /// of the system space could otherwise take the catalogue every tenant
    /// reads, along with its chain, in one confirmed call. Checked before
    /// the confirmation, because the confirmation is about proving intent
    /// and this is not a question of intent.
    SystemSpace {
        space: String,
    },
    /// A consumer that may not be evicted is behind the chain's head. See
    /// the module doc.
    ConsumerPinned {
        consumers: Vec<Consumer>,
        head: u64,
    },
    /// Custody would not destroy the space's key material, so nothing that
    /// follows would be erasure. Nothing has been deleted.
    Custody(String),
    Store(PoolError),
}

impl std::fmt::Display for EraseRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EraseRefusal::SystemSpace { space } => write!(
                f,
                "system_space: `{space}` is the deployment's own space, not a tenant's. It \
                 holds what every tenant reads and it has no exit."
            ),
            EraseRefusal::NotConfirmed { expected, got } => write!(
                f,
                "this deletion is not confirmed: it destroys `{expected}` irreversibly, and \
                 the confirmation is that space's own id typed back. Received `{got}`."
            ),
            EraseRefusal::ConsumerPinned { consumers, head } => write!(
                f,
                "refusing to delete a space whose chain still owes {} consumer(s) that may \
                 not be evicted; the head is at {head}:\n{}\nLet each catch up, or \
                 de-register it — which is an act somebody performs on purpose and the log \
                 records.",
                consumers.len(),
                consumers
                    .iter()
                    .map(|c| format!(
                        "  `{}` ({}) at position {} — {}",
                        c.name,
                        c.kind,
                        c.position,
                        why_pinned(c.kind)
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            EraseRefusal::Custody(e) => write!(
                f,
                "custody would not destroy this space's key material, so deleting its rows \
                 would leave every backup of them openable — which is not erasure. Nothing \
                 was deleted: {e}"
            ),
            EraseRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EraseRefusal {}

impl From<PoolError> for EraseRefusal {
    fn from(e: PoolError) -> EraseRefusal {
        EraseRefusal::Store(e)
    }
}

fn why_pinned(kind: ConsumerKind) -> &'static str {
    match kind {
        ConsumerKind::Archive => {
            "the archive is the copy of record, and history it has not received dies with \
             the space"
        }
        ConsumerKind::ClientFirstMirror => {
            "the device is the authoritative store; erasing the keys strands it rather than \
             freeing it"
        }
        ConsumerKind::Listener => {
            "at-least-once delivery holds only while the floor never passes the slowest \
             consumer"
        }
        // Unreachable: `Mirror` never reaches this list. Named rather than
        // wildcarded so a fifth kind arrives as a compile error here, where
        // somebody must decide whether it blocks a deletion.
        ConsumerKind::Mirror => "a mirror is a cache and does not pin a deletion",
    }
}

/// What deletion would do, computed without doing it.
///
/// The number a confirmation is *for*: an owner who has not been shown
/// what they are ending has not confirmed anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EraseDisclosure {
    pub space: String,
    /// Groups in the chain right now.
    pub log_groups: u64,
    /// Of those, how many are still inside the undo horizon — the window
    /// in which the platform guarantees the owner a fine-grained revert.
    /// Deleting spends that guarantee.
    pub inside_undo_horizon: u64,
    /// Sealed payloads that become unopenable.
    pub sealed_payloads: i64,
    /// Rows per entity table, in name order.
    pub rows: Vec<(String, i64)>,
    /// Non-evictable consumers behind the head. Non-empty means
    /// [`erase_space`] will refuse.
    pub pinned: Vec<Consumer>,
    /// Evictable mirrors behind the head. Reported so an operator sees
    /// what a deletion will strand, and does not block it.
    pub stranded_mirrors: Vec<Consumer>,
}

impl EraseDisclosure {
    pub fn total_rows(&self) -> i64 {
        self.rows.iter().map(|(_, n)| n).sum()
    }
}

/// What an erasure did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Erased {
    pub space: String,
    /// The disclosure the confirmation was taken against.
    pub disclosure: EraseDisclosure,
    /// Rows removed, per table, in the order the statement names them.
    pub deleted: Vec<(String, i64)>,
    /// Tables deliberately untouched. Always [`RETAINED`]; carried on the
    /// report so the op's answer states the residue rather than leaving it
    /// to a document.
    pub retained: Vec<String>,
    /// Objects removed from the space's blob store.
    ///
    /// `0` for a deployment with no blob tier wired, and the op says which
    /// of the two that was — "there are none" and "we did not look" are
    /// different facts and read identically if nobody states one.
    pub objects_deleted: u64,
    /// Objects **left** in a bucket the tenant owns, one entry per bucket
    /// (D-S5). The keys were destroyed, so they are unopenable, and the
    /// caller records each as a custody action. Empty when every object was
    /// in the deployment's own store.
    pub objects_left: Vec<crate::blobgc::ObjectsLeft>,
}

impl Erased {
    pub fn total_deleted(&self) -> i64 {
        self.deleted.iter().map(|(_, n)| n).sum()
    }
}

/// Everything deletion would destroy, and everything that would stop it.
pub async fn disclose(
    pool: &Pool,
    space: &SpaceId,
    schema: &Schema,
    policy: &LogPolicy,
    now: u64,
) -> Result<EraseDisclosure, PoolError> {
    let sid = ordo_commandlog::SpaceId(space.0.clone());
    let head = chain_len(pool, space).await?;
    let cutoff = now.saturating_sub(policy.undo_horizon.as_secs());
    let horizon = crate::prune::horizon_position(pool, &sid, cutoff).await?;

    let mut pinned = Vec::new();
    let mut stranded = Vec::new();
    for c in crate::prune::consumers(pool, &sid).await? {
        if c.position >= head {
            continue;
        }
        if c.kind.evictable() {
            stranded.push(c);
        } else {
            pinned.push(c);
        }
    }

    let mut rows = Vec::new();
    let mut names: Vec<&str> = schema.entities.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    for name in names {
        rows.push((name.to_string(), count_in(pool, name, space).await?));
    }

    Ok(EraseDisclosure {
        space: space.0.clone(),
        log_groups: head,
        // `horizon_position` is where the horizon *starts*: everything at
        // or after it is still revertable. The chain's head is one past
        // the last entry, so the count is the difference.
        inside_undo_horizon: head.saturating_sub(horizon),
        sealed_payloads: crate::sealed::space_payload_count(pool, space).await?,
        rows,
        pinned,
        stranded_mirrors: stranded,
    })
}

/// The reserved prefix a system space's id carries.
///
/// A reserved **prefix** rather than one reserved name, because a
/// deployment may come to hold more than one platform-owned space (a
/// catalogue and a package index are different artifacts with different
/// owners) and each still needs the same protection. `$` is the same
/// character the platform reserves everywhere else it needs a name a
/// caller cannot produce: `$space`, `$commandlog`, `$deployment`.
pub const SYSTEM_SPACE_PREFIX: &str = "$";

/// The id of the system space every deployment creates at boot (checklist
/// 17v). The argument for the name, and for what that space holds, is at
/// `ordo_host::provision::SYSTEM_SPACE`, which re-exports this constant.
/// It is defined here because [`is_system_space`] has to refuse it and
/// `ordo-exec` cannot see `ordo-host`.
///
/// **Why this one carries no `$`.** The prefix reserves ids a caller
/// cannot produce, which is the right shape for a platform-owned space
/// nothing addresses through the door. The space the boot creates is
/// addressed: it has a founding member row, the access engine is built
/// from that row, and `ordo_api::well_formed_space` binds only
/// alphanumerics, `_` and `-`, so a `$` id could never arrive in a request
/// header or be provisioned at all. So it is protected by its name instead
/// of by the prefix, and both answers stay inside one function.
pub const SYSTEM_SPACE: &str = "system";

/// Is this space the platform's own rather than a tenant's?
///
/// Named here rather than compared inline at the one call site, because
/// the next caller that needs the question is the one that would otherwise
/// answer it differently.
pub fn is_system_space(space: &SpaceId) -> bool {
    space.0 == SYSTEM_SPACE || space.0.starts_with(SYSTEM_SPACE_PREFIX)
}

/// Delete a space: destroy its keys, then remove its rows.
///
/// `confirm` must be the space's own id. See [`EraseRefusal::NotConfirmed`].
/// A system space is refused outright, before the confirmation. See
/// [`EraseRefusal::SystemSpace`].
#[allow(clippy::too_many_arguments)]
pub async fn erase_space(
    pool: &Pool,
    space: &SpaceId,
    schema: &Schema,
    keys: &dyn KeyProvider,
    policy: &LogPolicy,
    now: u64,
    confirm: &str,
    // The space's blob store, when the deployment has one. Taken here
    // rather than deleted by the caller afterwards, because **the ordering
    // is the argument** and this function is where the ordering lives —
    // see the block comment at the deletion site.
    blobs: Option<&dyn ordo_blob::StoreResolver>,
) -> Result<Erased, EraseRefusal> {
    // Before the confirmation, deliberately: typing the id back proves
    // intent, and this refusal is not about intent. A system space is not
    // a tenant, it holds what every tenant reads, and it has no exit.
    if is_system_space(space) {
        return Err(EraseRefusal::SystemSpace {
            space: space.0.clone(),
        });
    }
    if confirm != space.0 {
        return Err(EraseRefusal::NotConfirmed {
            expected: space.0.clone(),
            got: confirm.to_string(),
        });
    }

    let disclosure = disclose(pool, space, schema, policy, now).await?;
    if !disclosure.pinned.is_empty() {
        return Err(EraseRefusal::ConsumerPinned {
            consumers: disclosure.pinned.clone(),
            head: disclosure.log_groups,
        });
    }

    // Keys first. See the module doc on why the order is the argument.
    keys.destroy_space(space)
        .map_err(|e| EraseRefusal::Custody(e.to_string()))?;

    // **Objects before rows**, and this position is load-bearing twice.
    //
    // *After the gates*, because the two refusals above — an unconfirmed
    // deletion and a consumer still pinned to the log — must stop
    // everything. A caller who typed the wrong confirmation has not
    // consented to losing their files any more than to losing their rows,
    // and deleting objects ahead of that check would make the confirmation
    // a formality for the one part of the space that cannot be
    // reconstructed from a bundle.
    //
    // *Before `delete_rows`*, because `delete_rows` truncates every
    // space-scoped platform table, `$blobref` among them — and `$blobref`
    // **is** the list of objects to delete. Running afterwards leaves them
    // unreachable *and* undeletable, which is the exact failure
    // `crate::prune` names for the analogous ordering: *"the query that
    // would find it needs the very rows that were removed. Unreachable and
    // undeletable is worse than either alone."*
    //
    // The residual failure is the tolerable one: if this succeeds and
    // `delete_rows` fails, the space has lost its files and kept its rows —
    // visible, loud, and retryable, because both halves are idempotent.
    //
    // Crypto-erasure alone would not have covered this. The DEK is gone one
    // line above, so nothing can open those objects again — but an object's
    // existence and its size are observable without any key, so an erased
    // space would still be listable and still be billed.
    let objects = match blobs {
        Some(stores) => crate::blobgc::erase_space(pool, stores, space)
            .await
            .map_err(|e| EraseRefusal::Custody(format!("blob erasure: {e}")))?,
        None => crate::blobgc::ErasedObjects::default(),
    };

    let deleted = delete_rows(pool, space, schema).await?;
    Ok(Erased {
        objects_deleted: objects.deleted,
        objects_left: objects.left,
        space: space.0.clone(),
        disclosure,
        deleted,
        retained: RETAINED.iter().map(|s| s.to_string()).collect(),
    })
}

// ── the delete ───────────────────────────────────────────────────────────

/// Every table that holds rows for this space: the schema's entities, and
/// every platform table the catalog says has a space column.
///
/// **Discovered, not listed.** A hand-written list of platform tables is
/// the same fragility as a hand-written column list one layer up: the day
/// somebody adds a space-scoped platform table, a deletion that names
/// tables by hand starts leaving a tenant's rows behind and says it
/// succeeded. So the platform half comes from `information_schema`, keyed
/// on the two spellings the platform actually uses — `$space` on tables
/// that mirror an entity's shape, `space` on the ones written before it —
/// and the entity half comes from the schema.
async fn space_scoped_tables(
    pool: &Pool,
    schema: &Schema,
) -> Result<Vec<(String, String)>, PoolError> {
    let mut out: Vec<(String, String)> = schema
        .entities
        .iter()
        .map(|e| (e.name.clone(), ordo_schema::SPACE_COLUMN.to_string()))
        .collect();
    out.sort();

    let rows = pool
        .rows(
            "SELECT c.table_name, c.column_name \
             FROM information_schema.columns c \
             JOIN information_schema.tables t \
               ON t.table_schema = c.table_schema AND t.table_name = c.table_name \
             WHERE c.table_schema = 'public' AND t.table_type = 'BASE TABLE' \
               AND c.table_name LIKE '$%' \
               AND c.column_name IN ('$space', 'space') \
             ORDER BY c.table_name, c.column_name",
        )
        .await?;
    for r in &rows {
        let (Some(Some(table)), Some(Some(col))) = (r.first(), r.get(1)) else {
            continue;
        };
        if RETAINED.contains(&table.as_str()) {
            continue;
        }
        out.push((table.clone(), col.clone()));
    }
    Ok(out)
}

/// The platform tables that say **where** a space is stored rather than
/// holding any of it: its database attachment and its bucket attachment.
///
/// Erasure deletes their rows with everything else, since a deleted space
/// should not keep its attachments. [`rows_held`] leaves them out, because
/// it asks whether a space has data, and a space's own attachment row is not
/// data: counting it made every retry of a provision refuse itself.
/// Platform tables that **name** a space rather than hold its data:
/// where its rows live, and that it exists at all.
///
/// `$spaces` joined the list in 11-15, when the tenant registry row began
/// being written **before** anything was provisioned (`bootstrap.md`
/// §4.4). Without it, reserving the row made `rows_held` report 1 and the
/// very next step refused the create it had just started, with "this space
/// already holds rows on the default database". The row is not rows the
/// space holds; it is the record that the space is being made.
pub const ATTACHMENTS: &[&str] = &["$spacedb", "$spacebucket", "$spaces"];

/// How many rows `space` holds in `pool`'s database, across every table an
/// erasure would delete from except [`ATTACHMENTS`].
///
/// For provisioning onto a database of its own: a space that already holds
/// rows on the default cannot be attached elsewhere without leaving them
/// behind. It reads the list erasure uses, so "which tables hold a space"
/// has one answer rather than two that drift. The security stream is not in
/// it, for erasure's reason: a denial recorded about a space is a record
/// about the space, not the space's data.
pub async fn rows_held(pool: &Pool, space: &SpaceId, schema: &Schema) -> Result<i64, PoolError> {
    let tables: Vec<(String, String)> = space_scoped_tables(pool, schema)
        .await?
        .into_iter()
        .filter(|(table, _)| !ATTACHMENTS.contains(&table.as_str()))
        .collect();
    if tables.is_empty() {
        return Ok(0);
    }
    let lit = sql_lit(&space.0);
    let sum = tables
        .iter()
        .map(|(table, col)| {
            format!(
                "(SELECT count(*) FROM {} WHERE {} = {lit})",
                ordo_schema::ddl::q(table),
                ordo_schema::ddl::q(col)
            )
        })
        .collect::<Vec<_>>()
        .join(" + ");
    let rows = pool.rows(&format!("SELECT {sum}")).await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// One statement, one transaction, every table.
///
/// A data-modifying CTE per table rather than a sequence of `DELETE`s, and
/// that is load-bearing rather than clever. Every ref is a foreign key on
/// `($space, id)`, so a sequence of deletes has to run children-before-
/// parents, which needs a topological sort of the ref graph — and has no
/// correct answer at all when two entities reference each other. Inside one
/// statement, referential-integrity triggers fire once at the end, by which
/// time every row of the space is gone from every table and nothing dangles.
/// The ordering problem stops existing rather than being solved.
///
/// The space's chain-append advisory lock is held for the transaction, so a
/// concurrent write cannot append a group to a chain that is being deleted.
async fn delete_rows(
    pool: &Pool,
    space: &SpaceId,
    schema: &Schema,
) -> Result<Vec<(String, i64)>, PoolError> {
    let tables = space_scoped_tables(pool, schema).await?;
    if tables.is_empty() {
        return Ok(Vec::new());
    }
    let lit = sql_lit(&space.0);
    let ctes: Vec<String> = tables
        .iter()
        .enumerate()
        .map(|(i, (table, col))| {
            format!(
                "d{i} AS (DELETE FROM {} WHERE {} = {lit} RETURNING 1)",
                ordo_schema::ddl::q(table),
                ordo_schema::ddl::q(col)
            )
        })
        .collect();
    let counts: Vec<String> = (0..tables.len())
        .map(|i| format!("(SELECT count(*) FROM d{i})"))
        .collect();

    // `MultiStatementGroup`: SERIALIZABLE, because the one statement
    // below is a whole space's disappearance and must not interleave with a
    // write that read the space a moment earlier.
    let tx = pool.begin_for(OpClass::MultiStatementGroup).await?;
    tx.rows(&format!(
        "SELECT pg_advisory_xact_lock({APPEND_LOCK_NS}, {})",
        space_lock_key(&ordo_commandlog::SpaceId(space.0.clone()))
    ))
    .await?;
    let rows = tx
        .rows(&format!(
            "WITH {} SELECT {}",
            ctes.join(", "),
            counts.join(", ")
        ))
        .await?;
    tx.commit().await?;

    let row = rows.first().cloned().unwrap_or_default();
    Ok(tables
        .into_iter()
        .enumerate()
        .map(|(i, (table, _))| {
            let n = row
                .get(i)
                .and_then(|c| c.as_deref())
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            (table, n)
        })
        .collect())
}

async fn chain_len(pool: &Pool, space: &SpaceId) -> Result<u64, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM \"$commandlog\" WHERE \"space\" = {}",
            sql_lit(&space.0)
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

async fn count_in(pool: &Pool, table: &str, space: &SpaceId) -> Result<i64, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM {} WHERE {} = {}",
            ordo_schema::ddl::q(table),
            ordo_schema::ddl::q(ordo_schema::SPACE_COLUMN),
            sql_lit(&space.0)
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}
