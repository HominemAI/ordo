//! `ordo apply`, `ordo rollback`, `ordo deploys`, `ordo prune`,
//! `ordo verify`, `ordo checkpoint`, `ordo revert`, `ordo migrate` — the
//! operator surface (8i, 5f, 13b).
//!
//! `check` and `plan` are deploy-time commands: they read `.ordo` files and
//! answer from the compiler alone. These three are the other half of the
//! tool. They talk to a **running deployment's database**, and they exist
//! because `verify_chain`, `checkpoint` and `revert_group` were all
//! implemented, all correct, and reachable from nothing but their own unit
//! tests. Undo is the platform's headline safety net; a safety net with no
//! handle is a claim, not a net.
//!
//! # Why the CLI, and what that costs
//!
//! An operator surface has to be reachable when the thing it operates on is
//! in trouble, which argues against putting it behind the same HTTP door
//! whose behaviour you might be investigating: `verify` in particular is a
//! cron job that must run when the engine is refusing requests, and it must
//! be runnable by someone who has the database and the published verifying
//! key and nothing else. A process with an exit code is what cron consumes,
//! so that is what these are.
//!
//! The cost is honest and worth stating: `ordo-cli` now depends on
//! `ordo-exec`, so building the `ordo` binary builds the driver, the pool
//! and (transitively, because `ordo-exec` hosts the write protocol) the
//! conformance harness. The alternative — a second binary — would have put
//! the operator's three commands somewhere they would not be found next to
//! `check` and `plan`.
//!
//! # Exit codes, because an operator's cron reads those and not prose
//!
//! | code | meaning |
//! |---|---|
//! | 0 | the command did what it says: verified, signed, reverted |
//! | 1 | the operation failed or was refused — unknown group, `access_guard`, `cas_miss`, no database |
//! | 2 | usage: a missing or unknown flag |
//! | 3 | **verification failed** — the chain is broken, or could not be read |
//!
//! 3 is separate from 1 on purpose. "The chain at index 41 does not
//! recompute" and "I could not reach the database" must page different
//! people, and a cron job that only knows `!= 0` will treat a network blip
//! as evidence of tampering, cry wolf once, and be disabled.
//!
//! `TamperKind::Unreadable` exits 3 with the rest, and that is deliberate
//! too: `security-logging-design.md` is explicit that "I could not check"
//! and "it checks out" must not be the same value, so an unreadable entry
//! is a verification failure. The printed line says which kind it was.

use std::collections::BTreeMap;

use ordo_commandlog::{
    CheckpointVerifier, CommandGroup, CommandKind, CorrelationId, Ed25519Signer, Image,
    RestoreSource, SpaceId, Timestamp, Value,
};
use ordo_exec::secevent::StreamCheckpointed;
use ordo_exec::undo::{self, Anchor, Checkpointed, Db};
use ordo_exec::witness::Witness;
use ordo_exec::{Pool, PoolConfig};

/// What a command produced: text for the operator, and the number the
/// shell gets.
pub struct Outcome {
    pub report: String,
    pub code: i32,
}

impl Outcome {
    pub(crate) fn ok(report: String) -> Outcome {
        Outcome { report, code: 0 }
    }
    pub(crate) fn failed(report: String) -> Outcome {
        Outcome { report, code: 1 }
    }
    pub(crate) fn usage(report: String) -> Outcome {
        Outcome { report, code: 2 }
    }
    fn tampered(report: String) -> Outcome {
        Outcome { report, code: 3 }
    }
}

/// The three commands' flags, parsed once.
///
/// Deliberately tiny and deliberately strict: an unknown flag is a usage
/// error rather than something ignored. A typo'd `--spce` that silently
/// verified the wrong space would be the worst possible failure of a tool
/// whose whole job is to tell you the truth about your history.
#[derive(Default)]
pub struct Flags {
    pub(crate) positional: Vec<String>,
    pub(crate) named: BTreeMap<String, String>,
    pub(crate) switches: Vec<String>,
    /// Every `--flag value` pair in the order it was typed, so a flag that
    /// is **repeatable** keeps all of its values. `named` holds the last of
    /// each, which is what every single-valued flag wants.
    pub(crate) repeated: Vec<(String, String)>,
}

const SWITCHES: &[&str] = &[
    "--from-genesis",
    "--dry-run",
    "--no-activate",
    "--no-wait",
    "--apply",
    "--record",
    "--unattested",
    "--again",
    "--objects",
    "--tenant",
    // `ordo verify --deployment`: the deployment's own security stream,
    // which names no space and so takes no value.
    "--deployment",
    // `ordo checkpoint --security`: which of the two artifacts to sign. A
    // switch and not a value, because there are exactly two artifacts and
    // each has its own key, its own table and its own flags already.
    "--security",
    // `ordo verify --identity`: reconcile every link in `$authenticator`
    // against the deployment's signed security stream.
    "--identity",
    // `ordo triggers deregister --force`: retire a listener the running
    // deployment has not confirmed orphaned.
    "--force",
];

fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            let flag = format!("--{name}");
            if SWITCHES.contains(&flag.as_str()) {
                f.switches.push(flag);
                continue;
            }
            let value = it
                .next()
                .ok_or_else(|| format!("`{flag}` needs a value"))?
                .clone();
            f.repeated.push((flag.clone(), value.clone()));
            f.named.insert(flag, value);
        } else {
            f.positional.push(a.clone());
        }
    }
    Ok(f)
}

impl Flags {
    pub(crate) fn require(&self, name: &str) -> Result<&str, String> {
        self.named
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("`{name}` is required"))
    }

    /// One named flag's value, when it was given.
    ///
    /// `pub(crate)` rather than exposing `named`, so a reader of this
    /// struct can still see every place a flag is consulted.
    pub(crate) fn named_value(&self, name: &str) -> Option<&str> {
        self.named.get(name).map(String::as_str)
    }

    /// Every value given for a repeatable flag, in order.
    pub(crate) fn all_values(&self, name: &str) -> Vec<&str> {
        self.repeated
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }

    /// Refuse anything this command does not understand.
    pub(crate) fn only(&self, allowed: &[&str]) -> Result<(), String> {
        for k in self.named.keys().chain(self.switches.iter()) {
            if !allowed.contains(&k.as_str()) {
                return Err(format!("unknown flag `{k}` for this command"));
            }
        }
        Ok(())
    }
}

/// The pool these commands run against: `PoolConfig::from_env`, with
/// `--db` overriding the database name so one deployment's operator can
/// point at one space's database without exporting a variable.
fn pool_config(f: &Flags) -> PoolConfig {
    let mut cfg = PoolConfig::from_env();
    // Two connections: these commands are sequential, and an operator tool
    // must not take a share of a live deployment's connection budget it
    // does not need.
    cfg.size = 2;
    if let Some(db) = f.named.get("--db") {
        cfg.db = db.clone();
    }
    cfg
}

/// The verifying key, from flags or from the published environment.
///
/// Both halves are required together. A key id without a key cannot check
/// a signature, and a key without an id cannot check that the signature is
/// the one this receiver was told to expect — `verify_checkpoint` binds the
/// two, because a checkpoint that says "signed by B" must not pass under
/// A's public key just because A's key was the one to hand.
fn verifier(f: &Flags) -> Result<Option<CheckpointVerifier>, String> {
    let id = f
        .named
        .get("--key-id")
        .cloned()
        .or_else(|| std::env::var("ORDO_CHECKPOINT_KEY_ID").ok());
    let hex = f
        .named
        .get("--public-key")
        .cloned()
        .or_else(|| std::env::var("ORDO_CHECKPOINT_PUBKEY").ok());
    match (id, hex) {
        (None, None) => Ok(None),
        (Some(id), Some(hex)) => CheckpointVerifier::from_public_hex(id, &hex)
            .map(Some)
            .map_err(|e| e.to_string()),
        (Some(_), None) => Err("`--key-id` was given without `--public-key`".into()),
        (None, Some(_)) => Err("`--public-key` was given without `--key-id`".into()),
    }
}

/// The **security** stream's verifying key, which is a different key from
/// the command chain's on purpose.
///
/// `ordo_secevent::store`'s own doc gives the reason: if one key signed both
/// artifacts, *"compromising the key that signs routine domain attestations
/// is also the ability to forge the record of the compromise"*. So this
/// reads its own flags and its own variables, and `--key-id` is never
/// silently reused for it — a verifier handed the wrong key must say so
/// rather than report a green stream it checked under a key that could not
/// have signed it.
fn security_verifiers(f: &Flags) -> Result<Vec<CheckpointVerifier>, String> {
    let ids = f.all_values("--security-key-id");
    let hexes = f.all_values("--security-public-key");
    if ids.is_empty() && hexes.is_empty() {
        // The environment form, which carries one key and is the ordinary
        // single-key deployment.
        return match (
            std::env::var(ordo_secevent::SECURITY_KEY_ID_ENV).ok(),
            std::env::var(ordo_secevent::SECURITY_PUBKEY_ENV).ok(),
        ) {
            (Some(id), Some(hex)) => CheckpointVerifier::from_public_hex(id, &hex)
                .map(|v| vec![v])
                .map_err(|e| e.to_string()),
            _ => Ok(Vec::new()),
        };
    }
    // **Paired by position**, so two keys are two `--security-key-id
    // <id> --security-public-key <hex>` pairs and a mismatched count is a
    // usage error rather than a silent pairing of the wrong id to the wrong
    // key — which would verify nothing and report tampering.
    if ids.len() != hexes.len() {
        return Err(format!(
            "`--security-key-id` was given {} time(s) and `--security-public-key` {} — they \
             pair up in order, so each key needs both",
            ids.len(),
            hexes.len()
        ));
    }
    let mut out = Vec::with_capacity(ids.len());
    for (id, hex) in ids.into_iter().zip(hexes) {
        out.push(
            CheckpointVerifier::from_public_hex(id.to_string(), hex).map_err(|e| e.to_string())?,
        );
    }
    Ok(out)
}

fn now() -> Timestamp {
    Timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    )
}

/// Run one of the three commands. `args` is everything after the verb.
pub fn run(cmd: &str, args: &[String]) -> Outcome {
    let flags = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => return Outcome::usage(e),
    };
    // A current-thread runtime: these commands are one sequence of queries
    // each, and a work-stealing pool would buy nothing but threads.
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return Outcome::failed(format!("runtime: {e}")),
    };
    // `plan --against` needs its own runtime (it is one long sequence of
    // probes rather than the short scripts below), so it takes the flags
    // and builds one rather than borrowing this one.
    if cmd == "plan" {
        let Some(path) = flags.positional.first().cloned() else {
            return Outcome::usage(
                "plan --against needs a program (a `.ordo` file or a directory)".into(),
            );
        };
        drop(rt);
        return crate::migrate::run(&path, flags);
    }
    // `migrate --apply` is the plan plus execution, and builds its runtime
    // the same way and for the same reason.
    if cmd == "migrate" {
        let Some(path) = flags.positional.first().cloned() else {
            return Outcome::usage(
                "migrate --apply needs a program (a `.ordo` file or a directory)".into(),
            );
        };
        drop(rt);
        return crate::migrate::run_migrate(&path, flags);
    }
    // `keys` builds its own runtime per subcommand — `seal-master` needs
    // none at all (it is a file and an argon2 derivation), and the two that
    // do talk to Postgres also stand up a `PgKeyStore`, which owns a thread
    // and a runtime of its own. Handing them this one would nest runtimes.
    if cmd == "keys" {
        let Some(sub) = flags.positional.first().cloned() else {
            return Outcome::usage(
                "ordo keys <seal-master | init | rewrap-master | status | rotate | reseal | destroy>\n"
                    .into(),
            );
        };
        let cfg = pool_config(&flags);
        drop(rt);
        let mut flags = flags;
        flags.positional.remove(0);
        return crate::keys::run(&sub, flags, cfg);
    }
    // `identity` runs on the blocking stores of `ordo-host`, which own a
    // thread and a runtime each, so it builds its own for the one statement
    // it makes itself rather than nest inside this one.
    if cmd == "identity" {
        let Some(sub) = flags.positional.first().cloned() else {
            return Outcome::usage("ordo identity <recover>\n".into());
        };
        let cfg = pool_config(&flags);
        drop(rt);
        let mut flags = flags;
        flags.positional.remove(0);
        return crate::identity::run(&sub, flags, cfg);
    }
    match cmd {
        "verify" => rt.block_on(verify(flags)),
        "checkpoint" => rt.block_on(checkpoint(flags)),
        "revert" => rt.block_on(revert(flags)),
        "apply" => rt.block_on(apply(flags)),
        "rollback" => rt.block_on(rollback(flags)),
        "deploys" => rt.block_on(deploys(flags)),
        "prune" => rt.block_on(prune(flags)),
        "restore" => rt.block_on(restore(flags)),
        "quota" => rt.block_on(quota(flags)),
        "usage" => rt.block_on(usage(flags)),
        "space" => rt.block_on(space_cmd(flags)),
        "console-key" => rt.block_on(console_key_cmd(flags)),
        "import" => rt.block_on(import_cmd(flags)),
        "fleet" => rt.block_on(fleet_cmd(flags)),
        "triggers" => rt.block_on(crate::triggers::run(flags)),
        _ => Outcome::usage(format!("unknown command `{cmd}`")),
    }
}

