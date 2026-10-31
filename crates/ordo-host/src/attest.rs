//! Layer-1 client pinning over the command log, checklist 8q.
//!
//! [`ordo_api::attest`] states the contract and why it is worth having; this
//! is the implementation over a real chain, and the notes here are the two
//! things building it decided.
//!
//! # Signing when the head moves, not when a caller asks
//!
//! bootstrap.md's word is *"~free"*, and that is a constraint rather than a
//! boast: a layer everyone disables is not a layer. An Ed25519 signature per
//! request would not be free, and it would not be more truthful either — the
//! head only changes when a group is appended, so a signature computed for
//! the ten-thousandth reader covers the same bytes as the one computed for
//! the first.
//!
//! So the signature is cached per space and recomputed only when the
//! position moves. What remains per request is **one round trip** reading
//! both ends of the chain — the last `(idx, hash)` and the first `idx` —
//! stated plainly rather than described as free: two primary-key endpoint
//! scans limited to one row each, in one statement, on the same connection
//! pool the request is already using.
//!
//! Avoiding even that would mean the write path invalidating this cache,
//! which couples the log's append to the door's response headers for a
//! saving that is smaller than the query it replaces.
//!
//! # Why the *first* index is read too, and why it is in the cache key
//!
//! `verifiable_from` is the chain's first retained index — where a pruned
//! deployment can still be verified by recomputation. It is read in the same
//! statement as the head because a second round trip for one integer would
//! double the per-request cost of a layer whose whole claim is that it is
//! ~free.
//!
//! It is part of what the cache is keyed on, and that is the subtle half. A
//! prune moves the first index and **does not move the head**, so a cache
//! that only noticed the head would keep serving a pre-prune
//! `verifiable_from` on a chain that no longer has those entries — the one
//! wrong answer this field exists to prevent. The signature is still
//! recomputed only when the head moves; the cached *entry* is invalidated
//! when either end does.
//!
//! # Reading `upto` from the index, not from a count
//!
//! `upto` is `idx + 1` of the last row, which is what the log's own
//! `Checkpoint` means by it — **not** `count(*)`. On a chain that has never
//! had a row removed the two agree, and relying on that agreement would make
//! a pin silently wrong on one that has: a count answers "how many rows are
//! here", and a client pinning position needs "how far along is this chain",
//! which is what the monotonic `(space, idx)` key already promises.
//!
//! # Failure is unattested, never an outage
//!
//! Every error path answers `None`. Pinning is an auditing convenience laid
//! over the request path, and a client that requires a pin sees its absence,
//! which is the signal it needs anyway — where failing the read would turn a
//! log hiccup into an outage for callers who never asked to be witnesses.

use std::collections::HashMap;
use std::sync::Mutex;

use ordo_api::attest::{AttestationSource, HeadPin};
use ordo_commandlog::store::Signer;
use ordo_commandlog::{ChainHash, Checkpoint, SpaceId};
use ordo_exec::Pool;

/// What was signed last, per space, and the retention floor it was served
/// beside. See the module doc on why the floor is part of the key.
struct Cached {
    upto: u64,
    head: String,
    verifiable_from: u64,
    key_id: String,
    signature: String,
}

/// Both ends of a space's chain: `(upto, head, verifiable_from)`.
type Tip = (u64, String, u64);

pub struct PgAttestation {
    pool: Pool,
    signer: Box<dyn Signer + Send + Sync>,
    cache: Mutex<HashMap<String, Cached>>,
}

