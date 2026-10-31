//! `ordo keys` — the key ceremony.
//!
//! Custody that only a running server can create is custody nobody
//! performed. The ceremony (`seal-master`, `init`, `rewrap-master`) and
//! the epoch lifecycle (`status`, `rotate`, `reseal`, `destroy`), all
//! `--as`-attributed the way every other operator command already is.
//!
//! # The epoch lifecycle, in the order an operator meets it
//!
//! `rotate` mints the next epoch and enqueues the re-seal pass
//! (`plan-production-ops.md` K1); the `keys.reseal` worker on the
//! deployment moves every ciphertext off the old epoch. `status` answers
//! "is it finished, and if not, where is it stuck" without reading a
//! table. `reseal` is the **forced re-encrypt after an exposure**: it
//! enqueues a pass for every retired epoch that still holds anything, and
//! it is idempotent. `destroy` is the K2 gate: it counts all four
//! populations, refuses by name with the number, prints the one
//! consequence nothing can undo, and takes a `--confirm` token that names
//! the space and the epoch so a copied command for another space refuses.
//!
//! # A tenant is never provisioned as a boot side effect
//!
//! `LocalKeyProvider::release` refuses an unknown space *"rather than
//! silently creating keys, so a typo cannot mint a tenant"*, and the server
//! honours that: it refuses to boot when `ORDO_SPACE` names a tenant it has
//! no custody for, and prints this command. A binary that provisioned
//! whatever `ORDO_SPACE` said would turn a typo into a brand-new tenant
//! with brand-new keys, serving an empty space that looks perfectly
//! healthy, which is exactly what `TestProvider` does as a *convenience*,
//! with its own doc warning that it is convenience for fixtures and not a
//! custody model.
//!
//! **The one space that does not come from here** is
//! `ordo_host::provision::SYSTEM_SPACE`, the platform's own home, created
//! by the boot since checklist 17v. It is told apart from a tenant by where
//! its name comes from: a constant compiled into the binary, read from no
//! variable and no command line, so nothing an operator can mistype selects
//! it. This command still creates every tenant.
//!
//! # There is no escrow and there is no recovery
//!
//! Pinned by `implementation.md`: *"No silent escrow, ever. A recovery path
//! the user did not choose is a custody position they did not consent to."*
//! The consequence is one sentence and every one of these commands prints
//! it rather than letting an operator infer it:
//!
//! > The property that makes crypto-erasure real — that destroying key
//! > material is a delete, not a derivation — is the same property that
//! > makes losing key material unrecoverable.
//!
//! Splitting the passphrase across custodians (two-of-three, an envelope in
//! a safe) is an **operator practice this document describes and the
//! platform does not implement**. Building Shamir into `ordo-crypto` would
//! add a threshold scheme to a crate whose entire dependency list is three
//! crates, to solve a problem an envelope and a policy already solve — and
//! it would make the platform a participant in a recovery story the ruling
//! deliberately leaves to the person whose data it is.

use std::path::{Path, PathBuf};

use ordo_crypto::master;
use ordo_crypto::KeyProvider;
use ordo_exec::keystore::{self, PgKeyStore};
use ordo_exec::reseal;
use ordo_exec::{Pool, PoolConfig};

use crate::operator::{Flags, Outcome};

/// The loss table, printed by every command that creates or moves key
/// material. Four lines, because an operator who reads it once and acts on
/// it is worth more than a paragraph nobody finishes.
const LOSS: &str = "\n\
     Back up the sealed file and the passphrase SEPARATELY, to somewhere\n\
     that is not this deployment's database. There is no escrow and no\n\
     recovery: the property that makes crypto-erasure real is the property\n\
     that makes losing key material unrecoverable.\n";

pub fn run(sub: &str, flags: Flags, cfg: PoolConfig) -> Outcome {
    match sub {
        "seal-master" => seal_master(flags),
        "init" => init(flags, cfg),
        "rewrap-master" => rewrap_master(flags, cfg),
        "status" => status(flags, cfg),
        "rotate" => rotate(flags, cfg),
        "reseal" => reseal_cmd(flags, cfg),
        "destroy" => destroy(flags, cfg),
        other => Outcome::usage(format!(
            "unknown `ordo keys` command `{other}` \
             (seal-master | init | rewrap-master | status | rotate | reseal | destroy)"
        )),
    }
}