/// `ordo verify --proof <file>` -- check an exported chain proof with no
/// database at all.
///
/// The key comes from the same two flags the online mode uses, and the same
/// environment fallbacks, so an operator does not learn two spellings. Absent,
/// it is structure-only and the report says the word.
fn verify_proof_file(path: &str, flags: &Flags) -> Outcome {
    let doc = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(e) => return Outcome::failed(format!("{path}: {e}")),
    };
    let verifier = match verifier(flags) {
        Ok(v) => v,
        Err(e) => return Outcome::usage(e),
    };
    let pinned = flags.named.get("--head").map(String::as_str);
    match ordo_verify_proof::verify_proof(&doc, verifier.as_ref(), pinned) {
        Ok(v) => {
            let mut out = format!("{}\n", v.summary());
            for f in &v.findings {
                out.push_str(&format!("  [{}] {}\n", f.step, f.detail));
            }
            out.push_str(&format!(
                "  verifiable from index {} (unsigned; a feasibility hint, not a claim)\n",
                v.verifiable_from
            ));
            for c in v.caveats() {
                out.push_str(&format!("  NOTE: {c}\n"));
            }
            if v.ok() {
                Outcome::ok(out)
            } else {
                Outcome::tampered(out)
            }
        }
        Err(e) => Outcome::failed(format!("could not read the proof: {e}")),
    }
}

pub(crate) async fn connect(flags: &Flags) -> Result<Pool, Outcome> {
    Pool::connect(pool_config(flags))
        .await
        .map_err(|e| Outcome::failed(format!("database: {e}")))
}

/// The witness handle over the **control** database (`--control-db`,
/// defaulting to `--db`).
///
/// Defaulting to `--db` is the single-database deployment, which is the
/// common one and the one whose limit the howto states: one database means
/// the witness is restored with the chain by a whole-database PITR, and the
/// receivers' attestations are what catch that case. A routed deployment
/// passes `--control-db` and gets a record a space restore cannot touch.
///
/// A second pool rather than a clone when the names differ, because they
/// are two databases.
pub(crate) async fn witness_for(flags: &Flags, space_pool: &Pool) -> Result<Witness, Outcome> {
    match flags.named.get("--control-db") {
        Some(db) if Some(db) != flags.named.get("--db") => {
            let mut cfg = pool_config(flags);
            cfg.db = db.clone();
            Pool::connect(cfg)
                .await
                .map(Witness::new)
                .map_err(|e| Outcome::failed(format!("control database: {e}")))
        }
        _ => Ok(Witness::new(space_pool.clone())),
    }
}

// ── verify ───────────────────────────────────────────────────────────────

async fn verify(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&[
        "--space",
        "--db",
        "--key-id",
        "--public-key",
        "--from-genesis",
        "--proof",
        "--head",
        "--control-db",
        "--attestation",
        "--objects",
        "--master-key-file",
        "--passphrase-file",
        "--security-key-id",
        "--security-public-key",
        "--deployment",
        "--identity",
    ]) {
        return Outcome::usage(e);
    }
    // `--proof` is the offline mode, and it short-circuits before anything
    // touches a database. It is here because an operator looks for
    // verification beside the other verification commands; the **normative**
    // home is `ordo-verify-proof`, a binary that links no driver at all, and
    // a third party is pointed there rather than at this one. Both call
    // `ordo_verify_proof::verify_proof`, so there is one routine and two
    // front doors -- a second implementation of a verifier is the one place
    // a disagreement would read as tampering.
    if let Some(path) = flags.named.get("--proof").cloned() {
        return verify_proof_file(&path, &flags);
    }
    // **The security keys, validated here and not inside the report.** A
    // mismatched `--security-key-id`/`--security-public-key` pair is a usage
    // error — you passed the flags wrong — and surfacing it inside the
    // security paragraph made it exit 3, the code for *the record is wrong*.
    // Those are opposite findings and an operator's scripts watch for the
    // second one.
    let security_keys = match security_verifiers(&flags) {
        Ok(v) => v,
        Err(e) => return Outcome::usage(e),
    };
    // **The deployment security stream has no space and no command chain.**
    // `security-logging-design.md` §6.1 routes everything with no space
    // relationship here — unverified callers, non-members, tokens naming a
    // space they have nothing to do with — plus the platform's own custody
    // and key-release events. It is the operator's artifact, and it is the
    // only place a **cross-space sweep** is visible, because a principal
    // probing a hundred spaces appears in none of their per-space streams by
    // design. So it gets its own mode rather than being folded into a
    // space's: there is no `--space` that would name it, and a caller who
    // passed one would be verifying a different stream.
    // **`--identity`** is the deployment's stream, verified, and then every
    // link in the directory reconciled against it (`plan-passkey-recovery.md`
    // R8). A link with no record behind it is one somebody wrote by hand, and
    // is a verification failure, exit 3, because the stream is the artifact
    // meant to be believed when a table is in doubt.
    if flags.has("--identity") {
        if flags.named.contains_key("--space") {
            return Outcome::usage(
                "`--identity` reconciles the deployment's directory against the deployment's \
                 own security stream, which belongs to no space, so `--space` does not apply"
                    .to_string(),
            );
        }
        let pool = match connect(&flags).await {
            Ok(p) => p,
            Err(o) => return o,
        };
        let scope = ordo_secevent::Scope::Deployment;
        let mut failed = false;
        let mut out = String::new();
        match security_report(&security_keys, &pool, &scope).await {
            Ok((lines, bad)) => {
                out.push_str(lines.trim_start());
                failed |= bad;
            }
            Err(e) => {
                out.push_str(&format!("security  {}\nFAILED    {e}\n", scope.key().0));
                failed = true;
            }
        }
        out.push('\n');
        match crate::identity::reconcile(&pool).await {
            Ok((lines, bad)) => {
                out.push_str(&lines);
                failed |= bad;
            }
            Err(e) => {
                out.push_str(&format!("identity  FAILED    {e}\n"));
                failed = true;
            }
        }
        return if failed {
            Outcome::tampered(out)
        } else {
            Outcome::ok(out)
        };
    }
    if flags.has("--deployment") {
        if flags.named.contains_key("--space") {
            return Outcome::usage(
                "`--deployment` verifies the deployment's own security stream, which belongs to \
                 no space, so `--space` does not apply to it"
                    .to_string(),
            );
        }
        let pool = match connect(&flags).await {
            Ok(p) => p,
            Err(o) => return o,
        };
        let scope = ordo_secevent::Scope::Deployment;
        return match security_report(&security_keys, &pool, &scope).await {
            Ok((lines, bad)) => {
                let text = lines.trim_start().to_string();
                if bad {
                    Outcome::tampered(text)
                } else {
                    Outcome::ok(text)
                }
            }
            Err(e) => Outcome::tampered(format!("security  {}\nFAILED    {e}\n", scope.key().0)),
        };
    }
    let space = match flags.require("--space") {
        Ok(s) => SpaceId(s.to_string()),
        Err(e) => return Outcome::usage(e),
    };
    // `--objects` is the third mode `docs/plan-blob-chain.md` §4.5 names,
    // and it is independent of the other two: chain integrity ("the
    // record is intact") is a question about hashes and needs no schema,
    // no key and no store; object integrity ("the world still matches
    // it") needs all three, because it has to know which columns are
    // blob columns and it has to open what the chain recorded. It
    // short-circuits here, before an anchor is chosen, for the same
    // reason `--proof` does: it is not that mode's question.
    if flags.has("--objects") {
        return verify_objects(flags, space).await;
    }
    let verifier = match verifier(&flags) {
        Ok(v) => v,
        Err(e) => return Outcome::usage(e),
    };
    // The attestation files first: a document for another space is a usage
    // error, and a usage error must not be reported after a verification
    // that already ran.
    let receivers = match receivers_from(&flags, &space) {
        Ok(r) => r,
        Err(o) => return o,
    };
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    let db = Db::Pool(&pool);

    // The anchor is chosen and *reported*, never assumed. An operator
    // reading a green line has to be able to tell "the whole chain
    // recomputes" from "the suffix since a signed marker recomputes"; they
    // are different claims and only one of them covers last month.
    let (anchor, anchor_hash) = if flags.has("--from-genesis") {
        (Anchor::Genesis, ordo_commandlog::ChainHash::GENESIS)
    } else {
        match undo::choose_anchor(db, &space, verifier.as_ref()).await {
            Ok(a) => a,
            Err(e) => return Outcome::tampered(format!("space   {space}\nFAILED  {e}\n")),
        }
    };
    let mut out = format!("space   {space}\nanchor  {anchor}\n");
    if matches!(anchor, Anchor::Genesis) && verifier.is_none() {
        if let Ok(Some(cp)) = undo::latest_checkpoint(db, &space).await {
            out.push_str(&format!(
                "note    a checkpoint at position {} is stored but no verifying key was \
                 supplied, so it was not used as an anchor (--key-id/--public-key, or \
                 ORDO_CHECKPOINT_KEY_ID/ORDO_CHECKPOINT_PUBKEY)\n",
                cp.upto
            ));
        }
    }
    // **Both artifacts, always, and reported apart.** Ruled on the board
    // (`secevent`, *"Execute your suggested rollout"*): `ordo verify` walks
    // both chains and reports them separately, because a merged verdict
    // hides which artifact moved and which one moved is the whole
    // diagnosis. So neither half short-circuits the other — a broken
    // command chain must not stop the security stream being checked, since
    // the security stream is the artifact you are meant to be able to
    // believe *after* the domain record is in doubt.
    let mut failed = false;
    match undo::verify(db, &space, anchor, anchor_hash).await {
        Ok(v) => {
            match v.head {
                Some((idx, hash)) => out.push_str(&format!(
                    "VERIFIED {} group{} · head index {idx} = {hash}{}\n",
                    v.checked,
                    if v.checked == 1 { "" } else { "s" },
                    discontinuity_summary(&v.discontinuities),
                )),
                None => out.push_str("VERIFIED 0 groups · this space has no history\n"),
            }
            // A recorded restore is lawful history, so it exits 0 with the
            // rest of a green run; it is printed, never folded into the
            // summary alone, because the operator's next question is which
            // restore and from where.
            for d in &v.discontinuities {
                out.push_str(&format!("restore   {}\n", describe_marker(d)));
            }
            // An orphan is listed only when a marker vouches for it —
            // `undo::verify` fails on one that nothing attests — so the
            // markers are re-read here to name which, rather than printing
            // a position and leaving the operator to find the restore.
            if !v.orphaned_checkpoints.is_empty() {
                let markers = undo::restore_markers(db, &space).await.unwrap_or_default();
                for (upto, head) in &v.orphaned_checkpoints {
                    let by = match undo::explained_by(&markers, *upto) {
                        Some(idx) => format!("explained by restore at index {idx}"),
                        None => "unexplained".to_string(),
                    };
                    out.push_str(&format!(
                        "orphaned  checkpoint upto {upto} head {head}, {by}, kept\n"
                    ));
                }
            }
            // **The records outside this space's own tables** (R2d-iv).
            // Printed after the chain's own findings, because an operator
            // reads the chain first and then asks who else recorded a head
            // it has moved away from.
            match records_report(db, &flags, &space, &receivers).await {
                Ok((lines, bad)) => {
                    out.push_str(&lines);
                    failed |= bad;
                }
                Err(e) => {
                    out.push_str(&format!("FAILED  {e}\n"));
                    failed = true;
                }
            }
        }
        Err(e) => {
            out.push_str(&format!("FAILED  {e}\n"));
            failed = true;
        }
    }

    // The second artifact.
    let scope = ordo_secevent::Scope::Space(space.clone());
    match security_report(&security_keys, &pool, &scope).await {
        Ok((lines, bad)) => {
            out.push_str(&lines);
            failed |= bad;
        }
        Err(e) => {
            out.push_str(&format!("\nsecurity  {}\nFAILED    {e}\n", scope.key().0));
            failed = true;
        }
    }

    if failed {
        Outcome::tampered(out)
    } else {
        Outcome::ok(out)
    }
}

