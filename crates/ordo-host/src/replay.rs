//! The durable replay cache, checklist 8q.
//!
//! `MemoryReplayCache` is correct for one node and **wrong for more than
//! one**, in a way that fails silently: two replicas hold two maps, so a
//! retry that lands on the other one misses, executes, and the "exactly
//! once" promise degrades to "at least once" with nothing to observe. It
//! does not survive a restart either — the same degradation, once per
//! deploy.
//!
//! Everything structural here is already ruled in
//! [`ordo_api::idempotency`]'s module documentation, down to the statements;
//! this is that design built, and the notes below are only the places where
//! building it decided something the design left open.
//!
//! # `claim` is one statement, and the constraint is the mutual exclusion
//!
//! ```sql
//! INSERT INTO "$idempotency" (…, state, reserved_at) VALUES (…, 'pending', now())
//! ON CONFLICT ("$space", principal, op, key) DO UPDATE
//!   SET reserved_at = now(), fingerprint = EXCLUDED.fingerprint
//!   WHERE "$idempotency".state = 'pending'
//!     AND "$idempotency".reserved_at < now() - interval '…'
//! RETURNING (xmax = 0) AS inserted
//! ```
//!
//! A returned row means this caller holds the reservation — either it
//! inserted, or it took over one whose holder had gone quiet past the
//! timeout. **No row returned means somebody else holds the key**, and only
//! then does this read the row to answer `Hit` / `InFlight` / `Mismatch`.
//!
//! A `SELECT` followed by an `INSERT` would be the Z-012 race one layer
//! down: two callers both see nothing, both insert, one gets a constraint
//! violation it must then interpret. The unique index is what makes the
//! decision, in the same statement that acts on it.
//!
//! The stale takeover is a **predicate on the conflicting row** rather than
//! a second statement, for the same reason: between a `SELECT` that finds a
//! stale reservation and an `UPDATE` that takes it, another caller can take
//! it too, and both would execute.
//!
//! # Responses are sealed at rest
//!
//! A replay cache is a **plaintext mirror of every write response** — the
//! written row, post-defaults, sealed fields included, because the response
//! is what the caller saw after the executor opened them. Keeping that in
//! cleartext beside the data it was sealed away from would undo the sealing
//! for anything a caller ever wrote and retried. So the body is sealed
//! under the space's own key before it is stored, with the entity/field
//! binding `$idempotency:response` — a pseudo-entity, disjoint from every
//! real field's AAD, so a stored response cannot be swapped for a column's
//! payload or the reverse.
//!
//! An unopenable stored response is **not** an error the caller sees: it
//! means the epoch was destroyed or rotated out from under a cached entry,
//! and the honest answer is that there is nothing to replay. The entry is
//! treated as a miss and re-reserved, which costs a re-execution and never
//! a wrong answer.
//!
//! # What this does NOT do, stated rather than discovered later
//!
//! - **`put` does not commit inside the write's transaction — by ruling,
//!   not by omission** (`docs/plan-idempotency.md` §2, owner-approved
//!   2026-09-06). The trait receives no transaction and runs after the
//!   executor has returned, and it will keep doing so. What that leaves
//!   open is **not a re-execution**: the effect's own transaction carries
//!   the `$replay` journal with it
//!   ([`ordo_exec::TxConn::claim_lease`] / [`ordo_exec::TxConn::complete_lease`]),
//!   so commit publishes the effect and the record of it or neither. A
//!   retry re-claims at the door, hands its lease to the executor, and the
//!   executor's in-transaction read answers `Applied`: the attempt rolls
//!   back untouched and the router **answers instead of executing**.
//!
//!   What a crash between that commit and this `put` loses is the
//!   **verbatim response body**, and only that. The retry is told so, in
//!   the field machines read: `idempotency_applied` (409, terminal) —
//!   *the effect committed, its body went with the process that ran it,
//!   stop retrying*. Nothing re-executes; nothing lies.
//!
//!   Folding `put` in was audited and **rejected**, for two costs it
//!   cannot avoid. Sealing takes a [`KeyProvider`] release, and inside the
//!   transaction it would have to *succeed before the effect could commit*
//!   — a custody outage would then stop every write, where today it loses
//!   only a replay convenience. And the per-space append lock is
//!   transaction-scoped, so any statement placed after the chain append
//!   runs with it held: an unbounded `INSERT` there (a `whole` read's
//!   response is the object graph) is precisely the widening
//!   `implementation.md` names as the write path's failure mode. Measured,
//!   not assumed — `ordo-host/tests/lock_wait.rs` puts a number on what
//!   the section costs and on what the journal's own UPDATE would cost if
//!   it were moved into it.
//! - **`KeyBinding::Permanent` is not a column on the created row.** It is
//!   stored here like any other binding, with the entry's own lifetime, so
//!   the create-once guarantee lasts as long as the entry rather than as
//!   long as the row. Pre-existing shape, not a regression; it is what the
//!   checklist still records as owed under 8q.

