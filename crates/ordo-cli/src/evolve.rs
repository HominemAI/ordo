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
//!   tenant's database, then pins it (`std.deployment.replay`).
//! - `ordo evolve --list [--from X] [--to Y]` lists the stored plans
//!   (`std.deployment.plans`).
//!
//! None opens a database connection (owner, 2026-11-05: the CLI goes
//! through the door).

use crate::operator::{door_only, Flags, Outcome};

pub(crate) const USAGE: &str = "\
ordo evolve [<dir>] --from <version> --to <version> [--places <entity>.<field>=rescale|keep[,...]]
            [--fill <entity>.<field>=<expression>|fn <name> [secrets a,b] [hosts h,i][;...]]
            [--audit-images <entity>.<field>[,...]] [--digest <sha256:...>] [--run]
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
ordo evolve --tenant <id> [--to <version>]  --host <h> ...
    replay the stored path on the tenant's database, one step at a time, then pin it
    (to the active version when --to is absent).
ordo evolve --list [--from <version>] [--to <version>]  --host <h> ...
    the stored plans.";

/// `ordo evolve`.
pub(crate) async fn run(flags: Flags) -> Outcome {
    let mut allowed = vec![
        "--from",
        "--to",
        "--places",
        "--fill",
        "--audit-images",
        "--digest",
        "--run",
        "--list",
        "--tenant",
        "--approve-sign-in-removal",
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

async fn replay(host: &crate::wire::Host, flags: &Flags) -> Outcome {
    let [tenant] = flags.positional.as_slice() else {
        return Outcome::usage(format!("name one tenant: `--tenant <id>`\n{USAGE}\n"));
    };
    let row = match crate::wire::door_op(
        host,
        flags,
        ordo_host::deploy_ops::REPLAY,
        serde_json::json!({ "tenant": tenant, "to": flags.named_value("--to") }),
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
    if row["pinned"].is_null() {
        out.push_str("pinned     not yet: a backfill is still running; replay again to resume\n");
    } else {
        out.push_str(&format!(
            "pinned     {} (was {})\n",
            row["pinned"]["to"].as_str().unwrap_or("?"),
            row["pinned"]["from"].as_str().unwrap_or("none")
        ));
    }
    Outcome::ok(out)
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
