//! **A tenant is an id, and its name is an attribute** (owner-ruled
//! 2026-09-21): the minting of the id, and the one table that holds the
//! name beside it.
//!
//! # Why an id at all, when every space in the tree is a string today
//!
//! Every space id that exists now arrived from outside the binary:
//! `ORDO_SPACE`, a request header, an operator's `ordo keys init --space`.
//! That is workable for a deployment whose tenants an operator types once,
//! and it is not workable for a deployment that mints them, for the reason
//! `bootstrap.md` §2 item 3 gives: *"A caller-supplied id would let a
//! caller name a space that already exists elsewhere in the deployment and
//! probe for it. Ids are server-minted, always."*
//!
//! So the id is minted here, from 128 bits of randomness, and the name the
//! caller chose is written beside it as an attribute that nothing keys on.
//! The owner's words: *"ID is the uniqueness but there can be a `name`
//! associated for human reference."*
//!
//! # What the id looks like, and every constraint it had to satisfy
//!
//! `spc_` followed by 26 characters of Crockford base32. Four properties,
//! each of which ruled something out:
//!
//! 1. **[`ordo_api::well_formed_space`] binds it unchanged.** The door
//!    accepts alphanumerics, `_` and `-`, so no new validation rule exists
//!    and no id can be minted that the door would refuse. This is what
//!    rules out a UUID's hyphenated hex with its `-` in fixed positions
//!    (allowed, but it reads as structure that means nothing here) and any
//!    base64 shape, whose `+` and `/` the door refuses outright.
//! 2. **It carries no `$`.** That prefix is reserved for platform-owned
//!    spaces a caller cannot name (`ordo_exec::erase::SYSTEM_SPACE_PREFIX`),
//!    so a minted tenant can never collide with one and can never be
//!    mistaken for one.
//! 3. **128 bits, because "globally unique" was the ruling's word.** Not
//!    unique within this deployment, which a sequence would give: a tenant
//!    that moves between deployments, or a deployment that is restored
//!    beside the one it was cloned from, must not find its id taken. That
//!    is the same bar a UUIDv4 sets and it is set the same way.
//! 4. **Crockford base32 rather than hex**, so the alphabet has no `i`,
//!    `l`, `o` or `u`: an id gets read aloud in a support call and typed
//!    back from a screenshot, which is exactly the transcription that
//!    confuses `1`/`l` and `0`/`O`. Hex would need 32 characters for the
//!    same entropy; this needs 26.
//!
//! # What this module is not
//!
//! It is not provisioning. [`crate::provision::provision_space`] mints
//! custody and writes the founding owner, and it takes a space id as a
//! string exactly as before, because a space created by an operator naming
//! it and a space created by the door minting one must not run down two
//! different paths. This module supplies the id and remembers the name;
//! that one does the work.

use ordo_exec::{sql_lit, Pool, PoolError};

/// The registry of minted tenants, in the **control** database.
///
/// Deliberately not in the tenant's own database, though almost everything
/// else about a tenant is moving there (18d): a row that says *this id
/// exists and is called that* has to be readable when the tenant's own
/// database is unreachable, or an operator looking at a quarantined tenant
/// is looking at an opaque id with no way to find out whose it is. It is
/// the same argument `$spacedb` and `$spacebucket` already make for
/// staying central, one tier in.
const SPACES_TABLE: &str = "$spaces";

const SPACES_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$spaces\" ( \
  \"$space\" text NOT NULL PRIMARY KEY, \
  \"name\" text NOT NULL, \
  \"created_at\" bigint NOT NULL, \
  \"created_by\" text NOT NULL, \
  \"state\" text NOT NULL DEFAULT 'provisioning', \
  \"phase\" text NOT NULL DEFAULT 'reserved', \
  \"target\" text NOT NULL DEFAULT '', \
  \"state_at\" bigint NOT NULL DEFAULT 0, \
  \"note\" text NOT NULL DEFAULT '', \
  \"own_of\" text NULL, \
  \"paused_at\" bigint NOT NULL DEFAULT 0, \
  \"paused_by\" text NOT NULL DEFAULT '', \
  \"paused_note\" text NOT NULL DEFAULT '', \
  \"lease_holder\" text NOT NULL DEFAULT '', \
  \"lease_until\" bigint NOT NULL DEFAULT 0, \
  \"attempts\" bigint NOT NULL DEFAULT 0, \
  \"retry_at\" bigint NOT NULL DEFAULT 0, \
  \"stuck_at\" bigint NOT NULL DEFAULT 0 \
)";