/// The security stream's block of a `verify` report, and whether it failed.
///
/// Its own paragraph with its own `VERIFIED`/`FAILED` line, never folded
/// into the command chain's verdict. The fields it prints are the ones that
/// change what the verdict *means*: an anchor at genesis covers the whole
/// history and an anchor at a checkpoint covers the suffix since that
/// marker, and an operator reading a green line is entitled to know which
/// claim they were handed.
async fn security_report(
    verifiers: &[CheckpointVerifier],
    pool: &Pool,
    scope: &ordo_secevent::Scope,
) -> Result<(String, bool), String> {
    let store =
        ordo_exec::secevent::PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let audit = ordo_exec::secevent::audit_stream(&store, scope, verifiers)
        .await
        .map_err(|e| e.to_string())?;

    let mut out = format!("\nsecurity  {}\n", scope.key().0);
    out.push_str(&match audit.anchored_at {
        Some(at) => format!("anchor    checkpoint at position {at}\n"),
        None => "anchor    genesis\n".to_string(),
    });
    if audit.verifiable_from > 0 {
        out.push_str(&format!(
            "retained  from position {} — nothing before it is recomputable\n",
            audit.verifiable_from
        ));
    }
    if let Some(id) = &audit.key_id {
        out.push_str(&format!("key       {id}\n"));
    }
    // **Said, not assumed.** Without a key, checks 2 and 4 of
    // `audit_stream` do not run, so a green line here is a narrower claim
    // than a green line with one — and the narrower claim is precisely the
    // one that cannot detect entries removed from the *end* of the stream.
    if verifiers.is_empty() {
        let known = match audit.latest_checkpoint {
            Some(at) => format!("a checkpoint at position {at} is stored but was not used"),
            None => "no checkpoint is stored".to_string(),
        };
        out.push_str(&format!(
            "note      no verifying key was supplied ({known}), so the entries recompute \
             against each other and nothing proves none was removed from the end \
             (--security-key-id/--security-public-key, or {}/{})\n",
            ordo_secevent::SECURITY_KEY_ID_ENV,
            ordo_secevent::SECURITY_PUBKEY_ENV,
        ));
    }

    // **A marker signed by a key that was not supplied is not a failure.**
    // It is a narrower answer, and saying so is the whole of the rotation
    // fix: after a key rotation every pre-rotation marker is signed by a key
    // a single-key run does not hold, and reporting that as tampering meant
    // the record cried wolf exactly when a deployment had rotated because it
    // suspected a compromise.
    if let Some(id) = &audit.unreadable_marker {
        out.push_str(&format!(
            "note      the newest checkpoint is signed by key `{id}`, which was not supplied, \
             so nothing proves no entry was removed from the end. Pass that key as well \
             (--security-key-id/--security-public-key repeat, and pair up in order)\n"
        ));
    }

    match (&audit.failure, audit.head) {
        (Some(why), _) => {
            out.push_str(&format!("FAILED    {why}\n"));
            Ok((out, true))
        }
        (None, Some((idx, hash))) => {
            out.push_str(&format!(
                "VERIFIED  {} entr{} · head index {idx} = {hash}\n",
                audit.checked,
                if audit.checked == 1 { "y" } else { "ies" },
            ));
            Ok((out, false))
        }
        (None, None) => {
            out.push_str("VERIFIED  0 entries · this scope has recorded nothing\n");
            Ok((out, false))
        }
    }
}

/// `ordo verify --space <spc> --objects <dir|file.ordo>` —
/// `docs/plan-blob-chain.md` §4.5, the offline form of §4.3: walk every
/// image the chain has ever stored and compare its recorded digest
/// against the object that is there, rather than recomputing hashes.
///
/// **A schema, unlike the other two modes.** Chain-hash verification never
/// asks what a column means, only whether the bytes chain — `verify` and
/// `--proof` both take no program. Object verification has to know which
/// columns are blob columns before it can know which images have anything
/// to check, and that answer lives in the schema, not in the log. So this
/// is the one `verify` mode that takes the program positionally, the way
/// `revert` already does and for the identical reason.
///
/// **Real custody, unlike `ordo revert`'s.** `revert`'s own comment below
/// explains why it still passes `None` for both handles: the operator
/// tool's custody *selection* did not exist yet, and wiring a store there
/// first would mean a revert reporting absence for every object in a
/// deployment it was never configured to see. This is the command that
/// selection was deferred to. Opening a recorded digest is a key use with
/// no substitute, so a mode whose only job is opening digests has nothing
/// to do without one — the master-key ceremony is the one `ordo keys`
/// already runs (`--master-key-file`/`--passphrase-file`, or
/// `ORDO_MASTER_KEY`; see `keys::master_from`'s own doc on why the two
/// tools read the same two sources in the same order), and the object
/// store is the same `ORDO_BLOB_STORE` contract the server boots from
/// (`ordo_host::runtime::blob_from_env`). Both are called, not
/// reimplemented: a second store-selection dialect in this one binary
/// would be exactly the drift those functions' own docs warn against.
async fn verify_objects(flags: Flags, space: SpaceId) -> Outcome {
    let Some(path) = flags.positional.first().cloned() else {
        return Outcome::usage(
            "verify --objects needs the program (a `.ordo` file or a directory): only the \
             schema says which columns are blob columns, and the log does not carry that"
                .into(),
        );
    };
    let src = match crate::Sources::read(std::path::Path::new(&path)) {
        Ok(s) => s,
        Err(e) => return Outcome::failed(e),
    };
    let (schema, _registry) = match crate::load_sources(&src) {
        Ok(pair) => pair,
        Err(e) => return Outcome::failed(e),
    };
    let master = match crate::keys::master_from(&flags) {
        Ok(m) => m,
        Err(e) => {
            return Outcome::usage(format!(
                "verify --objects opens a recorded digest under a key release, and has none: \
                 {e}"
            ))
        }
    };
    let store = match ordo_exec::keystore::PgKeyStore::open(pool_config(&flags)) {
        Ok(s) => s,
        Err(e) => return Outcome::failed(format!("$keycustody: {e}\n")),
    };
    let keys: std::sync::Arc<dyn ordo_crypto::KeyProvider> = std::sync::Arc::new(
        ordo_crypto::LocalKeyProvider::from_master(master, Box::new(store)),
    );
    let binding = match ordo_host::runtime::blob_from_env() {
        Ok(Some(b)) => b,
        Ok(None) => {
            return Outcome::usage(
                "verify --objects needs a configured object store: set ORDO_BLOB_STORE=s3 \
                 (from ORDO_BLOB_S3_*) or ORDO_BLOB_STORE=fs:<path> (plus \
                 ORDO_DEV_UNSAFE_BLOB_STORE=1 outside dev)\n"
                    .into(),
            )
        }
        Err(e) => return Outcome::usage(format!("ORDO_BLOB_STORE: {e}\n")),
    };
    // **Each row's recorded store**, as the sweep resolves it. Since 25ac
    // every file records the bucket it was written to, and a tenant that
    // has attached, changed or detached a bucket has files in several
    // stores. A `SingleStore` here answered every name with the
    // deployment's own store, so a file in a tenant's bucket was looked for
    // in the wrong place and reported absent. The provider is the one the
    // serving runtime reads (`ORDO_TENANT_CREDENTIALS`); without one, a row
    // that names a bucket is reported unverified with that reason.
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    let credentials: Option<std::sync::Arc<dyn ordo_exec::creds::BucketCredentials>> =
        match ordo_host::runtime::credentials_from_env() {
            Ok(Some(dir)) => Some(dir),
            Ok(None) => None,
            Err(e) => return Outcome::usage(format!("ORDO_TENANT_CREDENTIALS: {e}\n")),
        };
    let pipeline = ordo_blob::BlobPipeline::new(
        ordo_exec::bucket::recorded_stores(pool.clone(), binding.store, credentials),
        keys.clone(),
        ordo_blob::BlobPolicy::DEFAULT,
    );
    let db = Db::Pool(&pool);
    let mut out = format!("space   {space}\nstore   {}\n", binding.label);
    match undo::verify_objects(db, &schema, &pipeline, keys.as_ref(), &space).await {
        Ok(v) => {
            out.push_str(&format!(
                "OBJECTS {} image{} walked over the stored chain, {} unverified\n",
                v.images,
                if v.images == 1 { "" } else { "s" },
                v.unverified.len(),
            ));
            for u in &v.unverified {
                out.push_str(&format!(
                    "unverified  `{}.{}` on `{}` (group `{}`): {}\n",
                    u.entity, u.field, u.row_id, u.correlation.0, u.reason,
                ));
            }
            out.push_str("VERIFIED every recorded digest against the object that is there\n");
            Outcome::ok(out)
        }
        Err(e) => {
            out.push_str(&format!("FAILED  {e}\n"));
            Outcome::tampered(out)
        }
    }
}

/// The `witness` and `receiver` lines, and whether any record the chain has
/// moved away from is one nothing accounts for.
///
/// The reading, stated where an operator meets it: **a receiver's head the
/// chain has moved away from, with no marker attesting a pre-restore head at
/// or above it, is a restore nobody recorded**, whatever the deployment's
/// own records say about each other. The deployment cannot reach a receiver
/// to ask, and that is exactly why the receiver's copy is evidence.
async fn records_report(
    db: Db<'_>,
    flags: &Flags,
    space: &SpaceId,
    receivers: &[Receiver],
) -> Result<(String, bool), String> {
    let control = flags.named_value("--control-db");
    if control.is_none() && receivers.is_empty() {
        return Ok(("witness  not read (--control-db)\n".to_string(), false));
    }
    // A second pool only when the two names differ; otherwise the space's
    // own, which is the single-database deployment.
    let control_pool = match control {
        Some(dbname) if Some(dbname) != flags.named_value("--db") => {
            let mut cfg = pool_config(flags);
            cfg.db = dbname.to_string();
            Some(
                Pool::connect(cfg)
                    .await
                    .map_err(|e| format!("control database: {e}"))?,
            )
        }
        Some(_) => match db {
            Db::Pool(p) => Some(p.clone()),
            Db::Tx(_) => None,
        },
        None => None,
    };
    let witness = control_pool.map(Witness::new);
    let heads: Vec<(u64, ordo_commandlog::ChainHash)> =
        receivers.iter().map(|r| (r.upto, r.head)).collect();
    let verdicts = undo::classify_records(db, witness.as_ref(), space, &heads)
        .await
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    if control.is_none() {
        out.push_str("witness  not read (--control-db)\n");
    }
    let mut failed = false;
    for v in &verdicts {
        let (label, detail) = match &v.origin {
            // The stored rows are already reported as `orphaned` lines
            // above; repeating them here would be one fact twice.
            undo::RecordOrigin::Stored => continue,
            undo::RecordOrigin::Witness { .. } => ("witness ", String::new()),
            undo::RecordOrigin::Receiver { index } => {
                ("receiver", format!("{}: ", receivers[*index].label))
            }
        };
        let state = match v.agreement {
            undo::Agreement::Current => "current".to_string(),
            undo::Agreement::BelowFloor => "below the floor".to_string(),
            undo::Agreement::Orphaned => match v.explained_by {
                Some(by) => format!("orphaned, explained by {by}"),
                None => "orphaned, UNEXPLAINED".to_string(),
            },
        };
        if matches!(v.agreement, undo::Agreement::Current) {
            // A record the chain agrees with is the ordinary case and says
            // nothing an operator has to act on. Only the receivers are
            // listed unconditionally, because the operator supplied those
            // files and expects a line per file.
            if matches!(v.origin, undo::RecordOrigin::Witness { .. }) {
                continue;
            }
        }
        out.push_str(&format!(
            "{label} {detail}checkpoint upto {} head {} {state}\n",
            v.upto, v.head
        ));
        if v.agreement == undo::Agreement::Orphaned && v.explained_by.is_none() {
            failed = true;
            out.push_str(&format!(
                "FAILED  the chain no longer reaches a head that {} recorded: upto {} head {}\n",
                v.origin, v.upto, v.head
            ));
        }
    }
    Ok((out, failed))
}