impl PgAttestation {
    pub fn new(pool: Pool, signer: Box<dyn Signer + Send + Sync>) -> Self {
        Self {
            pool,
            signer,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Both ends of a space's chain in one round trip: the last
    /// `(idx, hash)`, and the first `idx` a prune has left behind.
    ///
    /// An **empty chain pins genesis** (`upto = 0`, the all-zero head) rather
    /// than answering "no pin". bootstrap.md's word is *always on*, and a
    /// guarantee that switches on after the first write is not that: a client
    /// whose very first request is a read would hold nothing, and could not
    /// later tell "this space had no history when I looked" from "this
    /// deployment does not attest". A signed *"this chain is empty"* is a
    /// real claim, and the first one worth catching a deployment retracting.
    ///
    /// An empty chain's `verifiable_from` is `0`, and that is the honest
    /// answer only because `ordo_exec::prune` never removes the chain's last
    /// entry: an empty chain is one that was never written, so genesis
    /// really is where verification would start.
    ///
    /// A query FAILURE is still `None`, and the difference is the point: an
    /// empty result is knowledge, an error is the absence of it, and
    /// answering genesis on an error would be signing a claim nothing checked.
    async fn tip(&self, space: &str) -> Option<Tip> {
        // One statement, two endpoint scans of the `(space, idx)` primary
        // key. A correlated subquery rather than `min()/max()` over the same
        // scan, so each side is an index endpoint read rather than an
        // aggregate over the space's rows.
        let lit = space.replace('\'', "''");
        // Both sides are aliased and the subquery's output column is named:
        // without that, the outer `ORDER BY "idx"` is ambiguous between the
        // table's column and the projected one, and Postgres refuses the
        // statement — which this module would have served as an unattested
        // response rather than an error.
        let sql = format!(
            "SELECT c.\"idx\", c.\"hash\", \
               (SELECT b.\"idx\" FROM \"$commandlog\" b WHERE b.\"space\" = '{lit}' \
                ORDER BY b.\"idx\" LIMIT 1) AS \"first_idx\" \
             FROM \"$commandlog\" c WHERE c.\"space\" = '{lit}' ORDER BY c.\"idx\" DESC LIMIT 1"
        );
        let rows = self.pool.rows(&sql).await.ok()?;
        let Some(row) = rows.first() else {
            return Some((0, hex(&ChainHash::GENESIS.0), 0));
        };
        let idx: u64 = row.first()?.as_deref()?.parse().ok()?;
        let hash = row.get(1)?.clone()?;
        // The subquery cannot be NULL when the outer row exists — the same
        // predicate found this row — but an answer that will not parse must
        // not silently become `0`, which is the value that means "nothing
        // was pruned". `None` here is an unattested response, which is this
        // module's standing answer to *"I could not check"*.
        let verifiable_from: u64 = row.get(2)?.as_deref()?.parse().ok()?;
        Some((
            idx + 1,
            hash.trim_start_matches("\\x").to_string(),
            verifiable_from,
        ))
    }
}

#[async_trait::async_trait]
impl AttestationSource for PgAttestation {
    async fn head(&self, space: &str) -> Option<HeadPin> {
        let (upto, head, verifiable_from) = self.tip(space).await?;

        if let Ok(cache) = self.cache.lock() {
            if let Some(c) = cache.get(space) {
                // The floor is compared as well as the head: a prune moves
                // the floor and leaves the head where it was, and a hit on
                // the head alone would serve the pre-prune floor.
                if c.upto == upto && c.head == head && c.verifiable_from == verifiable_from {
                    return Some(HeadPin {
                        upto,
                        head,
                        key_id: c.key_id.clone(),
                        signature: c.signature.clone(),
                        verifiable_from,
                    });
                }
            }
        }

        // The head moved (or this is the first request for the space), so
        // this is the one request in each position that pays for a
        // signature.
        //
        // The bytes are the log's own `Checkpoint::signed_bytes`, not a
        // shape invented here: a pin a client verifies must be checkable
        // with the same routine that checks a stored checkpoint, or the
        // deployment would be publishing two attestations of one head that
        // could disagree.
        let bytes = Checkpoint::signed_bytes(
            &SpaceId(space.to_string()),
            upto,
            parse_hash(&head)?,
            // `at` is not part of what a position pin asserts — the claim is
            // "this head is at this position", which is timeless. Zero keeps
            // the bytes a function of the chain alone, so two replicas
            // signing the same head produce the same signature.
            ordo_commandlog::Timestamp(0),
        );
        let signature = hex(&self.signer.sign(&bytes));
        let key_id = self.signer.key_id().to_string();

        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                space.to_string(),
                Cached {
                    upto,
                    head: head.clone(),
                    verifiable_from,
                    key_id: key_id.clone(),
                    signature: signature.clone(),
                },
            );
        }
        Some(HeadPin {
            upto,
            head,
            key_id,
            signature,
            verifiable_from,
        })
    }

    /// The head at the position a read's rows were read at (C1).
    ///
    /// The common case costs nothing extra: nothing was written between the
    /// read and this pin, the current head is at that position, and the
    /// cached signature is the answer. Otherwise a write landed in between,
    /// and the head the rows were read at is read back from the chain and
    /// signed, uncached, because no other request will ask for that head.
    async fn head_at(&self, space: &str, upto: u64) -> Option<HeadPin> {
        let current = self.head(space).await?;
        if current.upto == upto {
            return Some(current);
        }
        if upto > current.upto {
            return None;
        }
        self.signed_at(space, upto, current.verifiable_from).await
    }
}

impl PgAttestation {
    /// The signed head at `upto`, read back from the chain rather than taken
    /// from the tip. `None` when that position has been pruned away, since a
    /// head nobody can recompute is not one to sign.
    async fn signed_at(&self, space: &str, upto: u64, verifiable_from: u64) -> Option<HeadPin> {
        if upto < verifiable_from {
            return None;
        }
        let head = if upto == 0 {
            hex(&ChainHash::GENESIS.0)
        } else {
            let lit = space.replace('\'', "''");
            let rows = self
                .pool
                .rows(&format!(
                    "SELECT \"hash\" FROM \"$commandlog\" WHERE \"space\" = '{lit}' AND \"idx\" = {}",
                    upto - 1
                ))
                .await
                .ok()?;
            rows.first()?
                .first()?
                .clone()?
                .trim_start_matches("\\x")
                .to_string()
        };
        let bytes = Checkpoint::signed_bytes(
            &SpaceId(space.to_string()),
            upto,
            parse_hash(&head)?,
            ordo_commandlog::Timestamp(0),
        );
        Some(HeadPin {
            upto,
            head,
            key_id: self.signer.key_id().to_string(),
            signature: hex(&self.signer.sign(&bytes)),
            verifiable_from,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hash(hex: &str) -> Option<ChainHash> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(ChainHash(out))
}