/// **The latch behind "one person, one tenant of their own"**
/// (`plan-provisioning-on-signup.md` §2.2).
///
/// Unique over `own_of` while the row is `provisioning`, `active` or
/// `paused`, so two
/// racing provisions for one principal cannot both reserve, a crash between
/// the registry row and activation leaves a row the next attempt finds, and
/// a `failed` or `reaped` row is outside the index so the next attempt may
/// provision again. Rows with a null `own_of` (every tenant `std.space.create`
/// makes) are never in it.
///
/// **A paused own tenant still holds the latch** (27ar). A person whose own
/// tenant is paused is answered with it, never given a second one. The
/// index before 27ar covered `provisioning` and `active` only and was named
/// `$spaces_own_of`; [`ensure_table`] drops that one, because an index's
/// predicate cannot be changed in place.
const OWN_OF_INDEX_DDL: &str = "\
CREATE UNIQUE INDEX IF NOT EXISTS \"$spaces_own_of_held\" ON \"$spaces\" (\"own_of\") \
WHERE \"state\" IN ('provisioning', 'active', 'paused')";

/// The states the own-of latch covers. Kept beside the index so the two
/// cannot drift.
const HELD_STATES: &str = "('provisioning', 'active', 'paused')";

/// One minted tenant, as the registry holds it.
///
/// `name` is not unique and is not indexed, and both are deliberate. A
/// deployment that refused a second tenant called "Acme" would be a
/// deployment where a name carries identity through the back door, which
/// is the thing the id ruling removed. Two tenants may share a name, and
/// telling them apart is what the id is for.
/// Where a tenant is in its own creation (`bootstrap.md` §4.1).
///
/// ```text
/// provisioning ──► active ◄──► paused
///      │
///      └──► failed ──► reaped   (terminal)
/// ```
///
/// **The point of the column is that every orphan is reachable from one
/// table.** §4.4: *"Nothing external is provisioned before the registry row
/// exists, and nothing external is used before its handle is recorded on
/// that row."* Until 11-15 this registry was written **last**, after the
/// custody keys and the database, so a crash in between left both with
/// nothing naming them. The row now comes first and the resources are
/// recorded on it as they are made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Reserved, and being built. **Not callable**, and in practice not
    /// reachable either: the door builds memberships from `members`, and a
    /// space in this state has no founding row yet.
    Provisioning,
    /// Built. The only state a tenant serves every op in.
    Active,
    /// Built, and held still on purpose (27ar). Its data is kept, its ops
    /// are refused `space_paused` except the ones the app marks `nopause`
    /// and the platform ops `crate::pause` lists. Reached only from
    /// `active`, and left only by a resume (back to `active`) or a delete.
    /// The reconciler never times it out.
    Paused,
    /// Creation stopped part way, or the reconciler timed it out. Carries
    /// the phase it reached, which is what tells `reap` how much to undo.
    Failed,
    /// Every resource released. Terminal.
    Reaped,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Provisioning => "provisioning",
            State::Active => "active",
            State::Paused => "paused",
            State::Failed => "failed",
            State::Reaped => "reaped",
        }
    }

    pub fn parse(s: &str) -> Option<State> {
        match s {
            "provisioning" => Some(State::Provisioning),
            "active" => Some(State::Active),
            "paused" => Some(State::Paused),
            "failed" => Some(State::Failed),
            "reaped" => Some(State::Reaped),
            _ => None,
        }
    }
}