/// One receiver's `ordo.attestation/1` document, read from a file.
struct Receiver {
    label: String,
    upto: u64,
    head: ordo_commandlog::ChainHash,
}

/// Parse every `--attestation <file>`.
///
/// Checked through `ordo_verify_proof::verify_attestation`, the same routine
/// a third party runs, so this command cannot disagree with the verifier a
/// receiver was pointed at. Without a key it is structure only and the line
/// says the word. A document naming another space is a usage error: it
/// answers a question about a different chain, and comparing it against this
/// one would be an answer nobody asked for.
fn receivers_from(flags: &Flags, space: &SpaceId) -> Result<Vec<Receiver>, Outcome> {
    let key = match verifier(flags) {
        Ok(v) => v,
        Err(e) => return Err(Outcome::usage(e)),
    };
    let mut out = Vec::new();
    for path in flags.all_values("--attestation") {
        let doc = std::fs::read_to_string(path)
            .map_err(|e| Outcome::failed(format!("could not read `{path}`: {e}")))?;
        let verdict = ordo_verify_proof::verify_attestation(&doc, key.as_ref())
            .map_err(|e| Outcome::usage(format!("`{path}`: {e}")))?;
        if verdict.space != space.0 {
            return Err(Outcome::usage(format!(
                "`{path}` is an attestation for space `{}`, not `{space}`",
                verdict.space
            )));
        }
        let head =
            ordo_commandlog::ChainHash(hex32(&verdict.head).map_err(|e| {
                Outcome::usage(format!("`{path}`: the head is not a chain hash: {e}"))
            })?);
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        let label = if verdict.structure_only {
            format!("{name} (unverified)")
        } else {
            name
        };
        out.push(Receiver {
            label,
            upto: verdict.upto,
            head,
        });
    }
    Ok(out)
}

