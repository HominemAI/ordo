//! `ordo evolve`: stored plans through the door (`plan-versions.md` §2.3,
//! increment 8; ruled 2026-10-04, `syntax-additions` ask 1).
//!
//! - `ordo evolve [<dir>] --from X --to Y [--places ...] [--digest D] [--run]`
//!   renders the step from X to Y on the deployment, refuses it with its
//!   errors, and stores it (`std.deployment.evolve`). It prints the receipt,
//!   the digest and the readers of each changed table. With `<dir>`, the
//!   receipt is written beside the sources, for the repository:
//!   `<dir>/receipts/<X>-to-<Y>.json`. With `--digest`, a deployment that
//!   renders another digest refuses (staging re-rendering dev's plan). With
//!   `--run`, the stored plan runs on the deployment's own database.
//!   The pre-checks run on every database, and every issue is printed with
//!   its fix; `--run` refuses while any is open (increment 9). A step that
//!   removes a sign-in method some people rely on stops, and goes on only
//!   with `--approve-sign-in-removal <count>` (ruled 2026-11-23, Q6).
//! - `ordo evolve --tenant <id> [--to Y]` replays the stored path on the
//!   tenant's database, then pins it (`std.deployment.replay`). A step that
//!   rewrites data (a rename, a filled field) moves the tenant with dual
//!   writes (increment 11a): it keeps writing, the backfill runs (`--budget`
//!   chunks of `--chunk-rows` at a time; run again to resume), the pin
//!   switches, and a soak (`--soak`, 7 days by default) keeps the old shape
//!   current. `--revert` returns it in the soak with every write kept
//!   (`std.deployment.revertSoak`); `--close-soak` ends the soak now
//!   (`std.deployment.closeSoak`).
//! - `ordo evolve --list [--from X] [--to Y]` lists the stored plans
//!   (`std.deployment.plans`).
//! - `ordo evolve --orphans` lists what versions left behind in every
//!   database, with drop dates and tombstones (`std.deployment.orphans`).
//! - `ordo evolve --prune <entity.field>` shows what pruning would remove,
//!   and removes it only when asked twice: `--confirm <entity.field>`, and in
//!   production `--confirm-deployment <name>` (`std.deployment.prune`,
//!   increment 14). On a terminal it asks for both instead.
//!
//! None opens a database connection (owner, 2026-11-05: the CLI goes
//! through the door).

use crate::operator::{door_only, Flags, Outcome};

pub(crate) const USAGE: &str = "\
ordo evolve [<dir>] --from <version> --to <version> [--places <entity>.<field>=rescale|keep[,...]]
            [--fill <entity>.<field>=<expression>|fn <name> [secrets a,b] [hosts h,i][;...]]
            [--audit-images <entity>.<field>[,...]] [--keep-plaintext <entity>.<field>[,...]]
            [--reverse <entity>.<old field>=<expression>[;...]] [--one-way]
            [--correspond <part>.<link>[,...]]
            [--digest <sha256:...>] [--run]
            [--approve-sign-in-removal <count>]  --host <h> ...
    render the step, refuse it if it has errors, store it; print the receipt and its digest,
    then the pre-checks on every database, each issue with its fix.
    With <dir>, write the receipt to <dir>/receipts/<from>-to-<to>.json. --digest refuses a
    deployment that renders another one. --run runs it on the deployment's own database,
    and refuses while a pre-check issue is open. A step that removes a sign-in method some
    people rely on stops; --approve-sign-in-removal <count> goes on, naming the count.
    --fill states how every existing row gets a value for a field the step makes required:
    an expression over the row's own fields, or a backend function of the new version with
    the secrets and hosts it may use. A backfill runs in chunks and resumes from its cursor.
    Its audit record is one summary per tenant; --audit-images writes every row's images.
    A step that seals a field drops its plaintext column once every row is sealed;
    --keep-plaintext keeps it, orphaned, until pruned. Its values stay readable until then,
    and in WAL and backups for good.
    A split (two fields each `prev(name)`) or a merge (`prev(first, last)`) takes a --fill
    for each new field, over the old ones, and a --reverse for each old field, over the new
    ones, which keeps it current through the soak. With no reverse it is one way, and needs
    --one-way. The pre-checks list the rows a mapping does not parse, and preview it on 3 rows.
    A rename that changes the type is converted the same way: --fill with `int(x)`, `text(x)`
    or `bool(x)`, and --reverse.
    An entity split (`entity <part> (prev: <entity>)` while <entity> stays declared) takes
    --correspond <part>.<link>: one part row per row, its owned unique link naming it.