/// How far creation got, which is what `reap` undoes in reverse.
///
/// Ordered, and the order is the compensation order read backwards. A phase
/// names work that **has been done**, so `Custody` means the keys exist and
/// `Attached` means the `$spacedb` row is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// The registry row exists and nothing external has been made.
    Reserved,
    /// The target was chosen and written to the row, before anything was
    /// dialled. This is the phase that makes a leaked database findable.
    Targeted,
    /// Custody keys minted.
    Custody,
    /// The tenant's database created.
    Database,
    /// `$spacedb` written, so routing resolves.
    Attached,
    /// The founding member row is in the tenant's database.
    Founded,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Reserved => "reserved",
            Phase::Targeted => "targeted",
            Phase::Custody => "custody",
            Phase::Database => "database",
            Phase::Attached => "attached",
            Phase::Founded => "founded",
        }
    }

    pub fn parse(s: &str) -> Option<Phase> {
        match s {
            "reserved" => Some(Phase::Reserved),
            "targeted" => Some(Phase::Targeted),
            "custody" => Some(Phase::Custody),
            "database" => Some(Phase::Database),
            "attached" => Some(Phase::Attached),
            "founded" => Some(Phase::Founded),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenant {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    /// §4.1's state machine.
    pub state: State,
    /// How far creation got. Meaningful for `Provisioning` and `Failed`;
    /// `Founded` for anything `Active`.
    pub phase: Phase,
    /// The database label this tenant's rows were put on, recorded
    /// **before** the database was made. `""` before a target is chosen.
    pub target: String,
    /// When the state last changed, in seconds. What the reconciler
    /// compares against `bootstrapTimeout`.
    pub state_at: u64,
    /// Why it failed, for an operator. Never shown to a caller.
    pub note: String,
    /// The principal that asked for it. Attribution, not authority: what
    /// this principal may *do* in the space is the founding owner row, and
    /// the two are separate on purpose, because a deployment owner who
    /// mints a tenant for somebody else is the ordinary case.
    pub created_by: String,
    /// The principal this tenant is the own tenant of, when it was made by
    /// built-in provisioning. `None` for a tenant `std.space.create` made.
    pub own_of: Option<String>,
    /// When it was paused, in seconds. `0` when it is not paused.
    pub paused_at: u64,
    /// Which layer paused it: [`PAUSED_BY_TENANT`] or
    /// [`PAUSED_BY_DEPLOYMENT`]. Never a name. `""` when not paused.
    pub paused_by: String,
    /// Why, from the closed list [`PAUSE_NOTES`]. `""` when not paused.
    pub paused_note: String,
    /// Until when, in seconds, one server holds the right to set this
    /// tenant up ([`claim_setup`]). `0` when nobody does.
    pub lease_until: u64,
    /// How many tries at setting it up failed.
    pub attempts: u64,
    /// When the next try may start, in seconds. `0`: at once.
    pub retry_at: u64,
    /// When it was first reported stuck, in seconds. `0`: not reported.
    pub stuck_at: u64,
}

/// A pause made from inside the tenant, by a role the app developer granted
/// `std.space.pause`.
pub const PAUSED_BY_TENANT: &str = "tenant";
/// A pause made by the deployment owner, `std.deployment.pauseTenant`.
pub const PAUSED_BY_DEPLOYMENT: &str = "deployment";

/// **Why a tenant is paused, as a closed list** (`plan-tenant-pause.md`
/// §3.3). Never free text, so nothing personal enters a stream. A notice for
/// the tenant's people is the app's to build (owner, 2026-10-05, ask 6).
pub const PAUSE_NOTES: [&str; 6] = ["owner", "billing", "abuse", "legal", "maintenance", "other"];

/// Mint a globally unique space id.
///
/// Randomness rather than a counter, and from the same source the
/// deployment already trusts for its attestation secret
/// ([`ordo_crypto::fill_random`], which is `getrandom` underneath). A
/// counter would need a sequence, a sequence would need a home, and a home
/// would make the id unique *to the database that issued it*, which is one
/// restore-from-backup away from two tenants sharing an id.
pub fn mint_space_id() -> String {
    let mut bytes = [0u8; 16];
    ordo_crypto::fill_random(&mut bytes);
    let mut out = String::with_capacity(30);
    out.push_str("spc_");
    out.push_str(&base32(&bytes));
    out
}

/// Crockford base32, lowercase, no padding. 16 bytes in, 26 characters
/// out, the last of which carries the 128th bit and three zeroes.
fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((acc << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    // Retried once for `Pool::ensure_platform_tables`' reason, which
    // `spacedb::ensure_table` states: `CREATE TABLE IF NOT EXISTS` is not
    // race-free between two booting replicas.
    if pool.rows(SPACES_DDL).await.is_err() {
        pool.rows(SPACES_DDL).await?;
    }
    // **The lifecycle columns, added to a table that predates them.**
    //
    // `CREATE TABLE IF NOT EXISTS` does nothing to a table that is already
    // there, so a `$spaces` created before 11-15 keeps its four columns and
    // every read of `state` fails with `column "state" does not exist`.
    //
    // No deployment exists to migrate (ruled 2026-09-25), so this is not
    // owed. It is here because **databases that predate the change do
    // exist** — every developer's conformance cluster holds a few dozen —
    // and the alternative is each of them meeting a confusing error and
    // being told to destroy their cluster.
    //
    // `IF NOT EXISTS` on each, so this is idempotent and costs one
    // statement per boot against a table that already has them.
    for column in [
        "\"state\" text NOT NULL DEFAULT 'provisioning'",
        "\"phase\" text NOT NULL DEFAULT 'reserved'",
        "\"target\" text NOT NULL DEFAULT ''",
        "\"state_at\" bigint NOT NULL DEFAULT 0",
        "\"note\" text NOT NULL DEFAULT ''",
        "\"own_of\" text NULL",
        "\"paused_at\" bigint NOT NULL DEFAULT 0",
        "\"paused_by\" text NOT NULL DEFAULT ''",
        "\"paused_note\" text NOT NULL DEFAULT ''",
        // Setting up a person's own tenant (owner, 2026-10-21): who is
        // making it, how often it was tried, when next, and when it was
        // first reported stuck.
        "\"lease_holder\" text NOT NULL DEFAULT ''",
        "\"lease_until\" bigint NOT NULL DEFAULT 0",
        "\"attempts\" bigint NOT NULL DEFAULT 0",
        "\"retry_at\" bigint NOT NULL DEFAULT 0",
        "\"stuck_at\" bigint NOT NULL DEFAULT 0",
    ] {
        pool.rows(&format!(
            "ALTER TABLE \"{SPACES_TABLE}\" ADD COLUMN IF NOT EXISTS {column}"
        ))
        .await?;
    }
    // A row that predates the lifecycle is a tenant that was created and
    // served, so it is `active` rather than `provisioning` — which is what
    // the column default would otherwise make it, quietly taking every
    // existing tenant out of service.
    //
    // **`state_at = 0` is what tells the two apart**, and it works only
    // because [`reserve`] writes `state_at` explicitly. It did not at
    // first, so a genuinely provisioning tenant also had zero there and
    // this would have activated it on the next boot — a half-built tenant
    // declared ready, which is the exact failure the state machine exists
    // to prevent, introduced by the code meant to smooth the upgrade.
    pool.rows(&format!(
        "UPDATE \"{SPACES_TABLE}\" SET \"state\" = 'active', \"phase\" = 'founded' \
         WHERE \"state\" = 'provisioning' AND \"state_at\" = 0"
    ))
    .await?;
    // The latch, after the column it is over. Retried once for the table's
    // own reason above: `IF NOT EXISTS` is not race-free between replicas.
    // The pre-27ar latch, whose predicate left a paused tenant out, goes
    // first; `IF EXISTS` makes that free on every later boot.
    pool.rows("DROP INDEX IF EXISTS \"$spaces_own_of\"").await?;
    if pool.rows(OWN_OF_INDEX_DDL).await.is_err() {
        pool.rows(OWN_OF_INDEX_DDL).await?;
    }
    Ok(())
}

/// Record a minted tenant. Written **after** provisioning succeeded, never
/// before.
///
/// The order matters and is the opposite of `bootstrap.md` §1's phase 1,
/// which reserves a registry row first so nothing later is orphanable.
/// That design reserves a caller-supplied `slug` and needs the row to hold
/// the uniqueness lock; there is no slug here and no lock to take, so the
/// row buys nothing at the front and costs something: a registry row for a
/// **Reserve a tenant's row before anything external is made.**
///
/// Renamed from `record` in 11-15, and the rename is the change: this used
/// to run **after** provisioning, on the reasoning that a name attached to
/// a tenant that does not exist is worse than a tenant with no name. That
/// had it backwards. `bootstrap.md` §4.4 settles it: *"Nothing external is
/// provisioned before the registry row exists, and nothing external is
/// used before its handle is recorded on that row."* A row naming a
/// resource that was never made is a row an operator can check and clear.
/// A custody key and a database that nothing names cannot be found at all.
///
/// The row lands in `provisioning`, which is not servable, and
/// [`activate`] is what makes it so.
///
/// `ON CONFLICT DO NOTHING` so a retry is safe, on the same terms as the
/// founding member row. A second call for one id keeps the first name,
/// because the alternative is a renaming path that nobody asked for
/// arriving disguised as a retry.
///
/// **`own_of`** names the principal this is the own tenant of
/// (`plan-provisioning-on-signup.md` §2.2), or `None` for a tenant made on
/// somebody's say-so. The answer is whether **this call wrote the row**.
/// `false` for an own tenant means the latch held: the principal already has
/// one, `provisioning` or `active`, and [`own_of`] reads it. The conflict
/// clause names no target, so it covers the id and the latch alike.
pub async fn reserve(
    pool: &Pool,
    id: &str,
    name: &str,
    created_by: &str,
    at: u64,
    own_of: Option<&str>,
) -> Result<bool, PoolError> {
    let own = own_of.map_or_else(|| "NULL".to_string(), sql_lit);
    let rows = pool
        .rows(&format!(
            "INSERT INTO \"{SPACES_TABLE}\" \
             (\"$space\", \"name\", \"created_at\", \"created_by\", \"state\", \"phase\", \
              \"state_at\", \"own_of\") \
             VALUES ({}, {}, {at}, {}, 'provisioning', 'reserved', {at}, {own}) \
             ON CONFLICT DO NOTHING RETURNING \"$space\"",
            sql_lit(id),
            sql_lit(name),
            sql_lit(created_by),
        ))
        .await?;
    Ok(!rows.is_empty())
}

/// The tenant that is this principal's own, if they hold one that is
/// `provisioning`, `active` or `paused`: exactly the rows the latch covers. A
/// `failed` or `reaped` one is not returned, because it does not block a
/// new one.
pub async fn own_of(pool: &Pool, principal: &str) -> Result<Option<Tenant>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT {COLUMNS} FROM \"{SPACES_TABLE}\" \
             WHERE \"own_of\" = {} AND \"state\" IN {HELD_STATES}",
            sql_lit(principal)
        ))
        .await?;
    Ok(rows.first().map(|r| parse_row(r)))
}