/// ` · 1 restore discontinuity at index N`, or nothing when the walk passed
/// no marker.
fn discontinuity_summary(ds: &[undo::Discontinuity]) -> String {
    match ds {
        [] => String::new(),
        [d] => format!(" · 1 restore discontinuity at index {}", d.idx),
        many => format!(
            " · {} restore discontinuities at indices {}",
            many.len(),
            many.iter()
                .map(|d| d.idx.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// One marker, as the operator reads it: where it sits, where history
/// resumed, what it was restored from, who ran it, and what the chain
/// said before (or that nobody could say).
fn describe_marker(d: &undo::Discontinuity) -> String {
    let m = &d.marker;
    let source = match &m.source {
        RestoreSource::Pitr { at } => format!("pitr at {}", at.0),
        RestoreSource::Bundle { manifest_sha256 } => format!(
            "bundle {}",
            manifest_sha256
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ),
    };
    let before = match m.pre_restore_head {
        Some((idx, hash)) => format!("pre-restore head {idx} = {hash}"),
        None => "pre-restore head unattested".to_string(),
    };
    format!(
        "index {} · restored from {} = {} · {source} · operator {} · {before}",
        d.idx, m.restored_from.0, m.restored_from.1, m.operator
    )
}

// ── checkpoint ───────────────────────────────────────────────────────────

async fn checkpoint(flags: Flags) -> Outcome {
    // **`--security` selects the artifact; it does not add one.** `verify`
    // walks both chains in one invocation because reading is free and a
    // merged verdict is the only thing that would be wrong there. Signing is
    // an act under a specific key, and the two keys are deliberately held by
    // different people for different reasons (`ordo_secevent::store`'s three
    // arguments). An operator whose environment holds only the security key
    // must be able to sign only the security stream, and one signing the
    // command chain must not silently produce a second signature under a key
    // they may not even have meant to have in scope.
    if flags.has("--security") {
        return security_checkpoint(flags).await;
    }
    if let Err(e) = flags.only(&["--space", "--db", "--control-db"]) {
        return Outcome::usage(e);
    }
    let space = match flags.require("--space") {
        Ok(s) => SpaceId(s.to_string()),
        Err(e) => return Outcome::usage(e),
    };
    // The signing key comes from the deployment, and unset is a refusal
    // rather than a default — a default checkpoint key means every
    // deployment can sign every other's history.
    let signer = match Ed25519Signer::from_env() {
        Ok(s) => s,
        Err(e) => return Outcome::failed(e.to_string()),
    };
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    let witness = match witness_for(&flags, &pool).await {
        Ok(w) => w,
        Err(o) => return o,
    };
    match undo::sign_checkpoint(&pool, &space, &signer, now(), &witness).await {
        Ok(Checkpointed::Signed(cp)) => Outcome::ok(format!(
            "space   {space}\nSIGNED  checkpoint upto {} · head {} · at {}\nkey     {}\n\
             verify  publish this to receivers: {}\n",
            cp.upto,
            cp.head,
            cp.at.0,
            cp.key_id,
            signer.verifier().public_hex(),
        )),
        Ok(Checkpointed::AlreadyAt(upto)) => Outcome::ok(format!(
            "space   {space}\nUNCHANGED  the head is already checkpointed at {upto}\n"
        )),
        Err(e) => Outcome::failed(format!("space   {space}\nREFUSED {e}\n")),
    }
}

/// `ordo checkpoint --security [--space <spc> | --deployment]` — sign a
/// marker at the head of a **security stream**.
///
/// # What this command is for, given that a worker does it on cadence
///
/// The same thing `ordo checkpoint` is for, given that
/// `commandlog.checkpoint` exists: a deployment whose supervisor is not
/// running, an operator who wants a marker *now* because they are about to
/// take a backup or hand a stream to an auditor, and — the one that matters
/// most here — a deployment that has not set
/// `ORDO_SECURITY_CHECKPOINT_KEY` in its server environment and holds the
/// key somewhere an operator reaches instead. That last case is not
/// hypothetical: the argument for the second key is that it is held by
/// *whoever answers for the deployment*, and a key held only by the process
/// it is meant to keep honest is a weaker version of the guarantee.
///
/// # It refuses far more than it signs, and that is the design
///
/// Everything this prints beyond `SIGNED` comes from
/// `ordo_exec::secevent::checkpoint_scope`, which verifies the stream before
/// it signs and names every state in which a signature would make matters
/// worse. The most important of those is an **already truncated** stream:
/// countersigning a shortened history is the one thing an operator could do
/// here that a verifier could not later undo.
async fn security_checkpoint(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--space", "--db", "--security", "--deployment"]) {
        return Outcome::usage(e);
    }
    // The scope, on `verify --deployment`'s rules and with its wording: the
    // deployment's stream belongs to no space, so `--space` does not name it
    // and passing both is a usage error rather than a quiet choice between
    // two different streams.
    let scope = match (flags.has("--deployment"), flags.named_value("--space")) {
        (true, Some(_)) => {
            return Outcome::usage(
                "`--deployment` signs the deployment's own security stream, which belongs to \
                 no space, so `--space` does not apply to it"
                    .to_string(),
            )
        }
        (true, None) => ordo_secevent::Scope::Deployment,
        (false, Some(s)) => ordo_secevent::Scope::Space(SpaceId(s.to_string())),
        (false, None) => {
            return Outcome::usage(
                "`ordo checkpoint --security` needs `--space <spc>` or `--deployment`".to_string(),
            )
        }
    };
    // **A signer that cannot reach its key is an error, never a skip.** The
    // ruling is `Ed25519Signer::from_env`'s and it is restated here because
    // the two halves of this command reach the same conclusion by different
    // routes. For `ordo checkpoint`, unset is a refusal because "a default
    // checkpoint key means every deployment can sign every other's history".
    // For an operator invocation it is also simply what was asked: this
    // command's entire purpose is to sign, so a run that signed nothing and
    // exited 0 would report success for the one thing it was told to do. The
    // scheduled half makes the opposite call for the opposite reason — see
    // `SecurityDrain::start_routed`, where a missing key disables the pass
    // and says so on `/health/deep`, because a background loop that failed
    // every tick forever would be an alert nobody could clear.
    let signer = match ordo_secevent::security_signer_from_env() {
        Ok(s) => s,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    let store =
        ordo_exec::secevent::PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let key = scope.key().0.clone();
    match ordo_exec::secevent::checkpoint_scope(&store, &scope, &signer, now()).await {
        Ok(StreamCheckpointed::Signed(cp)) => Outcome::ok(format!(
            "security  {key}\nSIGNED    checkpoint upto {} · head {} · at {}\nkey       {}\n\
             verify    give this to whoever verifies: {}\n",
            cp.upto,
            cp.head,
            cp.at.0,
            cp.key_id,
            signer.verifier().public_hex(),
        )),
        Ok(StreamCheckpointed::AlreadyAt(upto)) => Outcome::ok(format!(
            "security  {key}\nUNCHANGED the head is already checkpointed at {upto}\n"
        )),
        // Exit 0. An empty stream is not a fault and it is not a refusal:
        // `ordo_secevent::checkpoint` returns `None` for one because a
        // checkpoint over nothing anchors nothing, and a scope that has
        // recorded no decisions is the ordinary state of a healthy space.
        Ok(StreamCheckpointed::EmptyStream) => Outcome::ok(format!(
            "security  {key}\nUNCHANGED this scope has recorded nothing, and a checkpoint over \
             nothing anchors nothing\n"
        )),
        // Exit 3, the verification-failed code, and not 1. Every refusal
        // `checkpoint_scope` returns is a statement that this stream is in a
        // state `ordo verify` would also fail it for — a tamper, a truncation,
        // an unanchored tail, a marker nothing published signed. The operator
        // who typed a signing command has just been handed a verification
        // finding, and the exit code has to be the one their scripts already
        // watch for that.
        Ok(StreamCheckpointed::Refused(why)) => {
            Outcome::tampered(format!("security  {key}\nREFUSED   {why}\n"))
        }
        Err(e) => Outcome::failed(format!("security  {key}\nFAILED    {e}\n")),
    }
}

// ── restore ──────────────────────────────────────────────────────────────

/// `ordo restore --record` — write the restore marker for a restore that
/// has already happened.
///
/// # This tool attributes, it does not authenticate
///
/// There is no step-up on this command and there cannot be one. A step-up
/// is a door ceremony, and the boot assertion refuses to serve the space
/// until the marker exists, so the door is closed at exactly the moment the
/// marker is needed. The operator is named by `--as` and the name is
/// asserted, the same posture `ordo keys init --as` takes and states. What
/// joins the marker to an authenticated record is the correlation it shares
/// with the `custody-action` in the security-event stream.
///
/// # The source is never unknown
///
/// A restore from an unidentifiable source is refused (exit 2). `--at` for
/// a point-in-time restore, in **seconds**, the unit the chain uses
/// everywhere; `--digest` for a bundle, which is the manifest's own SHA-256
/// that `std.space.export` reports.
///
/// # `--tenant`: named by id, resolved through `$spacedb` (18d R-O1)
///
/// Without it, this command dials the space's database from `--db` and the
/// witness from `--control-db`, trusting the operator to point each at the
/// right place — fine for one database, and with N targets it is two
/// independent chances to name the wrong host, with the failure being a
/// chain compared against the wrong witness. `--tenant` takes the control
/// connection instead (`--control-db`, or `--db`'s own default) and a
/// tenant id (`--space`, which is what the id already is: see
/// `ordo_host::tenants`), resolves the tenant's database through
/// `$spacedb`, and prints which database it resolved. `--db` is refused
/// alongside it: two ways to name where the rows are is the exact hazard
/// this flag exists to remove, and guessing which one the operator meant
/// is worse than refusing. The manual form (no `--tenant`) is unchanged and
/// stays as break-glass, for a host `$spacedb` cannot resolve or an
/// operator who does not trust what is stored.
async fn restore(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&[
        "--record",
        "--space",
        "--db",
        "--control-db",
        "--tenant",
        "--as",
        "--source",
        "--at",
        "--digest",
        "--pre-restore-head",
        "--restored-from",
        "--unattested",
        "--again",
    ]) {
        return Outcome::usage(e);
    }
    if !flags.has("--record") {
        return Outcome::usage(
            "ordo restore --record --space <spc> --as <who> \
             (--source pitr --at <secs> | --source bundle --digest <hex>) \
             [--tenant]\n"
                .into(),
        );
    }
    if flags.has("--tenant") && flags.named.contains_key("--db") {
        return Outcome::usage(
            "`--tenant` resolves the database through `$spacedb`; `--db` names it \
             directly. Both together are two chances to name the wrong one — drop \
             one.\n"
                .into(),
        );
    }
    let space = match flags.require("--space") {
        Ok(s) => SpaceId(s.to_string()),
        Err(e) => return Outcome::usage(e),
    };
    let operator = match flags.require("--as") {
        Ok(s) => s.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let source = match restore_source(&flags) {
        Ok(s) => s,
        Err(e) => return Outcome::usage(e),
    };
    let pre_restore_head = match pre_restore_head(&flags) {
        Ok(h) => h,
        Err(e) => return Outcome::usage(e),
    };

    let (pool, witness, resolved) = match restore_target(&flags, &space).await {
        Ok(t) => t,
        Err(o) => return o,
    };
    let db = Db::Pool(&pool);
    let before = undo::continuity(db, &witness, &space)
        .await
        .map(|c| c.to_string())
        .unwrap_or_else(|e| format!("unreadable ({e})"));

    let restored_from = match flags.named_value("--restored-from") {
        None => None,
        Some(v) => match v.parse::<u64>() {
            Ok(i) => Some(i),
            Err(_) => return Outcome::usage(format!("`--restored-from` is an index, not `{v}`")),
        },
    };
    let rec = undo::RestoreRecord {
        operator,
        source,
        pre_restore_head,
        again: flags.has("--again"),
        restored_from,
    };
    match undo::record_restore(&pool, &witness, &space, rec).await {
        Ok(r) => {
            let after = undo::continuity(db, &witness, &space)
                .await
                .map(|c| c.to_string())
                .unwrap_or_else(|e| format!("unreadable ({e})"));
            let mut out = String::new();
            if let Some(label) = &resolved {
                out.push_str(&format!("database {label}\n"));
            }
            out.push_str(&format!(
                "space   {space}\ncontinuity before  {before}\n\
                 RECORDED marker at index {} · restored from ({}, {}) · hash {}\n",
                r.idx, r.restored_from.0, r.restored_from.1, r.hash
            ));
            if let Some(from) = r.defaulted {
                out.push_str(&format!("pre-restore head defaulted from {from}\n"));
            }
            if r.gap > 0 {
                out.push_str(&format!(
                    "gap     {} write{} landed between the restore and this record; \
                     `restored from` is where history broke and the marker's `prev` is \
                     where it was written\n",
                    r.gap,
                    if r.gap == 1 { "" } else { "s" }
                ));
            }
            out.push_str(&format!("audit   {}\n", r.audit));
            out.push_str(&format!(
                "witness {}\n",
                if r.witnessed {
                    "recorded in the control database"
                } else {
                    "NOT WITNESSED — run this again to complete the record"
                }
            ));
            out.push_str(&format!(
                "attest  {}\n",
                if r.cursor_reset {
                    "the sink's cursor now reads the position history resumed at; every head \
                     the chain still agrees with is delivered by (position, head), so a reused \
                     position is sent again"
                } else {
                    "no attestation sink for this space, so nothing to re-point"
                }
            ));
            out.push_str(&format!("continuity after   {after}\n"));
            if r.witnessed {
                Outcome::ok(out)
            } else {
                Outcome::failed(out)
            }
        }
        Err(e) => {
            let prefix = resolved
                .as_deref()
                .map(|label| format!("database {label}\n"))
                .unwrap_or_default();
            Outcome::failed(format!("{prefix}space   {space}\nREFUSED {e}\n"))
        }
    }
}

/// The space's own pool and the control witness, resolved one of two ways
/// (18d R-O1's own doc, above `restore`, argues the choice).
///
/// Returns the database this call actually used, for the report — `None`
/// in the manual form, which already knows what it typed and gains
/// nothing from being told back.
async fn restore_target(
    flags: &Flags,
    space: &SpaceId,
) -> Result<(Pool, Witness, Option<String>), Outcome> {
    if !flags.has("--tenant") {
        let pool = connect(flags).await?;
        let witness = witness_for(flags, &pool).await?;
        return Ok((pool, witness, None));
    }
    // The control connection: `--control-db`, or `--db`'s own default —
    // the same fallback `witness_for` already uses for the single-database
    // case, and correct here for the identical reason. `--db` itself is
    // refused alongside `--tenant` above, so this can only read the
    // default or an explicit `--control-db`.
    let mut control_cfg = pool_config(flags);
    if let Some(db) = flags.named.get("--control-db") {
        control_cfg.db = db.clone();
    }
    let control = Pool::connect(control_cfg)
        .await
        .map_err(|e| Outcome::failed(format!("database: {e}")))?;
    // `$spacedb` is not part of `ensure_platform_tables` (it is written by
    // exactly one production caller, `prepare_target`, and read by the
    // resolver and by this command) — an operator dialling a control
    // database from before 18d shipped anywhere near it must still get a
    // clean "unattached" rather than a table-does-not-exist refusal.
    ordo_exec::spacedb::ensure_table(&control)
        .await
        .map_err(|e| Outcome::failed(format!("database: {e}")))?;
    // `spacedb` keys on `ordo_crypto::SpaceId`; this module's `SpaceId` is
    // `ordo_commandlog`'s, the chain's own type. Both are a bare string
    // newtype over the identical id, so this is a rewrap, not a lookup.
    let lookup_key = ordo_crypto::custody::SpaceId(space.0.clone());
    match ordo_exec::spacedb::lookup(&control, &lookup_key)
        .await
        .map_err(|e| Outcome::failed(format!("database: {e}")))?
    {
        // Unattached: this tenant's rows are on the database this call is
        // already holding open, exactly as every space with no `$spacedb`
        // row is served from the deployment's default.
        None => {
            let witness = Witness::new(control.clone());
            Ok((control, witness, Some("default (unattached)".to_string())))
        }
        Some(a) => {
            let credentials = ordo_host::runtime::credentials_from_env()
                .map_err(|e| Outcome::failed(format!("{e}")))?
                .ok_or_else(|| {
                    Outcome::failed(
                        "this tenant is attached to its own database, and no \
                         ORDO_TENANT_CREDENTIALS provider is configured on this node to \
                         dial it\n"
                            .to_string(),
                    )
                })?;
            let label = ordo_exec::route::target_label(&a.host, a.port, &a.db);
            let template = pool_config(flags);
            // `$deployment` is the password this command already holds for
            // the control database, so a tenant on the deployment's own
            // cluster needs no second copy in the directory.
            let credentials = ordo_exec::route::WithOwnCredential::wrapping(
                credentials,
                template.password.clone(),
            );
            let cfg = ordo_exec::route::target_config(&a, credentials.as_ref(), &template)
                .map_err(|e| Outcome::failed(format!("database ({label}): {e}")))?;
            let pool = Pool::connect(cfg)
                .await
                .map_err(|e| Outcome::failed(format!("database ({label}): {e}")))?;
            // The witness is always the control database (R-O2: the
            // witness is never restored, and it is what makes a restore
            // checkable), never the resolved target — a tenant on its own
            // perimeter has no witness of its own to be.
            let witness = Witness::new(control);
            Ok((pool, witness, Some(label)))
        }
    }
}

/// `--source pitr --at <secs>` or `--source bundle --digest <hex>`.
fn restore_source(f: &Flags) -> Result<RestoreSource, String> {
    match f.named_value("--source") {
        Some("pitr") => {
            let at = f
                .named_value("--at")
                .ok_or("`--source pitr` needs `--at <seconds>`")?;
            at.parse()
                .map(|s| RestoreSource::Pitr { at: Timestamp(s) })
                .map_err(|_| format!("`--at` is seconds since the epoch, not `{at}`"))
        }
        Some("bundle") => {
            let hex = f
                .named_value("--digest")
                .ok_or("`--source bundle` needs `--digest <hex>`")?;
            hex32(hex)
                .map(|manifest_sha256| RestoreSource::Bundle { manifest_sha256 })
                .map_err(|e| format!("`--digest`: {e}"))
        }
        Some(other) => Err(format!("`--source` is `pitr` or `bundle`, not `{other}`")),
        None => {
            Err("`--source` is required: a restore from an unidentifiable source is refused".into())
        }
    }
}

/// `--pre-restore-head <idx>:<hex>`, or `--unattested`, or nothing, which
/// lets `record_restore` take its default.
fn pre_restore_head(f: &Flags) -> Result<Option<(u64, ordo_commandlog::ChainHash)>, String> {
    if f.has("--unattested") {
        if f.named_value("--pre-restore-head").is_some() {
            return Err("`--unattested` and `--pre-restore-head` say different things".into());
        }
        return Ok(None);
    }
    let Some(v) = f.named_value("--pre-restore-head") else {
        return Ok(None);
    };
    let (idx, hex) = v
        .split_once(':')
        .ok_or("`--pre-restore-head` is `<idx>:<hex>`")?;
    let idx: u64 = idx
        .parse()
        .map_err(|_| format!("`--pre-restore-head`: `{idx}` is not an index"))?;
    Ok(Some((
        idx,
        ordo_commandlog::ChainHash(hex32(hex).map_err(|e| format!("`--pre-restore-head`: {e}"))?),
    )))
}

fn hex32(s: &str) -> Result<[u8; 32], String> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("expected 64 lowercase hex characters, got `{s}`"));
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
            .map_err(|_| "not hexadecimal".to_string())?;
    }
    Ok(out)
}

// ── prune ────────────────────────────────────────────────────────────────

/// `ordo prune` — cut a space's chain back to its floor, or show where the
/// floor is and what is holding it.
///
/// # `--dry-run` is not a courtesy, it is the normal reading
///
/// This is the only command in the tool that **destroys audit evidence**, so
/// the useful mode is the one that answers "where would you cut, and who is
/// stopping you". `ordo_exec::prune::floor` computes that without touching
/// anything, and the report names the holder — an operator told only that
/// the log is not shrinking cannot act, and the report that names
/// `routine:digest` at position 4,102 is the one that gets fixed.
///
/// # The verifying key is required, and there is no default
///
/// A prune must verify the checkpoint it is cutting to and the prefix it is
/// about to remove. Both need the key, so an unset key is a refusal rather
/// than a prune that skips the checks — which would be a prune with its two
/// safety properties silently off. `--dry-run` does not need it, and does
/// not ask for it.
///
/// # The policy comes from `LogPolicy::DEFAULT`, and that is a stated gap
///
/// `policy commandlog { … }` is a declaration form and parsing it out of
/// `.ordo` source is not built. Until it is, this command uses the
/// specification's own defaults and prints them, so an operator can see the
/// horizon that produced the floor rather than assume theirs was read.
async fn prune(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--space", "--db", "--key-id", "--public-key", "--dry-run"]) {
        return Outcome::usage(e);
    }
    let space = match flags.require("--space") {
        Ok(s) => SpaceId(s.to_string()),
        Err(e) => return Outcome::usage(e),
    };
    let verifier = match verifier(&flags) {
        Ok(v) => v,
        Err(e) => return Outcome::usage(e),
    };
    let policy = ordo_exec::cadence::LogPolicy::DEFAULT;
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    if let Err(e) = ordo_exec::prune::ensure_tables(&pool).await {
        return Outcome::failed(format!("consumer table: {e}\n"));
    }

    if flags.has("--dry-run") {
        return match ordo_exec::prune::floor(&pool, &space, &policy, now().0).await {
            Ok(f) => Outcome::ok(floor_report(&f, &policy)),
            Err(e) => Outcome::failed(format!("space   {space}\ndatabase: {e}\n")),
        };
    }
    let Some(verifier) = verifier else {
        return Outcome::usage(
            "`--key-id` and `--public-key` are required to prune: the cut point's signature \
             and the prefix about to be destroyed are both checked, and a prune that skipped \
             them would be a prune with both of its safety properties off. \
             `--dry-run` needs no key."
                .into(),
        );
    };
    match ordo_exec::prune::prune(&pool, &space, &verifier, &policy, now().0).await {
        Ok(ordo_exec::prune::Pruned::Removed {
            floor,
            entries,
            payloads,
            verifiable_from,
        }) => Outcome::ok(format!(
            "{}PRUNED  {entries} entr{} and {payloads} sealed payload{} below position {}\n\
             clients recomputation now starts at {verifiable_from} \
             (served as `x-ordo-verifiable-from`)\n",
            floor_report(&floor, &policy),
            if entries == 1 { "y" } else { "ies" },
            if payloads == 1 { "" } else { "s" },
            floor.position,
        )),
        Ok(ordo_exec::prune::Pruned::NothingToPrune { floor, reason }) => Outcome::ok(format!(
            "{}UNCHANGED  {reason}\n",
            floor_report(&floor, &policy)
        )),
        Err(e) => Outcome::failed(format!("space   {space}\nREFUSED {e}\n")),
    }
}