ordo evolve --tenant <id> [--to <version>] [--budget <chunks>] [--chunk-rows <n>] [--soak <7d|0>]  --host <h> ...
    replay the stored path on the tenant's database, one step at a time, then pin it
    (to the active version when --to is absent). A step that rewrites data moves the tenant
    with dual writes: every write keeps both shapes while the backfill runs and through the
    soak. --budget stops the backfill after that many chunks; run again to resume.
ordo evolve --tenant <id> --revert | --close-soak  --host <h> ...
    in the soak, move the tenant back with every write kept, or close the soak now: the old
    columns are orphaned and readers lose them.
ordo evolve --list [--from <version>] [--to <version>]  --host <h> ...
    the stored plans.
ordo evolve --orphans  --host <h> ...
    what versions left behind in every database: each orphan, its drop date, and the tombstones.
ordo evolve --prune <entity.field> [--confirm <entity.field>] [--confirm-deployment <name>]  --host <h> ...
    what pruning would remove, then, asked twice, remove it from every database and keep a
    tombstone. Production also asks for the deployment's name. Off in production until
    `ordo environments set prune on --reason \"...\"`.";

/// `ordo evolve`.
pub(crate) async fn run(flags: Flags) -> Outcome {
    let mut allowed = vec![
        "--from",
        "--to",
        "--places",
        "--fill",
        "--audit-images",
        "--keep-plaintext",
        "--reverse",
        "--one-way",
        "--correspond",
        "--digest",
        "--run",
        "--list",
        "--tenant",
        "--approve-sign-in-removal",
        "--orphans",
        "--prune",
        "--confirm",
        "--confirm-deployment",
        "--budget",
        "--chunk-rows",
        "--soak",
        "--revert",
        "--close-soak",
    ];
    allowed.extend(crate::operator::DOOR_FLAGS);
    if let Err(e) = flags.only(&allowed) {
        return Outcome::usage(format!("{e}\n{USAGE}\n"));
    }
    let host = match door_only(&flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    if flags.has("--list") {
        return list(&host, &flags).await;
    }
    if flags.has("--orphans") {
        return orphans(&host, &flags).await;
    }
    if let Some(name) = flags.named_value("--prune") {
        return prune(&host, &flags, name).await;
    }
    if flags.has("--tenant") {
        return replay(&host, &flags).await;
    }
    let (Some(from), Some(to)) = (flags.named_value("--from"), flags.named_value("--to")) else {
        return Outcome::usage(format!(
            "name the step with `--from <version> --to <version>`\n{USAGE}\n"
        ));
    };
    if flags.positional.len() > 1 {
        return Outcome::usage(format!("one source directory at most\n{USAGE}\n"));
    }
    let row = match crate::wire::door_op(
        &host,
        &flags,
        ordo_host::deploy_ops::EVOLVE,
        serde_json::json!({
            "from": from,
            "to": to,
            "places": flags.named_value("--places"),
            "fill": flags.named_value("--fill"),
            "auditImages": flags.named_value("--audit-images"),
            "keepPlaintext": flags.named_value("--keep-plaintext"),
            "reverse": flags.named_value("--reverse"),
            "oneWay": flags.has("--one-way"),
            "correspond": flags.named_value("--correspond"),
            "expectDigest": flags.named_value("--digest"),
            "run": flags.has("--run"),
            "approveSignInRemoval": flags.named_value("--approve-sign-in-removal"),
        }),
        true,
    )
    .await
    {
        Ok(r) => r,
        Err(o) => return o,
    };
    let mut out = format!(
        "plan       {} to {}\ndigest     {}\nstored     {}\n",
        row["from"].as_str().unwrap_or(from),
        row["to"].as_str().unwrap_or(to),
        row["digest"].as_str().unwrap_or("?"),
        if row["stored"].as_bool() == Some(true) {
            "yes"
        } else {
            "already stored"
        },
    );
    if let Some(o) = row["options"].as_str().filter(|o| !o.is_empty()) {
        out.push_str(&format!("options    {o}\n"));
    }
    for line in ordo_host::evolve::receipt_lines(&row["receipt"]) {
        out.push_str(&format!("  {line}\n"));
    }
    for r in row["readers"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "READER     {}\n",
            r["line"].as_str().unwrap_or("?")
        ));
    }
    for w in row["warnings"].as_array().into_iter().flatten() {
        out.push_str(&format!("WARNING    {}\n", w.as_str().unwrap_or("?")));
    }
    if let Some(s) = row["signIn"].as_object() {
        out.push_str(&format!(
            "SIGN-IN    removes {}; {} person(s) left with no method{}\n",
            s.get("removed")
                .and_then(|r| r.as_array())
                .map(|r| r
                    .iter()
                    .filter_map(|m| m.as_str())
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
            s.get("people").and_then(|n| n.as_u64()).unwrap_or(0),
            if s.get("people").and_then(|n| n.as_u64()).unwrap_or(0) > 0 {
                " (approved)"
            } else {
                ""
            }
        ));
        for n in s
            .get("peerNotes")
            .and_then(|n| n.as_array())
            .into_iter()
            .flatten()
        {
            out.push_str(&format!("PEER       {}\n", n.as_str().unwrap_or("?")));
        }
    }
    out.push_str(&precheck_lines(&row["prechecks"]));
    if let Some(dir) = flags.positional.first() {
        match write_receipt(dir, &row) {
            Ok(path) => out.push_str(&format!("receipt    {path}\n")),
            Err(e) => {
                return Outcome::failed(format!(
                    "{out}the plan is stored, and its receipt was not written: {e}\n"
                ))
            }
        }
    }
    if !row["ran"].is_null() {
        let ran = &row["ran"];
        if ran["current"].as_bool() == Some(true) {
            out.push_str("ran        the deployment's database already records the step\n");
        } else {
            out.push_str(&format!(
                "ran        on the deployment's database: {} statement(s)\n",
                ran["statements"].as_i64().unwrap_or(0)
            ));
        }
        for b in ran["backfills"].as_array().into_iter().flatten() {
            out.push_str(&format!("{}\n", backfill_line(b)));
        }
        // A seal's end moved the deployment's own space to the version the
        // database now holds (increment 13). "Active" waits for activation.
        if let Some(v) = ran["ownSpace"]["version"].as_str() {
            out.push_str(&format!(
                "own space  `{}` now serves {v}; activate {v} to move the rest\n",
                ran["ownSpace"]["space"].as_str().unwrap_or("?")
            ));
        }
    }
    Outcome::ok(out)
}