/// The columns [`parse_row`] reads, in its order. The own-of column is last
/// (every reader indexes by position).
const COLUMNS: &str = "\"$space\", \"name\", \"created_at\", \"created_by\", \"state\", \
                       \"phase\", \"target\", \"state_at\", \"note\", \"own_of\", \
                       \"paused_at\", \"paused_by\", \"paused_note\", \"lease_until\", \
                       \"attempts\", \"retry_at\", \"stuck_at\"";

/// Move a tenant to a phase, and record what it is now standing on.
///
/// Called **before** the resource that phase names is created, never
/// after: §4.4's rule is that nothing external is used before its handle
/// is on the row, so a crash between the write and the creation leaves a
/// row claiming a resource that does not exist. That is the safe
/// direction. The other one leaves a resource nothing names.
pub async fn at_phase(
    pool: &Pool,
    id: &str,
    phase: Phase,
    target: &str,
    at: u64,
) -> Result<(), PoolError> {
    pool.rows(&format!(
        "UPDATE \"{SPACES_TABLE}\" SET \"phase\" = {}, \"target\" = {}, \
         \"state_at\" = {at} WHERE \"$space\" = {}",
        sql_lit(phase.as_str()),
        sql_lit(target),
        sql_lit(id),
    ))
    .await
    .map(|_| ())
}