// ── seal-master ──────────────────────────────────────────────────────────

/// `ordo keys seal-master --out <path> [--key-id <id>] --passphrase-file <p>`
///
/// Mints 32 CSPRNG bytes, derives the KEK with argon2id at the pinned
/// profile, writes the `ordo-master-v1` file, and prints the key id and a
/// **fingerprint** — enough to tell two master keys apart in a log and
/// useless for recovering either. It never prints the key and never writes
/// it anywhere but the sealed file.
///
/// The passphrase comes from a **file**, not from a prompt and not from a
/// flag. A flag is in the shell history and in `ps`; a prompt cannot be
/// scripted, and a ceremony that cannot be scripted is a ceremony somebody
/// performs by hand differently each time. A file is what the orchestrator's
/// secret store already mounts.
fn seal_master(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--out", "--key-id", "--passphrase-file"]) {
        return Outcome::usage(e);
    }
    let out = match flags.require("--out") {
        Ok(v) => PathBuf::from(v),
        Err(e) => return Outcome::usage(e),
    };
    let pass_file = match flags.require("--passphrase-file") {
        Ok(v) => PathBuf::from(v),
        Err(e) => return Outcome::usage(e),
    };
    let key_id = flags
        .named
        .get("--key-id")
        .cloned()
        .unwrap_or_else(default_key_id);

    let pass = match std::fs::read_to_string(&pass_file) {
        Ok(p) => p.trim_end_matches(['\n', '\r']).to_string(),
        Err(e) => {
            return Outcome::failed(format!("passphrase file {}: {e}\n", pass_file.display()))
        }
    };
    if pass.len() < 12 {
        return Outcome::failed(
            "a master-key passphrase is at least 12 characters. There is one argon2id\n\
             derivation between it and every space's keys, and no second factor.\n"
                .into(),
        );
    }

    match master::seal_master(&out, &key_id, pass.as_bytes()) {
        Ok(fingerprint) => Outcome::ok(format!(
            "sealed  {}\nkey-id  {key_id}\nfp      {fingerprint}\n{LOSS}",
            out.display()
        )),
        Err(e) => Outcome::failed(format!("{e}\n")),
    }
}

/// A key id nobody has to invent: the date. Ids exist to tell two master
/// keys apart in a log, and "which one is older" is the question actually
/// asked when there are two.
fn default_key_id() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("master-{secs}")
}

// ── init ─────────────────────────────────────────────────────────────────