/// One backfill's report, for a person: what was filled, how many rows,
/// and the record written.
fn backfill_line(b: &serde_json::Value) -> String {
    let p = &b["progress"];
    let mut line = format!(
        "BACKFILL   {}.{} = {}: {} of {} row(s), {}",
        b["entity"].as_str().unwrap_or("?"),
        b["field"].as_str().unwrap_or("?"),
        b["mapping"].as_str().unwrap_or("?"),
        p["rowsDone"].as_i64().unwrap_or(0),
        p["rowsTotal"].as_i64().unwrap_or(0),
        p["state"].as_str().unwrap_or("?"),
    );
    if let Some(v) = b["verified"].as_object() {
        line.push_str(&format!(
            "; verified {} row(s) in {} space(s), {} audit record",
            v.get("rows").and_then(|r| r.as_i64()).unwrap_or(0),
            v.get("spaces")
                .and_then(|s| s.as_array())
                .map(Vec::len)
                .unwrap_or(0),
            b["audit"].as_str().unwrap_or("?"),
        ));
    } else {
        line.push_str("; stopped at its budget, run the step again to resume from its cursor");
    }
    line
}

/// The pre-checks as a person reads them: each issue with its fix, each
/// database not asked, and each check that did not run.
fn precheck_lines(p: &serde_json::Value) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut out = format!(
        "prechecks  {} database(s) asked, {} issue(s)\n",
        p["checked"].as_array().map_or(0, |c| c.len()),
        p["issues"].as_array().map_or(0, |c| c.len())
    );
    for i in p["issues"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "ISSUE      database `{}`: {} {}\n",
            i["database"].as_str().unwrap_or("?"),
            i["code"].as_str().unwrap_or("?"),
            i["what"].as_str().unwrap_or("?")
        ));
        for l in i["lines"].as_array().into_iter().flatten() {
            out.push_str(&format!("             {}\n", l.as_str().unwrap_or("?")));
        }
        out.push_str(&format!(
            "  fix      {}\n",
            i["fix"].as_str().unwrap_or("?")
        ));
    }
    for n in p["notProbed"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "not asked  database `{}`: {}\n",
            n["database"].as_str().unwrap_or("?"),
            n["why"].as_str().unwrap_or("?")
        ));
    }
    for n in p["notRun"].as_array().into_iter().flatten() {
        out.push_str(&format!("not run    {}\n", n.as_str().unwrap_or("?")));
    }
    for v in p["preview"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "PREVIEW    database `{}`: {}\n",
            v["database"].as_str().unwrap_or("?"),
            v["change"].as_str().unwrap_or("?")
        ));
        for r in v["rows"].as_array().into_iter().flatten() {
            out.push_str(&format!("             {}\n", r.as_str().unwrap_or("?")));
        }
        if let Some(h) = v["hidden"].as_str() {
            out.push_str(&format!("             {h}\n"));
        }
    }
    out
}