/// The floor, its holder, and every mirror that stopped holding it.
///
/// Evictions are printed whether or not anything was pruned, because an
/// eviction is an operator alert in its own right: a mirror that stalled
/// past `consumerStall` stopped protecting its own cursor, and the operator
/// finds that out here or not at all.
fn floor_report(f: &ordo_exec::prune::Floor, policy: &ordo_exec::cadence::LogPolicy) -> String {
    let mut out = format!(
        "space   {}\npolicy  undoHorizon {}d · consumerStall {}d (defaults: `policy commandlog` \
         is not yet parsed from source)\nretained from {}\nhorizon at position {}\n\
         bound   {} — held by {}\n",
        f.space,
        policy.undo_horizon.as_secs() / 86_400,
        policy.consumer_stall.as_secs() / 86_400,
        f.first_stored,
        f.horizon_position,
        f.live_bound,
        f.holder,
    );
    match &f.anchor {
        Some(cp) => out.push_str(&format!(
            "floor   {} — the checkpoint at or below the bound, signed by {}\n",
            cp.upto, cp.key_id
        )),
        None => out.push_str(
            "floor   none — no checkpoint sits at or below the bound, and a checkpoint is the \
             only legal cut point\n",
        ),
    }
    for e in &f.evicted {
        out.push_str(&format!(
            "EVICTED mirror `{}` at position {} — idle {}s, past consumerStall. It no longer \
             holds the floor; re-registering it will not recover what is pruned.\n",
            e.name, e.position, e.idle_for
        ));
    }
    out
}

// ── apply / rollback / deploys ───────────────────────────────────────────

/// `ordo apply` — compile a program, store its artifact, point the
/// deployment at it, and nudge every replica.
///
/// # Why the compile gate is here and not in the registry
///
/// `ordo plan`'s document **records** an op that does not compile
/// (`error: {code, message}`) rather than failing, so `ordo check` can show
/// an author all fifty-seven ops instead of the first broken one. This is
/// the reader that turns that record back into a refusal: a registry row is
/// a promise that a node can load it, and an artifact carrying an op that
/// does not compile is a promise nobody can keep. The refusal names every
/// broken op, for the same reason `ordo check` does.
///
/// # The recorded error is not by itself the gate, and cannot be
///
/// Taking a non-null `error` as the refusal — which is what the roadmap's
/// sentence says, and what this command did in its first draft — refuses
/// every program with a caller-scoped read. `ordo plan` describes an op
/// through [`ordo_sema::bind_op_generic`], which binds with **no caller**,
/// and `$caller` in a *value* position (`tasks(assignee == $caller)`, the
/// shipped fixture's `myAssignedTasks`) is looked up in the parameter
/// environment rather than filled from the guard slot. So it records
/// `unbound_required_param`, and the op works perfectly: the load gate
/// binds the same op with a probe caller and verifies its shape, and the
/// request path binds it with the real one.
///
/// The disagreement is a defect in the describer, not in the program, and
/// the honest fix is upstream — `$caller` should be a **bind slot in the
/// artifact the way the space already is** (`ordo_plan::Space::Slot`, `$1`),
/// at which point such an op has publishable SQL and no recorded error.
/// That is an artifact-format change: it adds a slot to `bind_order` and
/// moves every op's version hash, which kills every outstanding capability
/// grant, so it is its own change with its own consent story.
///
/// Until then this command asks the second question rather than pretending
/// the first is sufficient: **does the op bind when a caller is supplied?**
/// If it does, the record is a describer limitation and is reported, not
/// refused. If it does not — the fixture's `giftOrders` filters on a sealed
/// field with no blind index — it is an author error and the apply stops.
/// Refusing on the weaker signal would be a gate that fires on correct
/// programs, which is worse than the gap it closes.
///
/// # What apply does to the schema, and what it does not
///
/// Three states, read by [`ordo_host::schema_apply::schema_state`] before
/// the artifact is stored:
///
/// - **Empty** — none of the program's tables exist and nothing is
///   serving. The schema is created whole through
///   [`ordo_host::schema_apply::create_schema`], the one DDL path the
///   platform already had for a fresh database. Before 2026-09-20 this
///   command computed the DDL only to hash it into the version, stored it
///   as text, and a first deployment meant pasting it into psql.
/// - **Matches** — the live schema holds everything this program asserts.
///   Store and activate, as always.
/// - **Behind** — the live schema lacks something the artifact asserts, so
///   the load gate would refuse it. The artifact is stored (the staged case
///   `--no-activate` exists for) and activation is **refused**, naming
///   `ordo migrate --apply` as the command that moves the schema. Refusing
///   here rather than at the node is the difference between an operator
///   reading the reason and a fleet logging it.
///
/// It does not run a migration. That is `ordo migrate --apply`'s job, and
/// `migration.md` owns the sequence: three deploys, not a side effect of
/// one. Drops are the other way round (artifact first, then the DDL) and
/// the gate tolerates them, so a program that only drops is a match.
async fn apply(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&[
        "--db",
        "--as",
        "--no-activate",
        "--no-wait",
        "--wait-secs",
        "--host",
        "--token",
        "--space",
    ]) {
        return Outcome::usage(e);
    }
    // How long to wait for the nodes to serve what this activates
    // (`deploy_wait`). Read before anything is applied, so a bad value
    // refuses the command rather than a deploy half reported.
    let wait = match crate::deploy_wait::from_flags(&flags) {
        Ok(w) => w,
        Err(e) => return Outcome::usage(format!("{e}\n")),
    };
    let Some(path) = flags.positional.first().cloned() else {
        return Outcome::usage("apply needs a program (a `.ordo` file or a directory)".into());
    };
    let src = match crate::Sources::read(std::path::Path::new(&path)) {
        Ok(s) => s,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    // The database this deployment uses: `--db`, else the one the
    // environment names. The refusal prints it in the command it suggests.
    let db = pool_config(&flags).db;
    let activate = !flags.has("--no-activate");

    // **17u: the endpoint, when one is named.** The whole difference is
    // which side of the wire compiles and applies — the rendering below is
    // the same function over the same value, so an operator reads one
    // report whichever mode they ran in.
    if let Some(host) = crate::wire::requested(&flags) {
        if flags.named.contains_key("--as") {
            // `--as` is an ASSERTION, and this door does not take one. The
            // actor recorded over the wire is the principal the token
            // proved. Accepting the flag and ignoring it would put a string
            // in an operator's command line that does not reach the audit
            // record it looks like it is writing.
            return Outcome::usage(
                "`--as` is an assertion, and `--host` does not take one: over the door the \
                 actor recorded in `$migrations` is the principal the bearer token proved.\n\
                 Drop `--as`, or drop `--host` and apply against the database.\n"
                    .into(),
            );
        }
        let host = match host {
            Ok(h) => h,
            Err(e) => return Outcome::usage(e),
        };
        return crate::wire::apply_over_the_wire(
            &host,
            &flags,
            src.text(),
            activate,
            &path,
            &db,
            wait,
        )
        .await;
    }

    let actor = flags.named.get("--as").cloned().unwrap_or_else(|| {
        // Attribution, never authentication — the same rule `revert --as`
        // states. An unset one is recorded as such rather than guessed from
        // a shell variable an audit reader cannot check.
        "operator:unattributed".to_string()
    });

    let (schema, registry) = match crate::load_sources(&src) {
        Ok(v) => v,
        Err(e) => return Outcome::failed(format!("{e}\n")),
    };
    let doc = match ordo_plan::artifact::program_artifact(&schema, &registry) {
        Ok(d) => d,
        Err(e) => return Outcome::failed(format!("artifact: {e}\n")),
    };
    let (broken, notes) = ordo_host::apply_op::triage_errors(&doc, &schema, &registry);
    if !broken.is_empty() {
        return Outcome::failed(format!(
            "REFUSED  {}\n",
            ordo_host::apply_op::ApplyRefused::Broken { ops: broken }
        ));
    }
    let ddl = match ordo_schema::ddl_for(&schema) {
        Ok(d) => d,
        Err(e) => return Outcome::failed(format!("ddl: {e}\n")),
    };
    let version = ordo_plan::artifact::program_version(&doc, &ddl);
    let program = ordo_host::schema_apply::Program {
        schema,
        doc,
        ddl,
        version,
        source: src.text().to_string(),
    };

    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    // The one body, called. `run_program` is `ordo_host::apply_op`'s, which
    // is the same function `std.deployment.apply` calls — see that module's
    // doc for why the sequence had to move out of this command before there
    // could be a door in front of it.
    // **One database, because that is what `--db` means.** The operator
    // named a connection string; applying to whatever that database's
    // `$spacedb` happens to list would reach targets they did not name,
    // from a tool whose whole contract is "this database". The door path
    // (`std.deployment.apply`) carries the deployment's real resolver and
    // walks every target there, which is where T7's fleet walk belongs:
    // that caller is the deployment acting on itself.
    let pools: std::sync::Arc<dyn ordo_exec::route::PoolResolver> =
        std::sync::Arc::new(ordo_exec::route::SinglePool::new(pool.clone()));
    match ordo_host::apply_op::run_program(
        &pool,
        pools.as_ref(),
        &program,
        notes,
        &actor,
        "ordo apply",
        activate,
    )
    .await
    {
        Ok(report) => {
            let out = render_apply(&report, &path, &db);
            match (wait, served_version(&report)) {
                (Some(limit), Some(version)) => {
                    match crate::deploy_wait::until_served(&version, limit, || {
                        crate::deploy_wait::look_in_database(&pool, &version)
                    })
                    .await
                    {
                        Ok(lines) => Outcome::ok(out + &lines),
                        Err(lines) => Outcome::failed(out + &lines),
                    }
                }
                _ => Outcome::ok(out),
            }
        }
        Err(refusal) => Outcome::failed(render_refusal(&refusal, &path, &db)),
    }
}

/// The version an apply made active, which is the one to wait for. `None`
/// for a staged apply, which changes nothing any node serves.
pub(crate) fn served_version(report: &ordo_host::apply_op::Report) -> Option<String> {
    use ordo_host::apply_op::Activation;
    match &report.activation {
        Activation::Switched { to, .. } => Some(to.clone()),
        Activation::AlreadyActive { version } => Some(version.clone()),
        Activation::Staged => None,
    }
}