/// `ordo keys init --space <s> --as <who>` (+ the master-key flags)
///
/// Creates `$keycustody` if absent, then mints the space's SMK and epoch
/// 1's DEK. Idempotent — `create_space_with` returns early when an SMK
/// already exists — so an operator who runs it twice gets the same answer
/// rather than a second tenant.
fn init(flags: Flags, cfg: PoolConfig) -> Outcome {
    if let Err(e) = flags.only(&[
        "--space",
        "--as",
        "--db",
        "--master-key-file",
        "--passphrase-file",
    ]) {
        return Outcome::usage(e);
    }
    let space = match flags.require("--space") {
        Ok(v) => ordo_crypto::SpaceId(v.to_string()),
        Err(e) => return Outcome::usage(e),
    };
    let actor = match flags.require("--as") {
        Ok(v) => v.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let master = match master_from(&flags) {
        Ok(m) => m,
        Err(e) => return Outcome::failed(e),
    };

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return Outcome::failed(format!("runtime: {e}\n")),
    };
    // Idempotent, and it stays here: this command runs against databases no
    // server has booted against yet. Since 17v the server creates the same
    // table on the way up, so whichever reaches an empty database first
    // makes it, and neither has minted custody for anybody's tenant by
    // doing so.
    let pool = match rt.block_on(Pool::connect(cfg.clone())) {
        Ok(p) => p,
        Err(e) => return Outcome::failed(format!("database: {e}\n")),
    };
    if let Err(e) = rt.block_on(keystore::ensure_tables(&pool)) {
        return Outcome::failed(format!("$keycustody: {e}\n"));
    }

    let store = match PgKeyStore::open(cfg) {
        Ok(s) => s,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    // `plan-production-ops.md` R1b, the same check `runtime.rs::build_custody`
    // runs on every boot: `$keycustody` holds wrapped keys, never the master,
    // and never the key `derive_from_secret` would produce from it.
    if store.holds_bytes(&master) || store.holds_bytes(&master::derive_from_secret(&master)) {
        return Outcome::failed(
            "the master key is stored inside `$keycustody` — wrapped keys belong in the \
             backup, the master does not\n  remedy: remove the offending row, then rotate the \
             master with `ordo keys rewrap-master`\n"
                .into(),
        );
    }
    let provider = ordo_crypto::LocalKeyProvider::from_master(master, Box::new(store));
    let existed = provider.current_epoch(&space).is_ok();
    if let Err(e) = provider.create_space(&space) {
        return Outcome::failed(format!("{e}\n"));
    }
    // Provisioning is a decision about a space, and a decision about a
    // space belongs in the security-event stream — the exit ops' precedent.
    // `--as` is asserted, never authenticated: this tool holds database
    // credentials, so it attributes rather than proves.
    let recorded = rt.block_on(record_provisioned(&pool, &space, &actor, existed));

    let epoch = provider.current_epoch(&space).unwrap_or(0);
    let verdict = if existed {
        "already provisioned — nothing minted"
    } else {
        "provisioned"
    };
    Outcome::ok(format!(
        "space   {}\nstatus  {verdict}\nepoch   {epoch}\nas      {actor}\naudit   {recorded}\n{}",
        space.0,
        if existed { "" } else { LOSS }
    ))
}

/// Append a `custody-action` to the space's security-event stream.
///
/// Best-effort and **said so in the output**: a ceremony that refused
/// because an audit append failed would leave an operator with custody
/// half-created and no way to tell. The stream is the record of the
/// decision, not the mechanism of it.
async fn record_provisioned(
    pool: &Pool,
    space: &ordo_crypto::SpaceId,
    actor: &str,
    existed: bool,
) -> String {
    if existed {
        return "not recorded (nothing changed)".into();
    }
    record_custody(pool, space, actor, OP_KEYS_INIT, "space_provisioned").await
}

/// The op each custody record carries. Dotted, because the recorder keeps an
/// op only when it is `[A-Za-z0-9._-]` and records anything else, spaces
/// included, as `<unprintable>` (reported fix 25o).
pub const OP_KEYS_INIT: &str = "ordo.keys.init";
pub const OP_KEYS_ROTATE: &str = "ordo.keys.rotate";
pub const OP_KEYS_RESEAL: &str = "ordo.keys.reseal";
pub const OP_KEYS_DESTROY: &str = "ordo.keys.destroy";

/// Append one `custody-action` to the space's security-event stream. The
/// shape every command in this module records through.
async fn record_custody(
    pool: &Pool,
    space: &ordo_crypto::SpaceId,
    actor: &str,
    op: &str,
    action: &str,
) -> String {
    let recorder = ordo_secevent::Recorder::default();
    recorder.observe(
        ordo_secevent::Observation {
            space: Some(ordo_commandlog::SpaceId(space.0.clone())),
            subject: ordo_secevent::Subject::member(actor),
            op: op.into(),
            kind: ordo_secevent::EventKind::CustodyAction {
                action: action.into(),
            },
            at: ordo_commandlog::Timestamp(now_ms()),
            correlation: ordo_secevent::correlation::mint(),
        },
        ordo_secevent::Relationship::Member,
    );
    let entries = recorder.drain_all(ordo_commandlog::Timestamp(now_ms()));
    // One transaction for the window, the way the drain does it: the
    // entries of one act belong together.
    let tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => return format!("NOT RECORDED ({e}) — the ceremony still happened"),
    };
    let mut n = 0;
    for e in &entries {
        if let Err(err) = tx.append_security_entry(e).await {
            return format!("NOT RECORDED ({err}) — the ceremony still happened");
        }
        n += 1;
    }
    match tx.commit().await {
        Ok(()) => format!("recorded ({n} entry)"),
        Err(e) => format!("NOT RECORDED ({e}) — the ceremony still happened"),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── rewrap-master ────────────────────────────────────────────────────────

/// `ordo keys rewrap-master --from <old> --from-passphrase-file <p>
///  --to <new> --to-passphrase-file <p> --as <who>`
///
/// Reads every `$keycustody` SMK row, unwraps under the old master,
/// re-wraps under the new, **in one transaction**.
///
/// **This is not SMK rotation and the two must not be confused**, because
/// their costs differ by orders of magnitude:
///
/// | | moves | cost |
/// |---|---|---|
/// | master re-wrap (this) | the wrapping of each space's SMK | one row per space; no envelope and **no digest** moves |
/// | SMK rotation | every blind-index key, since BIKs are SMK-derived | a full re-index of every `sealed indexed` column |
/// | DEK rotation | the write epoch; old epochs stay readable | free at rotation, then a re-seal pass |
///
/// The new file must already exist — written by `seal-master` — so the
/// operator has had one chance to back it up before anything depends on it.
fn rewrap_master(flags: Flags, cfg: PoolConfig) -> Outcome {
    if let Err(e) = flags.only(&[
        "--from",
        "--from-passphrase-file",
        "--to",
        "--to-passphrase-file",
        "--as",
        "--db",
        "--dry-run",
    ]) {
        return Outcome::usage(e);
    }
    let (from, from_pass, to, to_pass) = match (
        flags.require("--from"),
        flags.require("--from-passphrase-file"),
        flags.require("--to"),
        flags.require("--to-passphrase-file"),
    ) {
        (Ok(a), Ok(b), Ok(c), Ok(d)) => (
            PathBuf::from(a),
            PathBuf::from(b),
            PathBuf::from(c),
            PathBuf::from(d),
        ),
        (a, b, c, d) => {
            let e = [a.err(), b.err(), c.err(), d.err()]
                .into_iter()
                .flatten()
                .next()
                .unwrap_or_default();
            return Outcome::usage(e);
        }
    };
    let actor = match flags.require("--as") {
        Ok(v) => v.to_string(),
        Err(e) => return Outcome::usage(e),
    };

    let old = match open_file(&from, &from_pass) {
        Ok(m) => m,
        Err(e) => return Outcome::failed(e),
    };
    let new = match open_file(&to, &to_pass) {
        Ok(m) => m,
        Err(e) => return Outcome::failed(e),
    };
    if old == new {
        return Outcome::failed(
            "`--from` and `--to` hold the same master key; a re-wrap to itself is a \
             no-op that would look like it worked\n"
                .into(),
        );
    }

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return Outcome::failed(format!("runtime: {e}\n")),
    };
    let pool = match rt.block_on(Pool::connect(cfg)) {
        Ok(p) => p,
        Err(e) => return Outcome::failed(format!("database: {e}\n")),
    };
    let dry = flags.switches.iter().any(|s| s == "--dry-run");
    match rt.block_on(keystore::rewrap_master(&pool, &old, &new, dry)) {
        Ok(spaces) => {
            let verb = if dry { "would re-wrap" } else { "re-wrapped" };
            Outcome::ok(format!(
                "{verb}  {} space(s)\nfrom    {} ({})\nto      {} ({})\nas      {actor}\n\
                 note    no envelope and no blind index moved; this is a re-wrap, \
                 not an SMK rotation\n{}",
                spaces.len(),
                from.display(),
                master::key_id_of(&from).unwrap_or_default(),
                to.display(),
                master::key_id_of(&to).unwrap_or_default(),
                if dry { "" } else { LOSS }
            ))
        }
        Err(e) => Outcome::failed(format!("{e}\n")),
    }
}

// ── the epoch lifecycle: status | rotate | reseal | destroy ──────────────

/// What the four lifecycle commands share: a runtime of their own, the
/// pool, and the deployment's custody opened under the operator's master.
struct Session {
    rt: tokio::runtime::Runtime,
    pool: Pool,
    provider: ordo_crypto::LocalKeyProvider,
    space: ordo_crypto::SpaceId,
    actor: String,
}

const LIFECYCLE_FLAGS: &[&str] = &[
    "--space",
    "--as",
    "--db",
    "--master-key-file",
    "--passphrase-file",
    "--reason",
    "--epoch",
    "--confirm",
];

fn open_session(flags: &Flags, cfg: PoolConfig) -> Result<Session, Outcome> {
    let space = match flags.require("--space") {
        Ok(v) => ordo_crypto::SpaceId(v.to_string()),
        Err(e) => return Err(Outcome::usage(e)),
    };
    let actor = match flags.require("--as") {
        Ok(v) => v.to_string(),
        Err(e) => return Err(Outcome::usage(e)),
    };
    let master = master_from(flags).map_err(Outcome::failed)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Outcome::failed(format!("runtime: {e}\n")))?;
    let pool = rt
        .block_on(Pool::connect(cfg.clone()))
        .map_err(|e| Outcome::failed(format!("database: {e}\n")))?;
    // The pass table and the payload tables: the counts read them, and a
    // deployment that has never booted has none of them yet. Both DDLs
    // are the same idempotent guards the server runs.
    rt.block_on(pool.ensure_platform_tables())
        .map_err(|e| Outcome::failed(format!("platform tables: {e}\n")))?;
    rt.block_on(ordo_exec::reseal::ensure_tables(&pool))
        .map_err(|e| Outcome::failed(format!("$reseal: {e}\n")))?;
    let store = PgKeyStore::open(cfg).map_err(|e| Outcome::failed(format!("{e}\n")))?;
    if store.holds_bytes(&master) || store.holds_bytes(&master::derive_from_secret(&master)) {
        return Err(Outcome::failed(
            "the master key is stored inside `$keycustody` — wrapped keys belong in the \
             backup, the master does not\n"
                .into(),
        ));
    }
    let provider = ordo_crypto::LocalKeyProvider::from_master(master, Box::new(store));
    if provider.current_epoch(&space).is_err() {
        return Err(Outcome::failed(format!(
            "no custody for space `{}` — run `ordo keys init --space {} --as <who>`\n",
            space.0, space.0
        )));
    }
    Ok(Session {
        rt,
        pool,
        provider,
        space,
        actor,
    })
}