/// The tenant is built and may serve.
///
/// The last write of a create, and the only one that makes a space
/// callable. Guarded on `provisioning` so a reaped or failed tenant cannot
/// be activated by a late retry arriving after the reconciler gave up on
/// it.
pub async fn activate(pool: &Pool, id: &str, at: u64) -> Result<bool, PoolError> {
    let rows = pool
        .rows(&format!(
            "UPDATE \"{SPACES_TABLE}\" SET \"state\" = 'active', \"phase\" = 'founded', \
             \"state_at\" = {at} WHERE \"$space\" = {} AND \"state\" = 'provisioning' \
             RETURNING \"$space\"",
            sql_lit(id)
        ))
        .await?;
    Ok(!rows.is_empty())
}

/// Creation stopped. Records why, for an operator, and leaves the phase
/// alone because the phase is what `reap` reads to know how much to undo.
///
/// Guarded on `provisioning` for `activate`'s reason, and so that a second
/// failure does not overwrite the first explanation.
pub async fn fail(pool: &Pool, id: &str, note: &str, at: u64) -> Result<(), PoolError> {
    pool.rows(&format!(
        "UPDATE \"{SPACES_TABLE}\" SET \"state\" = 'failed', \"note\" = {}, \
         \"state_at\" = {at} WHERE \"$space\" = {} AND \"state\" = 'provisioning'",
        sql_lit(note),
        sql_lit(id),
    ))
    .await
    .map(|_| ())
}

/// Every resource released. Terminal.
pub async fn reaped(pool: &Pool, id: &str, at: u64) -> Result<(), PoolError> {
    pool.rows(&format!(
        "UPDATE \"{SPACES_TABLE}\" SET \"state\" = 'reaped', \"state_at\" = {at} \
         WHERE \"$space\" = {}",
        sql_lit(id)
    ))
    .await
    .map(|_| ())
}

/// Every tenant that is not serving, which is §4.1's one detection query:
/// *"Detection is therefore one query — `registry where state != 'active'`
/// — and it is on the operator dashboard as a mandatory pane, not a
/// runbook grep."*
pub async fn troubled(pool: &Pool) -> Result<Vec<Tenant>, PoolError> {
    Ok(all(pool)
        .await?
        .into_iter()
        // A paused tenant was held still on purpose and is not trouble.
        .filter(|t| !matches!(t.state, State::Active | State::Paused))
        .collect())
}