/// The receipt beside the sources: a record for the repository, never an
/// input. Named for its step, so each step keeps its own file.
fn write_receipt(dir: &str, row: &serde_json::Value) -> Result<String, String> {
    let base = std::path::Path::new(dir);
    let base = if base.is_file() {
        base.parent().unwrap_or(std::path::Path::new("."))
    } else {
        base
    };
    let safe = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let receipts = base.join("receipts");
    std::fs::create_dir_all(&receipts).map_err(|e| format!("{}: {e}", receipts.display()))?;
    let path = receipts.join(format!(
        "{}-to-{}.json",
        safe(row["from"].as_str().unwrap_or("from")),
        safe(row["to"].as_str().unwrap_or("to"))
    ));
    let body = serde_json::json!({
        "digest": row["digest"],
        "receipt": row["receipt"],
    });
    std::fs::write(&path, ordo_plan::artifact::to_bytes(&body))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

async fn list(host: &crate::wire::Host, flags: &Flags) -> Outcome {
    let row = match crate::wire::door_op(
        host,
        flags,
        ordo_host::deploy_ops::PLANS,
        serde_json::json!({
            "from": flags.named_value("--from"),
            "to": flags.named_value("--to"),
        }),
        false,
    )
    .await
    {
        Ok(r) => r,
        Err(o) => return o,
    };
    let plans = row["plans"].as_array().cloned().unwrap_or_default();
    if plans.is_empty() {
        return Outcome::ok(
            "no stored plans. Render one with `ordo evolve --from <version> --to <version>`\n"
                .into(),
        );
    }
    let mut out = String::new();
    for p in plans {
        out.push_str(&format!(
            "{} to {}  {}  by {} at {}{}\n",
            p["from"].as_str().unwrap_or("?"),
            p["to"].as_str().unwrap_or("?"),
            p["digest"].as_str().unwrap_or("?"),
            p["actor"].as_str().unwrap_or("?"),
            ordo_host::versions::utc_minute(p["at"].as_u64().unwrap_or(0)),
            p["options"]
                .as_str()
                .filter(|o| !o.is_empty())
                .map(|o| format!("  options {o}"))
                .unwrap_or_default(),
        ));
    }
    Outcome::ok(out)
}

/// A move with dual writes, for a person (increment 11a).
fn move_lines(mv: &serde_json::Value) -> String {
    let mut out = String::new();
    let phase = mv["phase"].as_str().unwrap_or("?");
    out.push_str(&format!(
        "move       {} to {}: {phase}\n",
        mv["from"].as_str().unwrap_or("?"),
        mv["to"].as_str().unwrap_or("?")
    ));
    match phase {
        "backfilling" => out.push_str(
            "           every write keeps both shapes; run again to resume the backfill\n",
        ),
        "gated" => out.push_str(&format!(
            "           serving {}, both shapes current; `--tenant <id>` switches it again\n",
            mv["from"].as_str().unwrap_or("?")
        )),
        "soaking" => {
            if let Some(until) = mv["soakUntil"].as_u64() {
                out.push_str(&format!(
                    "soak       until {}; every write keeps {}'s shape too. `--revert` goes \
                     back with every write kept, `--close-soak` ends it now\n",
                    ordo_host::versions::utc_minute(until),
                    mv["from"].as_str().unwrap_or("?")
                ));
            }
        }
        "closed" => {
            let gone: Vec<&str> = mv["orphaned"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|o| o.as_str())
                .collect();
            out.push_str(&format!(
                "closed     the soak is over{}\n",
                if gone.is_empty() {
                    String::new()
                } else {
                    format!("; orphaned: {}", gone.join(", "))
                }
            ));
        }
        _ => {}
    }
    out
}

async fn soak(host: &crate::wire::Host, flags: &Flags, tenant: &str, op: &str) -> Outcome {
    let row = match crate::wire::door_op(
        host,
        flags,
        op,
        serde_json::json!({ "tenant": tenant }),
        true,
    )
    .await
    {
        Ok(r) => r,
        Err(o) => return o,
    };
    let mut out = format!("tenant     {tenant}\n");
    if let Some(p) = row["pinned"].as_object() {
        out.push_str(&format!(
            "pinned     {} (was {})\n",
            p.get("to").and_then(|v| v.as_str()).unwrap_or("?"),
            p.get("from").and_then(|v| v.as_str()).unwrap_or("none")
        ));
    }
    out.push_str(&move_lines(&row));
    Outcome::ok(out)
}

async fn replay(host: &crate::wire::Host, flags: &Flags) -> Outcome {
    let [tenant] = flags.positional.as_slice() else {
        return Outcome::usage(format!("name one tenant: `--tenant <id>`\n{USAGE}\n"));
    };
    if flags.has("--revert") && flags.has("--close-soak") {
        return Outcome::usage(format!(
            "`--revert` and `--close-soak` are two acts: name one\n{USAGE}\n"
        ));
    }
    if flags.has("--revert") {
        return soak(host, flags, tenant, ordo_host::deploy_ops::REVERT_SOAK).await;
    }
    if flags.has("--close-soak") {
        return soak(host, flags, tenant, ordo_host::deploy_ops::CLOSE_SOAK).await;
    }
    let row = match crate::wire::door_op(
        host,
        flags,
        ordo_host::deploy_ops::REPLAY,
        serde_json::json!({
            "tenant": tenant,
            "to": flags.named_value("--to"),
            "budget": flags.named_value("--budget"),
            "chunkRows": flags.named_value("--chunk-rows"),
            "soak": flags.named_value("--soak"),
        }),
        true,
    )
    .await
    {
        Ok(r) => r,
        Err(o) => return o,
    };
    let steps = row["steps"].as_array().cloned().unwrap_or_default();
    let mut out = format!(
        "tenant     {tenant}\nreplayed   {} to {}, {} step(s)\n",
        row["from"].as_str().unwrap_or("?"),
        row["to"].as_str().unwrap_or("?"),
        steps.len()
    );
    for s in &steps {
        out.push_str(&format!(
            "  {} to {}  {}  {} statement(s)\n",
            s["from"].as_str().unwrap_or("?"),
            s["to"].as_str().unwrap_or("?"),
            s["digest"].as_str().unwrap_or("?"),
            s["statements"].as_i64().unwrap_or(0)
        ));
    }
    for s in &steps {
        for b in s["backfills"].as_array().into_iter().flatten() {
            out.push_str(&format!("{}\n", backfill_line(b)));
        }
    }
    if row["pinned"].is_null() && row["move"].is_object() {
        // A move in its soak, run again: nothing moved this time.
    } else if row["pinned"].is_null() {
        out.push_str("pinned     not yet: a backfill is still running; replay again to resume\n");
    } else {
        out.push_str(&format!(
            "pinned     {} (was {})\n",
            row["pinned"]["to"].as_str().unwrap_or("?"),
            row["pinned"]["from"].as_str().unwrap_or("none")
        ));
    }
    if row["move"].is_object() {
        out.push_str(&move_lines(&row["move"]));
    }
    Outcome::ok(out)
}

async fn orphans(host: &crate::wire::Host, flags: &Flags) -> Outcome {
    let row = match crate::wire::door_op(
        host,
        flags,
        ordo_host::deploy_ops::ORPHANS,
        serde_json::json!({}),
        false,
    )
    .await
    {
        Ok(r) => r,
        Err(o) => return o,
    };
    let mut out = String::new();
    for d in row["databases"].as_array().into_iter().flatten() {
        let db = d["database"].as_str().unwrap_or("?");
        if let Some(e) = d["error"].as_str() {
            out.push_str(&format!("database `{db}`: not read: {e}\n"));
            continue;
        }
        for o in d["orphans"].as_array().into_iter().flatten() {
            let name = o["name"].as_str().unwrap_or("?");
            let when = |k: &str| {
                o[k].as_u64()
                    .map(ordo_host::versions::utc_minute)
                    .unwrap_or_else(|| "-".into())
            };
            if o["prunedAt"].is_null() {
                out.push_str(&format!(
                    "ORPHAN     {name}  database `{db}`  left by {}  since {}  drop date {}{}{}\n",
                    o["to"].as_str().unwrap_or("?"),
                    when("orphanedAt"),
                    when("dropAfter"),
                    if o["due"].as_bool() == Some(true) {
                        " (due)"
                    } else {
                        ""
                    },
                    if o["sealed"].as_bool() == Some(true) {
                        "  sealed"
                    } else {
                        ""
                    },
                ));
            } else {
                out.push_str(&format!(
                    "PRUNED     {name}  database `{db}`  at {} by {}\n",
                    when("prunedAt"),
                    o["prunedBy"].as_str().unwrap_or("?"),
                ));
            }
        }
    }
    if out.is_empty() {
        out.push_str("no orphans: no version left a field or an entity behind\n");
    }
    Outcome::ok(out)
}

/// Ask on the terminal, when there is one. `None` off a terminal.
fn ask(question: &str) -> Option<String> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return None;
    }
    eprint!("{question}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok()?;
    Some(line.trim().to_string())
}