/// The deployed program's schema, from the registry's active artifact.
///
/// The column counts need to know which columns are sealed, and the
/// artifact the deployment serves is the only honest source. A database
/// with no active program has no entity tables, so an empty schema is the
/// exact answer there, and the note says so rather than letting zero read
/// as "nothing to move".
fn deployed_schema(s: &Session) -> Result<(ordo_schema::Schema, Option<String>), Outcome> {
    // The registry table itself is created by `ordo apply`; a deployment
    // that never applied has no program and no entities, and an error
    // reading the table is that same absence.
    let active =
        s.rt.block_on(ordo_exec::registry::active(&s.pool))
            .unwrap_or_default();
    let (text, note) = match active {
        Some(a) => (a.source, None),
        None => (
            String::new(),
            Some("no program is deployed; the column count covers no entity".to_string()),
        ),
    };
    let src = crate::Sources::from_parts(vec![("deployed".to_string(), text)]);
    let (schema, _) = crate::load_sources(&src)
        .map_err(|e| Outcome::failed(format!("the deployed program does not load: {e}\n")))?;
    Ok((schema, note))
}

fn now_secs() -> u64 {
    now_ms() / 1000
}

/// `YYYY-MM-DD` from Unix seconds, so the consequence prints as a date an
/// operator can compare with a backup's timestamp.
fn iso_date(secs: u64) -> String {
    // Howard Hinnant's civil-from-days.
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// `ordo keys status --space <s> --as <who>`
///
/// Current epoch; every live epoch with its four counts and what stands
/// between it and destruction; every pass row with its cursor, counts,
/// attempts and the seconds since it last advanced.
fn status(flags: Flags, cfg: PoolConfig) -> Outcome {
    if let Err(e) = flags.only(LIFECYCLE_FLAGS) {
        return Outcome::usage(e);
    }
    let s = match open_session(&flags, cfg) {
        Ok(s) => s,
        Err(o) => return o,
    };
    let (schema, note) = match deployed_schema(&s) {
        Ok(v) => v,
        Err(o) => return o,
    };
    let current = s.provider.current_epoch(&s.space).unwrap_or(0);
    let epochs = match s.rt.block_on(keystore::epochs(&s.pool, &s.space)) {
        Ok(v) => v,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let passes = match s.rt.block_on(reseal::passes(&s.pool, &s.space)) {
        Ok(v) => v,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let now = now_secs();

    let mut out = format!("space    {}\ncurrent  epoch {current}\n", s.space.0);
    if let Some(n) = &note {
        out.push_str(&format!("note     {n}\n"));
    }
    out.push_str("\nEPOCHS\n  epoch  payloads  columns  blobrefs  objects  standing\n");
    for e in &epochs {
        let left = match s
            .rt
            .block_on(reseal::remaining(&s.pool, &schema, &s.space, *e))
        {
            Ok(v) => v,
            Err(err) => return Outcome::failed(format!("counting epoch {e}: {err}\n")),
        };
        let open: Vec<&reseal::PassRow> = passes
            .iter()
            .filter(|r| r.from_epoch == *e && r.state != reseal::PassState::Done)
            .collect();
        let standing = if *e == current {
            "current: new writes seal under it".to_string()
        } else if left.total() == 0 {
            let at = passes
                .iter()
                .filter(|r| r.from_epoch == *e)
                .filter_map(|r| r.finished_at)
                .max();
            match at {
                Some(t) => format!(
                    "destroyable: re-sealed {} (`ordo keys destroy --epoch {e}`)",
                    iso_date(t)
                ),
                None => format!("destroyable: nothing under it (`ordo keys destroy --epoch {e}`)"),
            }
        } else if let Some(r) = open.first() {
            let stuck = now.saturating_sub(r.advanced_at);
            format!(
                "pass {} on {}: advanced {stuck}s ago, attempts {}",
                r.state.as_str(),
                r.population,
                r.attempts
            )
        } else {
            "retired, no pass: run `ordo keys reseal`".to_string()
        };
        out.push_str(&format!(
            "  {e:<5}  {:<8}  {:<7}  {:<8}  {:<7}  {standing}\n",
            left.payloads, left.columns, left.blobrefs, left.objects
        ));
    }
    if !passes.is_empty() {
        out.push_str(
            "\nPASSES\n  from  to  population  state    done  skipped  attempts  cursor\n",
        );
        for r in &passes {
            out.push_str(&format!(
                "  {:<4}  {:<2}  {:<10}  {:<7}  {:<4}  {:<7}  {:<8}  {}\n",
                r.from_epoch,
                r.to_epoch,
                r.population,
                r.state.as_str(),
                r.rows_done,
                r.rows_skipped,
                r.attempts,
                r.cursor
                    .as_deref()
                    .map(|c| c.replace('\u{1f}', "/"))
                    .unwrap_or_else(|| "-".into())
            ));
        }
    }
    Outcome::ok(out)
}

/// `ordo keys rotate --space <s> --reason <r> --as <who>`
///
/// The one production caller of `KeyProvider::rotate`: mints the next
/// epoch, enqueues the pass, records the decision.
fn rotate(flags: Flags, cfg: PoolConfig) -> Outcome {
    if let Err(e) = flags.only(LIFECYCLE_FLAGS) {
        return Outcome::usage(e);
    }
    let reason = match flags.require("--reason") {
        Ok(v) => v.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let s = match open_session(&flags, cfg) {
        Ok(s) => s,
        Err(o) => return o,
    };
    let (schema, note) = match deployed_schema(&s) {
        Ok(v) => v,
        Err(o) => return o,
    };
    let old = s.provider.current_epoch(&s.space).unwrap_or(0);
    let new = match s.rt.block_on(reseal::enqueue_rotation(
        &s.pool,
        &s.provider,
        &s.space,
        &reason,
        now_secs(),
    )) {
        Ok(e) => e,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let left = match s
        .rt
        .block_on(reseal::remaining(&s.pool, &schema, &s.space, old))
    {
        Ok(v) => v,
        Err(e) => return Outcome::failed(format!("counting epoch {old}: {e}\n")),
    };
    let recorded = s.rt.block_on(record_custody(
        &s.pool,
        &s.space,
        &s.actor,
        OP_KEYS_ROTATE,
        &format!("epoch_rotated {old}->{new} reason={reason}"),
    ));
    Outcome::ok(format!(
        "space    {}\nrotated  {old} -> {new}\nreason   {reason}\npass     enqueued: \
         payloads {}, columns {}, blobrefs {}, objects {}\n{}as       {}\naudit    {recorded}\n\
         note     the deployment's keys.reseal worker moves them; \
         `ordo keys status --space {}` reports progress\n",
        s.space.0,
        left.payloads,
        left.columns,
        left.blobrefs,
        left.objects,
        note.map(|n| format!("note     {n}\n")).unwrap_or_default(),
        s.actor,
        s.space.0,
    ))
}

/// `ordo keys reseal --space <s> --as <who>` — the forced re-encrypt.
///
/// Every retired epoch that still holds anything and has no open pass
/// gets one, targeted at the current epoch. Idempotent: run it twice and
/// the second run enqueues nothing.
fn reseal_cmd(flags: Flags, cfg: PoolConfig) -> Outcome {
    if let Err(e) = flags.only(LIFECYCLE_FLAGS) {
        return Outcome::usage(e);
    }
    let s = match open_session(&flags, cfg) {
        Ok(s) => s,
        Err(o) => return o,
    };
    let (schema, note) = match deployed_schema(&s) {
        Ok(v) => v,
        Err(o) => return o,
    };
    let current = s.provider.current_epoch(&s.space).unwrap_or(0);
    let epochs = match s.rt.block_on(keystore::epochs(&s.pool, &s.space)) {
        Ok(v) => v,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let passes = match s.rt.block_on(reseal::passes(&s.pool, &s.space)) {
        Ok(v) => v,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let mut lines = Vec::new();
    for e in epochs.into_iter().filter(|e| *e != current) {
        let left = match s
            .rt
            .block_on(reseal::remaining(&s.pool, &schema, &s.space, e))
        {
            Ok(v) => v,
            Err(err) => return Outcome::failed(format!("counting epoch {e}: {err}\n")),
        };
        if left.total() == 0 {
            continue;
        }
        let open = passes
            .iter()
            .any(|r| r.from_epoch == e && r.state != reseal::PassState::Done);
        if open {
            lines.push(format!(
                "epoch {e}: a pass is already open ({} left)",
                left.total()
            ));
            continue;
        }
        match s.rt.block_on(reseal::enqueue_pass(
            &s.pool,
            &s.space,
            e,
            current,
            "forced re-encrypt",
            now_secs(),
        )) {
            Ok(_) => lines.push(format!(
                "epoch {e}: pass enqueued (payloads {}, columns {}, blobrefs {}, objects {})",
                left.payloads, left.columns, left.blobrefs, left.objects
            )),
            Err(err) => return Outcome::failed(format!("{err}\n")),
        }
    }
    if lines.is_empty() {
        return Outcome::ok(format!(
            "space    {}\nreseal   nothing to re-seal: every retired epoch is at zero\n{}",
            s.space.0,
            note.map(|n| format!("note     {n}\n")).unwrap_or_default(),
        ));
    }
    let recorded = s.rt.block_on(record_custody(
        &s.pool,
        &s.space,
        &s.actor,
        OP_KEYS_RESEAL,
        "reseal_forced",
    ));
    Outcome::ok(format!(
        "space    {}\n{}\n{}as       {}\naudit    {recorded}\n",
        s.space.0,
        lines
            .iter()
            .map(|l| format!("reseal   {l}"))
            .collect::<Vec<_>>()
            .join("\n"),
        note.map(|n| format!("note     {n}\n")).unwrap_or_default(),
        s.actor,
    ))
}

/// `ordo keys destroy --space <s> --epoch <n> --as <who> --confirm <s>:<n>`
///
/// The K2 gate. Refuses the current epoch by name; refuses a non-zero
/// population by name with the count; prints the consequence and asks
/// for the token; destroys; records.
fn destroy(flags: Flags, cfg: PoolConfig) -> Outcome {
    if let Err(e) = flags.only(LIFECYCLE_FLAGS) {
        return Outcome::usage(e);
    }
    let epoch: ordo_crypto::Epoch = match flags.require("--epoch").map(str::parse) {
        Ok(Ok(n)) => n,
        Ok(Err(_)) => return Outcome::usage("`--epoch` must be a number from 1 to 255".into()),
        Err(e) => return Outcome::usage(e),
    };
    let s = match open_session(&flags, cfg) {
        Ok(s) => s,
        Err(o) => return o,
    };
    let (schema, note) = match deployed_schema(&s) {
        Ok(v) => v,
        Err(o) => return o,
    };
    let gate = match s.rt.block_on(reseal::destroy_gate(
        &s.pool,
        &schema,
        &s.provider,
        &s.space,
        epoch,
    )) {
        Ok(g) => g,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let resealed_at = match gate {
        reseal::Gate::Current => {
            return Outcome::failed(format!(
                "refused: epoch {epoch} is the one new writes seal under in `{}`; \
                 `ordo keys rotate` first\n",
                s.space.0
            ))
        }
        reseal::Gate::Refused { population, count } => {
            return Outcome::failed(format!(
                "refused: {count} {population} still under epoch {epoch} in `{}`\n  \
                 run `ordo keys reseal --space {} --as {}` and watch `ordo keys status`\n{}",
                s.space.0,
                s.space.0,
                s.actor,
                note.map(|n| format!("note     {n}\n")).unwrap_or_default(),
            ))
        }
        reseal::Gate::Clear { resealed_at } => resealed_at,
    };
    let date = resealed_at.map(iso_date);
    let consequence = format!(
        "epoch     {epoch} in space {}\nunder it  payloads 0, columns 0, blobrefs 0, objects 0\n\
         re-sealed {}\n\
         consequence: backups and archived log bundles taken before {} hold this epoch's \
         ciphertext and will not open for sealed fields in this space once it is destroyed. \
         Destroy only after a backup taken since the re-seal completed.\n",
        s.space.0,
        date.as_deref()
            .unwrap_or("never (nothing was ever under it)"),
        date.as_deref().unwrap_or("today"),
    );
    let token = format!("{}:{epoch}", s.space.0);
    match flags.named.get("--confirm").map(String::as_str) {
        None => {
            return Outcome::failed(format!(
                "{consequence}to proceed: add `--confirm {token}`\n"
            ))
        }
        Some(t) if t != token => {
            return Outcome::failed(format!(
                "{consequence}refused: `--confirm {t}` does not name this space and epoch \
                 (expected `--confirm {token}`)\n"
            ))
        }
        Some(_) => {}
    }
    if let Err(e) = s.provider.destroy_epoch(&s.space, epoch) {
        return Outcome::failed(format!("{e}\n"));
    }
    let recorded = s.rt.block_on(record_custody(
        &s.pool,
        &s.space,
        &s.actor,
        OP_KEYS_DESTROY,
        &format!("epoch_destroyed {epoch}"),
    ));
    Outcome::ok(format!(
        "{consequence}destroyed epoch {epoch}\nas        {}\naudit     {recorded}\n",
        s.actor
    ))
}

fn open_file(path: &Path, pass: &Path) -> Result<[u8; 32], String> {
    let p = std::fs::read_to_string(pass)
        .map_err(|e| format!("passphrase file {}: {e}\n", pass.display()))?;
    master::open_sealed_master(path, p.trim_end_matches(['\n', '\r']).as_bytes())
        .map_err(|e| format!("{}: {e}\n", path.display()))
}

/// The master key, from the sealed file when given and from
/// `ORDO_MASTER_KEY` otherwise.
///
/// The same two sources the server has, in the same order of preference, so
/// the ceremony and the deployment cannot end up holding different keys
/// because they read the environment differently.
pub(crate) fn master_from(flags: &Flags) -> Result<[u8; 32], String> {
    match (
        flags.named.get("--master-key-file"),
        flags.named.get("--passphrase-file"),
    ) {
        (Some(f), Some(p)) => open_file(&PathBuf::from(f), &PathBuf::from(p)),
        (Some(_), None) | (None, Some(_)) => Err(
            "--master-key-file and --passphrase-file come together; neither alone opens \
             anything\n"
                .into(),
        ),
        // The same rule as the server's `local` provider (S6-009): these
        // commands mint and open the wrapped keys that reach backups, so an
        // env master is development only here too.
        (None, None) if !master::dev_unsafe_from_env() => {
            Err(format!("{}\n", master::env_master_refusal()))
        }
        (None, None) => master::MasterKeySource::Env
            .open()
            .map_err(|e| format!("{e}\n")),
    }
}