/// The apply's report, as an operator reads it.
///
/// **The one renderer**, shared by the direct path and by `--host`, and
/// that is the mechanism behind 17u's promise that the two print the same
/// thing: they print the same `Report`. Two format strings kept in step by
/// hand would drift on the first refusal somebody added to one of them.
///
/// `path` and `db` are the caller's own facts rather than the report's —
/// the server does not know what the operator called the file or the
/// database — so the remedies that name a command are built here.
pub(crate) fn render_apply(report: &ordo_host::apply_op::Report, path: &str, db: &str) -> String {
    use ordo_host::apply_op::{Activation, SchemaOutcome, Stored};

    let mut out = String::new();
    for line in &report.notes {
        out.push_str(&format!("note       {line}\n"));
    }
    let mut behind: &[(String, String)] = &[];
    match &report.schema {
        SchemaOutcome::Created {
            tables,
            migration,
            event,
        } => out.push_str(&format!(
            "schema     created  ({tables} table(s) on an empty target; `$migrations` \
             #{migration}; {event})\n"
        )),
        SchemaOutcome::Migrated {
            changes,
            migration,
            event,
        } => {
            out.push_str(&format!(
                "schema     migrated  {} safe change(s) applied for you (`$migrations` \
                 #{migration}; {event}); turn this off with {}=off\n",
                changes.len(),
                ordo_host::schema_apply::SAFE_CHANGES_SETTING
            ));
            for (code, change) in changes {
                out.push_str(&format!("           {code:<3} {change}\n"));
            }
        }
        SchemaOutcome::Matches { pending: 0 } => out.push_str("schema     matches\n"),
        SchemaOutcome::Matches { pending } => out.push_str(&format!(
            "schema     matches  ({pending} drop(s) still live; the gate tolerates extra \
             objects, increment 5 contracts them)\n"
        )),
        SchemaOutcome::Behind { changes } => behind = changes,
    }
    out.push_str(&match report.stored {
        Stored::Written => format!("stored     {}\n", report.version),
        Stored::AlreadyPresent => format!(
            "stored     {}  (already present, byte-identical)\n",
            report.version
        ),
    });
    if !behind.is_empty() {
        out.push_str(&format!(
            "schema     behind   {} change(s) the load gate would refuse on:\n",
            behind.len()
        ));
        for (code, change) in behind {
            out.push_str(&format!("           {code:<3} {change}\n"));
        }
    }
    match &report.activation {
        Activation::Staged if !behind.is_empty() => out.push_str(&format!(
            "activated  no  (--no-activate: staged; run `ordo migrate --apply {path} \
             --db {db}`, then activate)\n"
        )),
        Activation::Staged => out
            .push_str("activated  no  (--no-activate: the deployment still points where it did)\n"),
        Activation::Switched { from, to } => out.push_str(&format!(
            "activated  {to}  (was {})\nnodes      nudged on `{}`\n",
            from.as_deref().unwrap_or("nothing"),
            ordo_exec::registry::DEPLOY_CHANNEL,
        )),
        Activation::AlreadyActive { version } => out.push_str(&format!(
            "activated  {version}  (already active; nothing re-pointed)\n"
        )),
    }
    let t = &report.tenants;
    if t.advanced + t.current > 0 || !t.held.is_empty() {
        out.push_str(&format!(
            "tenants    {} brought to this version, {} already there, {} held closed\n",
            t.advanced,
            t.current,
            t.held.len()
        ));
        for (target, why) in &t.held {
            out.push_str(&format!("           held  {target}: {why}\n"));
        }
        if !t.held.is_empty() {
            out.push_str(
                "           a held tenant stays closed until its database is brought \
                 forward; the deployment retries the ones that were unreachable\n",
            );
        }
    }
    out
}

/// A refusal, as an operator reads it.
///
/// The same class words on both paths; the remedy that names a command is
/// built here, where the path and the database name are known.
pub(crate) fn render_refusal(
    refusal: &ordo_host::apply_op::ApplyRefused,
    path: &str,
    db: &str,
) -> String {
    use ordo_host::apply_op::ApplyRefused;
    match refusal {
        ApplyRefused::NotActivated { version, changes } => {
            let mut out = format!(
                "schema     behind   {} change(s) the load gate would refuse on:\n",
                changes.len()
            );
            for (code, change) in changes {
                out.push_str(&format!("           {code:<3} {change}\n"));
            }
            out.push_str(&format!(
                "REFUSED  not activated: no node could load {version} over this database. \
                 These changes need your say, so they are not applied for you. Run\n         \
                 `ordo migrate --apply {path} --db {db}` to move the schema, \
                 then `ordo apply` again.\n         \
                 (Safe additive changes are applied by `ordo apply` itself; \
                 {} off would stop that too.)\n",
                ordo_host::schema_apply::SAFE_CHANGES_SETTING
            ));
            out
        }
        other => format!("REFUSED  {other}\n"),
    }
}

/// `ordo rollback` — re-point the registry at a version it already holds.
///
/// It is deliberately the *same mechanism* as `apply`'s activation rather
/// than a special path: `implementation.md` says rollback is re-pointing at
/// the prior version hash, and a rollback that had its own code could
/// behave differently from the deploy it is undoing. What it appends is one
/// more activation row, so the history reads forwards.
async fn rollback(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--db", "--as", "--version"]) {
        return Outcome::usage(e);
    }
    let version = match flags.require("--version") {
        Ok(v) => v.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let actor = flags
        .named
        .get("--as")
        .cloned()
        .unwrap_or_else(|| "operator:unattributed".to_string());
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    // The one body, which `std.deployment.activate` calls too: the same
    // lock as an apply, and the same refusal for a version whose tables
    // this database does not hold.
    use ordo_host::apply_op::Activation;
    match ordo_host::apply_op::activate(&pool, &version, &actor).await {
        Ok(Activation::Switched { from, to }) => Outcome::ok(format!(
            "rolled back to {to}  (was {})\nnodes      nudged on `{}`\n",
            from.as_deref().unwrap_or("nothing"),
            ordo_exec::registry::DEPLOY_CHANNEL,
        )),
        Ok(Activation::AlreadyActive { version }) => {
            Outcome::ok(format!("UNCHANGED  {version} is already active\n"))
        }
        Ok(Activation::Staged) => Outcome::failed("REFUSED an activation answered staged\n".into()),
        Err(e) => Outcome::failed(format!("REFUSED {e}\n")),
    }
}

/// `ordo deploys` — what is stored, what is serving, and every re-point.
///
/// The activation log is printed rather than only the pointer, because the
/// question an operator actually has during an incident is *what was
/// serving at the time*, and a pointer cannot answer it.
async fn deploys(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--db", "--limit"]) {
        return Outcome::usage(e);
    }
    let limit: usize = match flags.named.get("--limit").map(|s| s.parse::<usize>()) {
        None => 20,
        Some(Ok(n)) => n,
        Some(Err(_)) => return Outcome::usage("`--limit` must be a number".into()),
    };
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    // The one body, which `std.deployment.versions` answers from too.
    let ordo_host::apply_op::History {
        active,
        stored,
        activations: log,
    } = match ordo_host::apply_op::history(&pool, limit).await {
        Ok(h) => h,
        Err(e) => return Outcome::failed(format!("database: {e}\n")),
    };
    let mut out = format!(
        "active     {}\n\nSTORED ({})\n",
        active
            .as_deref()
            .unwrap_or("nothing has ever been activated"),
        stored.len()
    );
    for (v, at, actor) in &stored {
        out.push_str(&format!(
            "  {v}  applied {at} by {actor}{}\n",
            if Some(v.as_str()) == active.as_deref() {
                "  <- serving"
            } else {
                ""
            }
        ));
    }
    out.push_str(&format!(
        "\nACTIVATIONS (newest first, {} shown)\n",
        log.len()
    ));
    for a in &log {
        out.push_str(&format!(
            "  #{}  {}  at {} by {}\n",
            a.seq, a.version, a.at.0, a.actor
        ));
    }
    Outcome::ok(out)
}

// ── revert ───────────────────────────────────────────────────────────────

async fn revert(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--space", "--db", "--group", "--as", "--dry-run"]) {
        return Outcome::usage(e);
    }
    let Some(path) = flags.positional.first().cloned() else {
        return Outcome::usage(
            "revert needs the program (a `.ordo` file or a directory): the guards it must \
             satisfy and the entities it must write live in the schema, not in the log"
                .into(),
        );
    };
    let (space, correlation, principal) = match (
        flags.require("--space"),
        flags.require("--group"),
        flags.require("--as"),
    ) {
        (Ok(s), Ok(g), Ok(p)) => (
            SpaceId(s.to_string()),
            CorrelationId(g.to_string()),
            p.to_string(),
        ),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return Outcome::usage(e),
    };
    let src = match crate::Sources::read(std::path::Path::new(&path)) {
        Ok(s) => s,
        Err(e) => return Outcome::failed(e),
    };
    let (schema, _registry) = match crate::load_sources(&src) {
        Ok(pair) => pair,
        Err(e) => return Outcome::failed(e),
    };
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };

    let head = format!(
        "space   {space}\ngroup   {}\nas      user:{principal}\n",
        correlation.0
    );
    // **No key custody, stated rather than faked.** Resolving a sealed
    // image means opening a payload, and opening needs a `KeyProvider`.
    // This binary has none to give, and inventing custody here would be
    // worse than having none. So `ordo revert` still refuses an entity with
    // a sealed field. A server-side caller holding `Executor::keys` passes
    // `Some` and reverts sealed rows for real.
    //
    // The reason has moved twice now, and the second move is R2c's. It was
    // "nothing persists the payloads" until 8j; then "there is no
    // `WrappedKeyStore` over Postgres" until `keystore::PgKeyStore` landed.
    // What is missing today is the operator tool's custody *selection* —
    // which master key, from where, with which refusal when it is absent —
    // and that is `plan-runtime.md` R2b's `ORDO_KEY_PROVIDER` plus the
    // `ordo keys` ceremony. Wiring it before that surface exists would mean
    // this command inventing its own answer to a question the deployment is
    // about to answer once, for every process.
    let keys: Option<&dyn undo::KeyProvider> = None;
    // **No object store either, for the same reason and one more.**
    // Verifying a restored handle against the object that exists needs a
    // resolved store and a key release, and this binary has neither. With
    // no pipeline the check is skipped whole and the absent case is not
    // reported, which is the honest answer: a caller that cannot reach an
    // object has learned nothing by not reaching it. `ordo verify
    // --objects` (C7) is where this tool grows the ability, and wiring a
    // store here before then would mean a revert that reports absence for
    // every object in a deployment it was never configured to see.
    let blobs: Option<&undo::BlobPipeline> = None;
    let events: Option<&undo::Recorder> = None;
    if flags.has("--dry-run") {
        return match undo::plan_revert(
            &pool,
            &schema,
            keys,
            &space,
            &correlation,
            &principal,
            now(),
        )
        .await
        {
            Ok(plan) => Outcome::ok(format!(
                "{head}PLAN    {} — nothing applied\n{}",
                plan.correlation.0,
                render_group(&plan)
            )),
            Err(e) => Outcome::failed(format!("{head}REFUSED {e}\n")),
        };
    }
    match undo::revert(
        &pool,
        &schema,
        keys,
        blobs,
        events,
        &space,
        &correlation,
        &principal,
        now(),
    )
    .await
    {
        Ok(r) => Outcome::ok(format!(
            "{head}REVERTED as new history: group `{}` at chain index {} = {}\n{}",
            r.group.correlation.0,
            r.index,
            r.hash,
            render_group(&r.group),
        )),
        Err(e) => Outcome::failed(format!("{head}REFUSED {e}\n")),
    }
}

/// One line per command, with the fields that actually moved.
///
/// A revert's whole value to the person running it is being able to see
/// what it will touch before it happens and what it touched after — so the
/// rendering is a field-level diff, not a count.
fn render_group(group: &CommandGroup) -> String {
    let mut out = String::new();
    for c in &group.commands {
        let verb = match c.kind {
            CommandKind::Create => "create",
            CommandKind::Converge => "converge",
            CommandKind::Delete => "delete",
        };
        out.push_str(&format!("  {verb} {}/{}\n", c.entity, c.row_id));
        for line in diff(c.before.as_ref(), c.after.as_ref()) {
            out.push_str(&format!("    {line}\n"));
        }
    }
    out
}

fn diff(before: Option<&Image>, after: Option<&Image>) -> Vec<String> {
    let empty = Image::new();
    let b = before.unwrap_or(&empty);
    let a = after.unwrap_or(&empty);
    let mut keys: Vec<&String> = b.keys().chain(a.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|k| b.get(*k) != a.get(*k))
        .map(|k| match (b.get(k), a.get(k)) {
            (Some(x), Some(y)) => format!("{k}: {} -> {}", show(x), show(y)),
            (Some(x), None) => format!("{k}: {} -> (gone)", show(x)),
            (None, Some(y)) => format!("{k}: {}", show(y)),
            (None, None) => unreachable!("a key came from one of the two images"),
        })
        .collect()
}