async fn prune(host: &crate::wire::Host, flags: &Flags, name: &str) -> Outcome {
    let call = |confirm: Option<String>, deployment: Option<String>| {
        crate::wire::door_op(
            host,
            flags,
            ordo_host::deploy_ops::PRUNE,
            serde_json::json!({
                "name": name,
                "confirm": confirm,
                "confirmDeployment": deployment,
            }),
            true,
        )
    };
    let mut confirm = flags.named_value("--confirm").map(str::to_string);
    let mut deployment = flags
        .named_value("--confirm-deployment")
        .map(str::to_string);
    let first = match call(confirm.clone(), deployment.clone()).await {
        Ok(r) => r,
        Err(o) => return o,
    };
    let mut out = prune_lines(&first);
    if first["pruned"].as_bool() == Some(true) {
        return Outcome::ok(out);
    }
    // The second ask, typed at apply time.
    let production = first["environment"].as_str() == Some("production");
    if confirm.is_none() {
        print!("{out}");
        out.clear();
        confirm = ask(&format!("Type `{name}` to prune it from every database: "));
        if confirm.is_some() && production && deployment.is_none() {
            deployment = ask("This is production. Type the deployment's name: ");
        }
    }
    if confirm.is_none() {
        out.push_str(&format!(
            "nothing was pruned. To prune it, ask again: {}\n",
            first["confirm"].as_str().unwrap_or("?")
        ));
        return Outcome::ok(out);
    }
    match call(confirm, deployment).await {
        Ok(r) => Outcome::ok(format!("{out}{}", prune_lines(&r))),
        Err(o) => o,
    }
}