use std::time::Duration;

use ordo_api::idempotency::{
    KeyBinding, ReplayCache, ReplayClaim, ReplayKey, RequestFingerprint, StoredResponse,
};
use ordo_crypto::custody::{KeyProvider, Purpose, SpaceId};
use ordo_exec::route::{PoolResolver, SinglePool};
use ordo_exec::Pool;

/// DDL for the store. A platform table, like `$commandlog` and
/// `$sealedpayload`, so it is created at boot rather than generated.
pub const IDEMPOTENCY_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS "$idempotency" (
    "$space"      text        NOT NULL,
    "principal"   text        NOT NULL,
    "op"          text        NOT NULL,
    "key"         text        NOT NULL,
    "fingerprint" text        NOT NULL,
    "state"       text        NOT NULL,
    "status"      int,
    "response"    bytea,
    "reserved_at" timestamptz NOT NULL,
    "stored_at"   timestamptz,
    PRIMARY KEY ("$space", "principal", "op", "key")
)"#;

/// A Postgres-backed [`ReplayCache`].
///
/// **Its rows live with the space they are about** (`plan-space-routing.md`
/// §3): an idempotency record holds a sealed copy of a write's response,
/// which is tenant data under the tenant's custody, so each key's space is
/// resolved to the database that space lives in and the record is kept
/// there. The table is created in a database the first time this cache uses
/// it.
pub struct PgReplayCache {
    pools: std::sync::Arc<dyn PoolResolver>,
    /// Databases this cache has created its table in, so the DDL runs once
    /// per database rather than once per request.
    ensured: std::sync::Mutex<Vec<Pool>>,
    keys: std::sync::Arc<dyn KeyProvider>,
    ttl: Duration,
    reservation_timeout: Duration,
}

impl PgReplayCache {
    /// A cache over one database, for every space.
    pub fn new(pool: Pool, keys: std::sync::Arc<dyn KeyProvider>) -> Self {
        Self::routed(std::sync::Arc::new(SinglePool::new(pool)), keys)
    }

    /// A cache that keeps each space's records in the database `pools`
    /// resolves that space to.
    pub fn routed(
        pools: std::sync::Arc<dyn PoolResolver>,
        keys: std::sync::Arc<dyn KeyProvider>,
    ) -> Self {
        Self {
            pools,
            ensured: std::sync::Mutex::new(Vec::new()),
            keys,
            ttl: Duration::from_secs(ordo_api::idempotency::DEFAULT_TTL_SECS),
            reservation_timeout: Duration::from_secs(
                ordo_api::idempotency::DEFAULT_RESERVATION_TIMEOUT_SECS,
            ),
        }
    }

    /// Shorter windows, for tests that must observe a takeover without
    /// waiting a minute for one.
    pub fn with_bounds(mut self, ttl: Duration, reservation_timeout: Duration) -> Self {
        self.ttl = ttl;
        self.reservation_timeout = reservation_timeout;
        self
    }

    /// Create the table in every database the resolver lists, so a
    /// deployment can do it at boot rather than on a request.
    ///
    /// **Only the deployment's own database can refuse the boot.** Every
    /// tenant has a database of its own, so one tenant's cluster being down
    /// or its credential missing must not take the door down for every
    /// other tenant. A tenant database that cannot be reached is reported
    /// here and ensured on its first keyed request instead, which
    /// `pool_for` already does.
    pub async fn ensure_table(&self) -> Result<(), String> {
        let targets = self.pools.targets().await.map_err(|e| e.to_string())?;
        for (n, t) in targets.into_iter().enumerate() {
            let ensured = match t.pool {
                Ok(pool) => self.ensure_on(&pool).await,
                Err(e) => Err(e),
            };
            match ensured {
                Ok(()) => {}
                // The first target is the default, the deployment's own.
                Err(e) if n == 0 => return Err(format!("{}: {e}", t.label)),
                Err(e) => eprintln!(
                    "  idempotency table not ensured on {} at boot ({e}); it is ensured \
                     on that database's first keyed request",
                    t.label
                ),
            }
        }
        Ok(())
    }