/// Mark every provision that has been `provisioning` too long as `failed`
/// (`bootstrap.md` §4.1, the `bootstrapTimeout` reconciler).
///
/// **It marks and does not release.** Releasing a tenant's database and
/// keys is `std.space.reap`, which is operator-only and step-up gated for
/// the reason §4.5 gives: a process that destroys databases with no human
/// asking will eventually destroy the wrong one. What this does is make a
/// stuck provision **visible** — `failed` is what puts it in [`troubled`],
/// which is §4.1's one detection query.
///
/// Lives here rather than in the worker so the policy sits with the table
/// it is about, and so it can be tested without standing up a supervisor.
///
/// Returns how many it timed out. A tenant that cannot be marked does not
/// stop the sweep: the next one may be the one somebody is waiting on.
pub async fn time_out_stale(pool: &Pool, timeout_secs: u64, now: u64) -> Result<u64, PoolError> {
    let cutoff = now.saturating_sub(timeout_secs);
    let mut timed_out = 0;
    for t in troubled(pool).await? {
        if t.state != State::Provisioning || t.state_at > cutoff {
            continue;
        }
        // A person's own tenant is never given up on (owner, 2026-10-21:
        // "it should never really stop trying"). The setup sweep keeps
        // trying it and reports it stuck instead.
        if t.own_of.is_some() {
            continue;
        }
        // The note names the phase, because that is what tells an operator
        // — and `std.space.reap` — how much was built before it stopped.
        let note = format!(
            "provisioning timed out after {timeout_secs}s at phase `{}`; target `{}`",
            t.phase.as_str(),
            if t.target.is_empty() {
                "(none chosen)"
            } else {
                &t.target
            },
        );
        match fail(pool, &t.id, &note, now).await {
            Ok(()) => timed_out += 1,
            Err(e) => eprintln!("provisioning.reconciler: `{}` not marked failed: {e}", t.id),
        }
    }
    Ok(timed_out)
}

// ── setting up a person's own tenant (owner, 2026-10-21) ─────────────────

/// **Take the right to set up tenant `id`**, for `lease_secs` from `now`.
///
/// One statement, guarded on the row still being `provisioning` and on no
/// other holder's lease being live, so two servers that ask at once get
/// one `true` between them. The holder that already has it renews it.
/// Answers whether `holder` now holds it.
pub async fn claim_setup(
    pool: &Pool,
    id: &str,
    holder: &str,
    now: u64,
    lease_secs: u64,
) -> Result<bool, PoolError> {
    let until = now.saturating_add(lease_secs);
    let rows = pool
        .rows(&format!(
            "UPDATE \"{SPACES_TABLE}\" SET \"lease_holder\" = {h}, \"lease_until\" = {until} \
             WHERE \"$space\" = {} AND \"state\" = 'provisioning' \
             AND (\"lease_until\" <= {now} OR \"lease_holder\" = {h}) \
             RETURNING \"$space\"",
            sql_lit(id),
            h = sql_lit(holder),
        ))
        .await?;
    Ok(!rows.is_empty())
}

/// Give the right back, so any server may take it once the row is due.
pub async fn release_setup(pool: &Pool, id: &str, holder: &str) -> Result<(), PoolError> {
    pool.rows(&format!(
        "UPDATE \"{SPACES_TABLE}\" SET \"lease_until\" = 0 \
         WHERE \"$space\" = {} AND \"lease_holder\" = {}",
        sql_lit(id),
        sql_lit(holder),
    ))
    .await
    .map(|_| ())
}

/// **A try at setting up failed; try again at `retry_at`.**
///
/// The row stays `provisioning`, so the person keeps their one tenant and
/// nothing is given up. The note is the deployment owner's, never shown to
/// the person. Answers how many tries have now failed.
pub async fn retry_later(
    pool: &Pool,
    id: &str,
    note: &str,
    retry_at: u64,
) -> Result<u64, PoolError> {
    let rows = pool
        .rows(&format!(
            "UPDATE \"{SPACES_TABLE}\" SET \"attempts\" = \"attempts\" + 1, \
             \"retry_at\" = {retry_at}, \"note\" = {}, \"lease_until\" = 0 \
             WHERE \"$space\" = {} AND \"state\" = 'provisioning' RETURNING \"attempts\"",
            sql_lit(note),
            sql_lit(id),
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|c| c.parse().ok())
        .unwrap_or(0))
}

/// Every own tenant still being set up, oldest first.
pub async fn setting_up(pool: &Pool) -> Result<Vec<Tenant>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT {COLUMNS} FROM \"{SPACES_TABLE}\" \
             WHERE \"state\" = 'provisioning' AND \"own_of\" IS NOT NULL \
             ORDER BY \"created_at\", \"$space\""
        ))
        .await?;
    Ok(rows.iter().map(|r| parse_row(r)).collect())
}

/// Record that tenant `id` was reported stuck, once. Answers whether this
/// call recorded it, so the report goes out from one server only.
pub async fn mark_stuck(pool: &Pool, id: &str, now: u64) -> Result<bool, PoolError> {
    let rows = pool
        .rows(&format!(
            "UPDATE \"{SPACES_TABLE}\" SET \"stuck_at\" = {now} \
             WHERE \"$space\" = {} AND \"state\" = 'provisioning' AND \"stuck_at\" = 0 \
             RETURNING \"$space\"",
            sql_lit(id),
        ))
        .await?;
    Ok(!rows.is_empty())
}