fn prune_lines(row: &serde_json::Value) -> String {
    let name = row["name"].as_str().unwrap_or("?");
    let pruned = row["pruned"].as_bool() == Some(true);
    let mut out = String::new();
    for d in row["databases"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "{}  {name}  database `{}`  {} row(s) hold a value\n",
            if pruned { "PRUNED     " } else { "WOULD PRUNE" },
            d["database"].as_str().unwrap_or("?"),
            d["rows"].as_i64().unwrap_or(0),
        ));
    }
    for d in row["notRead"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "not read   database `{}`: {}\n",
            d["database"].as_str().unwrap_or("?"),
            d["why"].as_str().unwrap_or("?")
        ));
    }
    for r in row["readers"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "READER     {} reads this table; it loses the column\n",
            r.as_str().unwrap_or("?")
        ));
    }
    if pruned {
        out.push_str(&format!(
            "pruned     {name}: removed, with what was written to it; a tombstone is kept\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    /// Each issue prints with its database, its spaces and its fix; a
    /// database not asked and a check not run are printed too, so a clean
    /// answer never hides them.
    #[test]
    fn the_prechecks_print_each_issue_with_its_fix() {
        let p = serde_json::json!({
            "checked": ["default", "acme"],
            "issues": [{
                "database": "acme", "code": "C1", "what": "duplicates over `note`.`title`",
                "lines": ["space `acme`  value first  2 row(s)"],
                "fix": "make `note`.`title` distinct",
            }],
            "notProbed": [{ "database": "globex", "why": "it records `0.9.0`" }],
            "notRun": ["the `requires` check did not run"],
        });
        let out = super::precheck_lines(&p);
        assert!(out.contains("2 database(s) asked, 1 issue(s)"), "{out}");
        assert!(out.contains("ISSUE      database `acme`: C1"), "{out}");
        assert!(out.contains("space `acme`"), "{out}");
        assert!(out.contains("fix      make"), "{out}");
        assert!(out.contains("not asked  database `globex`"), "{out}");
        assert!(out.contains("not run    the `requires`"), "{out}");
        assert_eq!(super::precheck_lines(&serde_json::Value::Null), "");
    }
}