fn show(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Text(s) => format!("{s:?}"),
        Value::Id(s) => s.clone(),
        Value::Decimal(d) => d.to_string(),
        // Unreachable through a revert (the plan refuses one), but this is
        // a renderer: it prints what it is given, and it must never print
        // something that looks like a value for a handle that is not one.
        Value::SealedRef(_) => "(sealed)".into(),
    }
}

// ── quota ────────────────────────────────────────────────────────────────

/// `ordo quota set` / `ordo quota show` — 17n M2's operator surface over
/// `$quotas`. Dispatches on the first positional argument the way `ordo
/// keys` does, because the two subcommands take disjoint flags and a flat
/// flag set would let `set --space s` silently succeed with `show`'s typo.
async fn quota(flags: Flags) -> Outcome {
    let Some(sub) = flags.positional.first().cloned() else {
        return Outcome::usage(
            "ordo quota <set --space <s> --quantity <requests|rows_returned> --limit <n> \
             --grace <n> --period-secs <n> | show --space <s>>\n"
                .into(),
        );
    };
    let mut flags = flags;
    flags.positional.remove(0);
    match sub.as_str() {
        "set" => quota_set(flags).await,
        "show" => quota_show(flags).await,
        other => Outcome::usage(format!(
            "unknown `ordo quota` subcommand `{other}` — `set` or `show`"
        )),
    }
}

/// `ordo quota set --space <s> --quantity <requests|rows_returned> --limit <n> --grace <n> \
/// --period-secs <n>` — an upsert: re-running with a new number is how a
/// limit changes, not a two-step remove-then-add. Refuses a quantity
/// outside v1's set (`ordo_host::quotas::QUANTITIES`) — `writes` is 18i's,
/// on the post-deployment roadmap, and a typo'd quantity that silently
/// never matched anything would look configured and gate nothing.
async fn quota_set(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&[
        "--space",
        "--quantity",
        "--limit",
        "--grace",
        "--period-secs",
        "--db",
    ]) {
        return Outcome::usage(e);
    }
    let space = match flags.require("--space") {
        Ok(s) => s.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let quantity = match flags.require("--quantity") {
        Ok(s) => s.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let limit = match parse_u64(&flags, "--limit") {
        Ok(n) => n,
        Err(e) => return Outcome::usage(e),
    };
    let grace = match parse_u64(&flags, "--grace") {
        Ok(n) => n,
        Err(e) => return Outcome::usage(e),
    };
    let period_secs = match parse_u64(&flags, "--period-secs") {
        Ok(n) => n,
        Err(e) => return Outcome::usage(e),
    };
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    if let Err(e) = ordo_host::quotas::ensure_table(&pool).await {
        return Outcome::failed(format!("database: {e}\n"));
    }
    match ordo_host::quotas::set_limit(&pool, &space, &quantity, limit, grace, period_secs).await {
        Ok(()) => Outcome::ok(format!(
            "space   {space}\nSET     {quantity} limit {limit} grace {grace} period {period_secs}s\n"
        )),
        // The same engine and words as `std.deployment.setQuota` (G9).
        Err(e @ ordo_host::quotas::QuotaRefusal::Database(_)) => {
            Outcome::failed(format!("{e}\n"))
        }
        Err(e) => Outcome::usage(format!("{e}\n")),
    }
}

/// `ordo quota show --space <s>` — what is configured, not what has been
/// used; `ordo usage` (M3) answers that.
async fn quota_show(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--space", "--db"]) {
        return Outcome::usage(e);
    }
    let space = match flags.require("--space") {
        Ok(s) => s.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    if let Err(e) = ordo_host::quotas::ensure_table(&pool).await {
        return Outcome::failed(format!("database: {e}\n"));
    }
    match ordo_host::quotas::limits_for(&pool, &space).await {
        Ok(rows) if rows.is_empty() => {
            Outcome::ok(format!("space   {space}\n(no quotas configured)\n"))
        }
        Ok(rows) => {
            let mut out = format!("space   {space}\n");
            for (quantity, limit) in rows {
                out.push_str(&format!(
                    "  {quantity:<14} limit {:<8} grace {:<8} period {}s\n",
                    limit.limit, limit.grace, limit.period_secs
                ));
            }
            Outcome::ok(out)
        }
        Err(e) => Outcome::failed(format!("database: {e}\n")),
    }
}

fn parse_u64(flags: &Flags, name: &str) -> Result<u64, String> {
    flags.require(name).and_then(|v| {
        v.parse::<u64>()
            .map_err(|_| format!("`{name}` must be a whole number"))
    })
}

// ── usage ────────────────────────────────────────────────────────────────

/// The one-line description every `ordo usage` usage error repeats — the
/// CLI's own words for M3's boundary (`plan-production-ops.md` "the
/// billing boundary"): the platform emits records, and building an invoice
/// on top of them is somebody else's job.
const USAGE_TEXT: &str = "ordo usage --space <s> --from <secs> --to <secs> [--db <name>]\n\
     Ordo meters. It does not invoice.\n";

/// `ordo usage --space <s> --from <secs> --to <secs>` — 17n M3, the CLI
/// twin of `GET /usage`. Both render through `ordo_exec::usage::
/// render_between`, so the two surfaces say byte-identical things about
/// the same closed period, gaps (18d R-O5) included.
async fn usage(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--space", "--from", "--to", "--db"]) {
        return Outcome::usage(e);
    }
    let space = match flags.require("--space") {
        Ok(s) => s.to_string(),
        Err(_) => return Outcome::usage(USAGE_TEXT.into()),
    };
    let from = match parse_u64(&flags, "--from") {
        Ok(n) => n,
        Err(_) => return Outcome::usage(USAGE_TEXT.into()),
    };
    let to = match parse_u64(&flags, "--to") {
        Ok(n) => n,
        Err(_) => return Outcome::usage(USAGE_TEXT.into()),
    };
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    match ordo_exec::usage::render_between(&pool, &space, from, to).await {
        Ok(out) => Outcome::ok(out),
        Err(e) => Outcome::failed(format!("database: {e}\n")),
    }
}

// ── fleet ────────────────────────────────────────────────────────────────

/// `ordo fleet` — 18d R-O6: what an operator sees about the fleet, in one
/// place, assembled from what the **control database alone** can say.
///
/// # Three of the six facts are not here, on purpose
///
/// R-O6 names six facts an operator must see in one place. This command
/// opens a database connection, not a socket into a running replica, and
/// three of the six are the state of a *process*, not of the database:
/// the quarantine register (`Spaces::quarantined`, a `Mutex` that dies with
/// the replica that holds it), the connection budget a particular node
/// computed from its own `PoolLimits`, and which credential labels a
/// particular node has installed. Printing a snapshot of durable state as
/// if it answered for those would be a report that looks complete and
/// is not; they belong on `/health/ready` (T5's register is already read
/// there — see `runtime.rs`) and a future extension of it, which is a live
/// surface for exactly this reason.
///
/// # What durable state can answer, and does
///
/// Each target's schema version and when it was last confirmed
/// ([`ordo_exec::fleet`], R-S2's cache — itself a cache and never asked to
/// decide anything here either), which tenants are on it (`$spacedb`,
/// joined against `$spaces` for the name — every id prints with its
/// tenant's name beside it, the 2026-09-21 ruling this whole increment is
/// named for), and each target's custody (`$spacedb.custody`, R-O3's
/// question, answered before a restore is planned rather than during one).
async fn fleet_cmd(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--db"]) {
        return Outcome::usage(e);
    }
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    if let Err(e) = ordo_host::fleet::ensure_tables(&pool).await {
        return Outcome::failed(format!("database: {e}\n"));
    }
    match ordo_host::fleet::read(&pool).await {
        Ok(fleet) => Outcome::ok(render_fleet(&fleet.targets)),
        Err(e) => Outcome::failed(format!("database: {e}\n")),
    }
}

/// One target row per distinct database, `default` first, then every
/// attached target, each with its version, its custody and its tenants,
/// every tenant printed as its id **and** its name (R-O6). The grouping is
/// `ordo_host::fleet::group`, which `std.deployment.tenants` shares.
fn render_fleet(targets: &[ordo_host::fleet::Target]) -> String {
    let mut out = String::new();
    for t in targets {
        let version = t
            .confirmed
            .as_ref()
            .map(|c| format!("{} (confirmed {})", c.version, c.confirmed_at))
            .unwrap_or_else(|| "not yet confirmed".to_string());
        let custody = t
            .custody
            .map(|c| c.as_str())
            .unwrap_or("platform (the deployment's own database)");
        out.push_str(&format!(
            "target  {}\n  version   {version}\n  custody   {custody}\n",
            t.label
        ));
        if t.tenants.is_empty() {
            out.push_str("  tenants   (none)\n");
        } else {
            for (id, name) in &t.tenants {
                out.push_str(&format!("  tenant    {id}  {name}\n"));
            }
        }
    }
    out
}

// ── space ────────────────────────────────────────────────────────────────

/// `ordo space create`, the one subcommand `space` has (17aa piece 3).
///
/// Dispatches on the first positional the way `ordo quota` and `ordo keys`
/// do, for the same reason: room for a second subcommand later (the
/// roadmap's `std.space.reap`) without every existing flag set having to
/// make way for it.
/// `ordo import --bundle <b> --name <n> --host <h> --token <t> --space <s>`.
///
/// Door-only, on `space create`'s precedent and for a reason of its own: an
/// import re-seals every sealed field under the destination's key, and this
/// tool holds no custody. See `crate::wire::import_over_the_wire`.
async fn import_cmd(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--bundle", "--name", "--host", "--token", "--space"]) {
        return Outcome::usage(e);
    }
    crate::wire::import_over_the_wire(&flags).await
}

/// `ordo console-key revoke --id <ck-…> --host <h> --token <t> --space <s>`:
/// revoke a console's key from the deployment's side
/// (`std.deployment.revokeConsoleKey`), over the door like `space create`.
async fn console_key_cmd(flags: Flags) -> Outcome {
    const USAGE: &str =
        "ordo console-key revoke --id <ck-...> --host <h> --token <t> --space <s>\n";
    let Some(sub) = flags.positional.first().cloned() else {
        return Outcome::usage(USAGE.into());
    };
    let mut flags = flags;
    flags.positional.remove(0);
    match sub.as_str() {
        "revoke" => {
            if let Err(e) = flags.only(&["--id", "--host", "--token", "--space"]) {
                return Outcome::usage(e);
            }
            crate::wire::revoke_console_key_over_the_wire(&flags).await
        }
        other => Outcome::usage(format!(
            "unknown `ordo console-key` subcommand `{other}`, the only one is `revoke`\n{USAGE}"
        )),
    }
}

async fn space_cmd(flags: Flags) -> Outcome {
    let Some(sub) = flags.positional.first().cloned() else {
        return Outcome::usage(
            "ordo space create --name <n> --owner <o> --host <h> --token <t> --space <s>\n".into(),
        );
    };
    let mut flags = flags;
    flags.positional.remove(0);
    match sub.as_str() {
        "create" => space_create(flags).await,
        other => Outcome::usage(format!(
            "unknown `ordo space` subcommand `{other}`, the only one is `create`\n"
        )),
    }
}

/// `ordo space create --name <n> --owner <o> --host <h> --token <t> \
/// --space <s>`: mint a tenant through `std.space.create`.
///
/// This command has no database mode. `apply`'s two paths exist because it
/// had a database path before it had a door; `std.space.create` never did,
/// per `tenant_ops.rs`'s module doc, so `--host` is not optional here the
/// way it is for `apply`, and `crate::wire::create_space_over_the_wire`
/// is the whole body: same host resolution, same bearer token, same
/// refusal rendering as `apply --host`, over `ordo_host::tenant_ops::CREATE`
/// instead of `apply_op::APPLY`.
async fn space_create(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&[
        "--name",
        "--owner",
        "--host",
        "--token",
        "--space",
        // Pins the key `space create` otherwise mints, so a re-run after a
        // lost connection replays the first answer instead of building a
        // second tenant. See `wire::space_create`.
        "--idempotency-key",
    ]) {
        return Outcome::usage(e);
    }
    crate::wire::create_space_over_the_wire(&flags).await
}