    async fn ensure_on(&self, pool: &Pool) -> Result<(), String> {
        if self
            .ensured
            .lock()
            .expect("ensured databases are not poisoned")
            .iter()
            .any(|p| p.same_handle(pool))
        {
            return Ok(());
        }
        // Retried once for `Pool::ensure_platform_tables`' reason: two
        // requests creating the table at once can collide on the catalog.
        if pool.rows(IDEMPOTENCY_DDL).await.is_err() {
            pool.rows(IDEMPOTENCY_DDL)
                .await
                .map_err(|e| e.to_string())?;
        }
        self.ensured
            .lock()
            .expect("ensured databases are not poisoned")
            .push(pool.clone());
        Ok(())
    }

    /// The database `space`'s records live in, with the table present.
    /// `None` when it cannot be reached, which every caller answers the way
    /// it answers an unreachable cache.
    async fn pool_for(&self, space: &str) -> Option<Pool> {
        let pool = self
            .pools
            .resolve(&ordo_crypto::SpaceId(space.to_string()))
            .await
            .ok()?;
        self.ensure_on(&pool).await.ok()?;
        Some(pool)
    }

    fn seal_body(&self, key: &ReplayKey, body: &serde_json::Value) -> Option<Vec<u8>> {
        let space = SpaceId(key.space.clone());
        let lease = self
            .keys
            .release(&space, Purpose::Write, &correlation(key))
            .ok()?;
        self.keys
            .seal_at(
                &lease,
                "$idempotency",
                "response",
                ordo_crypto::Anchor::Key(&anchor(key)),
                body.to_string().as_bytes(),
            )
            .ok()
    }

    fn open_body(&self, key: &ReplayKey, sealed: &[u8]) -> Option<serde_json::Value> {
        let space = SpaceId(key.space.clone());
        let lease = self
            .keys
            .release(&space, Purpose::Read, &correlation(key))
            .ok()?;
        let plain = self
            .keys
            .open_at(
                &lease,
                "$idempotency",
                "response",
                ordo_crypto::Anchor::Key(&anchor(key)),
                sealed,
            )
            .ok()?;
        serde_json::from_slice(&plain).ok()
    }
}

/// What a journalled response is sealed at: the record's own identity,
/// principal, op and key, so a response moved to another record does not
/// open. The unit separator keeps one part from reading as another. The
/// space is not part of it, because the space's key already is.
fn anchor(key: &ReplayKey) -> String {
    format!("{}\u{1f}{}\u{1f}{}", key.principal, key.op, key.key)
}

/// The lease's correlation. It names the request this key belongs to and
/// carries no payload — a correlation is an audit breadcrumb, not a place
/// to smuggle the thing being sealed.
fn correlation(key: &ReplayKey) -> String {
    format!("idempotency:{}:{}", key.op, key.key)
}

fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn hex_literal(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2 + 4);
    s.push_str("'\\x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s.push('\'');
    s
}