/// What `/health/deep` says about own tenants being set up: how many, and
/// each one reported stuck, by id, with its age, tries and step. Never the
/// person and never the note, which can name hosts. `null` when the
/// registry cannot be read.
pub async fn setup_report(pool: &Pool, now: u64) -> serde_json::Value {
    let Ok(all) = setting_up(pool).await else {
        return serde_json::Value::Null;
    };
    let stuck: Vec<serde_json::Value> = all
        .iter()
        .filter(|t| t.stuck_at > 0)
        .map(|t| {
            serde_json::json!({
                "space": t.id,
                "seconds": now.saturating_sub(t.created_at),
                "attempts": t.attempts,
                "step": t.phase.as_str(),
            })
        })
        .collect();
    serde_json::json!({
        "in_progress": all.len(),
        "stuck": stuck,
        "stuck_after_setting": "ORDO_PROVISION_STUCK_SECS",
    })
}

/// One tenant by id, or `None` for an id this deployment never minted.
pub async fn lookup(pool: &Pool, id: &str) -> Result<Option<Tenant>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT {COLUMNS} FROM \"{SPACES_TABLE}\" WHERE \"$space\" = {}",
            sql_lit(id)
        ))
        .await?;
    Ok(rows.first().map(|r| parse_row(r)))
}

/// The principal's own tenant as their provisioning status reads it: the
/// held one ([`own_of`]) when there is one, else the newest `failed` or
/// `reaped` one, else `None`.
pub async fn latest_own(pool: &Pool, principal: &str) -> Result<Option<Tenant>, PoolError> {
    if let Some(t) = own_of(pool, principal).await? {
        return Ok(Some(t));
    }
    let rows = pool
        .rows(&format!(
            "SELECT {COLUMNS} FROM \"{SPACES_TABLE}\" WHERE \"own_of\" = {} \
             ORDER BY \"created_at\" DESC, \"$space\" LIMIT 1",
            sql_lit(principal)
        ))
        .await?;
    Ok(rows.first().map(|r| parse_row(r)))
}

/// Every minted tenant, newest first, for an operator surface that has to
/// show a human which opaque id is whose.
pub async fn all(pool: &Pool) -> Result<Vec<Tenant>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT {COLUMNS} FROM \"{SPACES_TABLE}\" ORDER BY \"created_at\" DESC, \"$space\""
        ))
        .await?;
    Ok(rows.iter().map(|r| parse_row(r)).collect())
}

fn parse_row(row: &[Option<String>]) -> Tenant {
    let cell = |i: usize| {
        row.get(i)
            .and_then(|c| c.as_deref())
            .unwrap_or_default()
            .to_string()
    };
    Tenant {
        id: cell(0),
        name: cell(1),
        // A row whose `created_at` does not parse is a row from a future
        // this build does not know. Zero rather than a refusal: the id and
        // the name are the answer a caller came for, and a timestamp is
        // not worth failing a lookup over.
        created_at: cell(2).parse().unwrap_or(0),
        created_by: cell(3),
        // An unknown state reads as `Failed` rather than as `Active`. A
        // row this build cannot interpret is a row it must not serve from,
        // and `Failed` is the reading that keeps an operator looking at it
        // instead of a caller reaching it.
        state: State::parse(&cell(4)).unwrap_or(State::Failed),
        phase: Phase::parse(&cell(5)).unwrap_or(Phase::Reserved),
        target: cell(6),
        state_at: cell(7).parse().unwrap_or(0),
        note: cell(8),
        own_of: Some(cell(9)).filter(|p| !p.is_empty()),
        paused_at: cell(10).parse().unwrap_or(0),
        paused_by: cell(11),
        paused_note: cell(12),
        lease_until: cell(13).parse().unwrap_or(0),
        attempts: cell(14).parse().unwrap_or(0),
        retry_at: cell(15).parse().unwrap_or(0),
        stuck_at: cell(16).parse().unwrap_or(0),
    }
}

/// What asking to pause a tenant came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Paused {
    /// This call paused it.
    Now(Tenant),
    /// It was already paused. The record is the first pause's, unchanged:
    /// two pausers at once both read the one that landed.
    Already(Tenant),
    /// It is in a state that cannot be paused (`provisioning`, `failed`,
    /// `reaped`): there is nothing serving to hold still.
    NotServing(Tenant),
    /// This deployment never minted that id.
    Unknown,
}

/// **Pause a tenant** (27ar): `active` to `paused` in one statement guarded
/// on `state = 'active'`, so two pausers cannot both record it.
///
/// `by` is [`PAUSED_BY_TENANT`] or [`PAUSED_BY_DEPLOYMENT`]; `note` is from
/// [`PAUSE_NOTES`]. Both are checked by the caller, which answers the
/// refusal in its own words.
pub async fn pause(
    pool: &Pool,
    id: &str,
    by: &str,
    note: &str,
    at: u64,
) -> Result<Paused, PoolError> {
    let rows = pool
        .rows(&format!(
            "UPDATE \"{SPACES_TABLE}\" SET \"state\" = 'paused', \"state_at\" = {at}, \
             \"paused_at\" = {at}, \"paused_by\" = {}, \"paused_note\" = {} \
             WHERE \"$space\" = {} AND \"state\" = 'active' RETURNING {COLUMNS}",
            sql_lit(by),
            sql_lit(note),
            sql_lit(id),
        ))
        .await?;
    if let Some(row) = rows.first() {
        return Ok(Paused::Now(parse_row(row)));
    }
    Ok(match lookup(pool, id).await? {
        None => Paused::Unknown,
        Some(t) if t.state == State::Paused => Paused::Already(t),
        Some(t) => Paused::NotServing(t),
    })
}

/// What asking to resume a tenant came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resumed {
    /// This call resumed it.
    Now(Tenant),
    /// It was not paused. Nothing changed.
    NotPaused(Tenant),
    /// The deployment owner paused it, and only the deployment owner lifts
    /// that pause.
    HeldByDeployment(Tenant),
    /// This deployment never minted that id.
    Unknown,
}

/// **Resume a paused tenant** (27ar): `paused` back to `active`, guarded on
/// `state = 'paused'`.
///
/// `by_deployment` is true for `std.deployment.resumeTenant`, which lifts
/// either kind of pause. A resume from inside the tenant lifts only a pause
/// made from inside the tenant, and the guard says so in the statement.
pub async fn resume(
    pool: &Pool,
    id: &str,
    by_deployment: bool,
    at: u64,
) -> Result<Resumed, PoolError> {
    let whose = if by_deployment {
        String::new()
    } else {
        format!(" AND \"paused_by\" = {}", sql_lit(PAUSED_BY_TENANT))
    };
    let rows = pool
        .rows(&format!(
            "UPDATE \"{SPACES_TABLE}\" SET \"state\" = 'active', \"state_at\" = {at}, \
             \"paused_at\" = 0, \"paused_by\" = '', \"paused_note\" = '' \
             WHERE \"$space\" = {} AND \"state\" = 'paused'{whose} RETURNING {COLUMNS}",
            sql_lit(id),
        ))
        .await?;
    if let Some(row) = rows.first() {
        return Ok(Resumed::Now(parse_row(row)));
    }
    Ok(match lookup(pool, id).await? {
        None => Resumed::Unknown,
        Some(t) if t.state == State::Paused => Resumed::HeldByDeployment(t),
        Some(t) => Resumed::NotPaused(t),
    })
}

/// Every paused tenant. What each replica's pause register is loaded from.
pub async fn paused(pool: &Pool) -> Result<Vec<Tenant>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT {COLUMNS} FROM \"{SPACES_TABLE}\" WHERE \"state\" = 'paused'"
        ))
        .await?;
    Ok(rows.iter().map(|r| parse_row(r)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_id_is_one_the_door_will_bind() {
        for _ in 0..64 {
            let id = mint_space_id();
            assert!(
                ordo_api::well_formed_space(&id),
                "the door would refuse a tenant this deployment minted: {id}"
            );
        }
    }

    #[test]
    fn a_minted_id_carries_no_character_a_reader_confuses() {
        let id = mint_space_id();
        let body = id.strip_prefix("spc_").expect("the prefix");
        assert_eq!(body.len(), 26, "128 bits in Crockford base32: {id}");
        for c in body.chars() {
            assert!(
                !matches!(c, 'i' | 'l' | 'o' | 'u'),
                "`{c}` is in the alphabet Crockford excludes: {id}"
            );
            assert!(c.is_ascii_digit() || c.is_ascii_lowercase(), "{id}");
        }
    }

    #[test]
    fn minting_twice_does_not_produce_one_id_twice() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(mint_space_id()), "a minted id repeated");
        }
    }

    /// The `$` prefix is reserved for platform-owned spaces a caller cannot
    /// name, and a minted tenant must never land on one.
    #[test]
    fn a_minted_id_is_never_a_system_space() {
        for _ in 0..64 {
            let id = mint_space_id();
            assert!(
                !ordo_exec::erase::is_system_space(&ordo_crypto::SpaceId(id.clone())),
                "{id}"
            );
        }
    }

    /// The encoder against the vector Crockford's own specification gives,
    /// so a rewrite of [`base32`] cannot quietly change what an id says.
    #[test]
    fn the_encoder_agrees_with_a_known_vector() {
        assert_eq!(base32(&[0x00]), "00");
        assert_eq!(base32(&[0xff]), "zw");
        assert_eq!(base32(&[0x00, 0x00, 0x00, 0x00, 0x00]), "00000000");
        assert_eq!(base32(&[0xff; 5]), "zzzzzzzz");
        assert_eq!(base32(b"foo"), "csqpy");
    }
}