#[async_trait::async_trait]
impl ReplayCache for PgReplayCache {
    async fn claim(
        &self,
        key: &ReplayKey,
        fingerprint: &RequestFingerprint,
        _binding: KeyBinding,
    ) -> ReplayClaim {
        let fp = fingerprint.hex();
        let Some(pool) = self.pool_for(&key.space).await else {
            // The space's database cannot be reached, which is an unreachable
            // cache: in flight, never reserved (see below).
            return ReplayClaim::InFlight;
        };
        // One statement. The unique constraint decides who reserves, and the
        // stale-takeover test is a predicate on the conflicting row so the
        // takeover is atomic too. `RETURNING` a row at all means this caller
        // now holds the key.
        let insert = format!(
            "INSERT INTO \"$idempotency\" \
             (\"$space\", \"principal\", \"op\", \"key\", \"fingerprint\", \"state\", \"reserved_at\") \
             VALUES ({sp}, {pr}, {op}, {k}, {fp}, 'pending', now()) \
             ON CONFLICT (\"$space\", \"principal\", \"op\", \"key\") DO UPDATE \
               SET \"reserved_at\" = now(), \"fingerprint\" = EXCLUDED.\"fingerprint\", \
                   \"state\" = 'pending', \"response\" = NULL, \"status\" = NULL \
               WHERE \"$idempotency\".\"state\" = 'pending' \
                 AND \"$idempotency\".\"reserved_at\" < now() - interval '{to} seconds' \
             RETURNING 1",
            sp = lit(&key.space),
            pr = lit(&key.principal),
            op = lit(&key.op),
            k = lit(&key.key),
            fp = lit(&fp),
            to = self.reservation_timeout.as_secs(),
        );
        match pool.rows(&insert).await {
            Ok(rows) if !rows.is_empty() => return ReplayClaim::Reserved,
            Ok(_) => {}
            // A cache that cannot be reached must not silently become no
            // cache at all: reporting the key as in flight makes the caller
            // retry, where falling through to `Reserved` would execute a
            // request whose idempotency nothing is enforcing.
            Err(_) => return ReplayClaim::InFlight,
        }

        // Somebody else holds it. Only now is a read correct, and it cannot
        // race: the row exists and its state only moves forward.
        let select = format!(
            "SELECT \"state\", \"fingerprint\", \"status\", \"response\", \
                    (\"stored_at\" < now() - interval '{ttl} seconds') AS expired \
             FROM \"$idempotency\" \
             WHERE \"$space\" = {sp} AND \"principal\" = {pr} \
               AND \"op\" = {op} AND \"key\" = {k}",
            ttl = self.ttl.as_secs(),
            sp = lit(&key.space),
            pr = lit(&key.principal),
            op = lit(&key.op),
            k = lit(&key.key),
        );
        let Ok(rows) = pool.rows(&select).await else {
            return ReplayClaim::InFlight;
        };
        let Some(row) = rows.first() else {
            // It vanished between the two statements — an `abandon` landing
            // in the gap. Nothing holds the key, and reporting in-flight
            // costs one retry rather than an unenforced execution.
            return ReplayClaim::InFlight;
        };
        let cell = |i: usize| row.get(i).cloned().flatten();
        let state = cell(0).unwrap_or_default();
        let stored_fp = cell(1).unwrap_or_default();

        // The payload check comes before the state check: same key with a
        // different payload is a mismatch whether the first attempt has
        // finished or not, and answering `InFlight` there would tell a
        // caller to retry a request that can never succeed under that key.
        if stored_fp != fp {
            return ReplayClaim::Mismatch;
        }
        if state != "done" {
            return ReplayClaim::InFlight;
        }
        if cell(4).as_deref() == Some("t") {
            // Expired response, live row. The entry is no longer an
            // authority, so this is a miss — but the row is here, so the
            // reservation has to be retaken rather than inserted.
            return ReplayClaim::InFlight;
        }
        let (Some(status), Some(sealed)) = (cell(2), cell(3)) else {
            return ReplayClaim::InFlight;
        };
        let Some(bytes) = decode_bytea(&sealed) else {
            return ReplayClaim::InFlight;
        };
        match self.open_body(key, &bytes) {
            // Unopenable means the epoch is gone, which is not the caller's
            // problem and not a lie worth telling: there is nothing to
            // replay, so this behaves as a miss and costs a re-execution.
            None => ReplayClaim::InFlight,
            Some(body) => ReplayClaim::Hit(StoredResponse {
                status: status.parse().unwrap_or(200),
                body,
            }),
        }
    }

    async fn put(
        &self,
        key: ReplayKey,
        fingerprint: RequestFingerprint,
        response: StoredResponse,
        _binding: KeyBinding,
    ) {
        let Some(sealed) = self.seal_body(&key, &response.body) else {
            // Nothing storable. Leaving a `pending` marker behind would park
            // the key until the timeout, so release it: the retry re-runs,
            // which is the same answer the in-memory cache gives when an
            // entry is evicted.
            self.abandon(&key).await;
            return;
        };
        let sql = format!(
            "UPDATE \"$idempotency\" SET \"state\" = 'done', \"status\" = {st}, \
                    \"response\" = {resp}, \"stored_at\" = now(), \"fingerprint\" = {fp} \
             WHERE \"$space\" = {sp} AND \"principal\" = {pr} \
               AND \"op\" = {op} AND \"key\" = {k}",
            st = response.status,
            resp = hex_literal(&sealed),
            fp = lit(&fingerprint.hex()),
            sp = lit(&key.space),
            pr = lit(&key.principal),
            op = lit(&key.op),
            k = lit(&key.key),
        );
        if let Some(pool) = self.pool_for(&key.space).await {
            let _ = pool.rows(&sql).await;
        }
    }

    async fn abandon(&self, key: &ReplayKey) {
        let sql = format!(
            "DELETE FROM \"$idempotency\" WHERE \"$space\" = {sp} AND \"principal\" = {pr} \
               AND \"op\" = {op} AND \"key\" = {k} AND \"state\" = 'pending'",
            sp = lit(&key.space),
            pr = lit(&key.principal),
            op = lit(&key.op),
            k = lit(&key.key),
        );
        if let Some(pool) = self.pool_for(&key.space).await {
            let _ = pool.rows(sql.as_str()).await;
        }
    }
}

/// Postgres renders `bytea` as `\x…` in text mode, which is how this pool
/// reads every column.
fn decode_bytea(s: &str) -> Option<Vec<u8>> {
    let hex = s.strip_prefix("\\x")?;
    (hex.len() % 2 == 0)
        .then(|| {
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                .collect::<Option<Vec<u8>>>()
        })
        .flatten()
}
