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
            [--digest <sha256:...>] [--run]  --host <h> ...
    render the step, refuse it if it has errors, store it; print the receipt and its digest.
    With <dir>, write the receipt to <dir>/receipts/<from>-to-<to>.json. --digest refuses a
    deployment that renders another one. --run runs it on the deployment's own database.
ordo evolve --tenant <id> [--to <version>]  --host <h> ...
    replay the stored path on the tenant's database, one step at a time, then pin it
    (to the active version when --to is absent).
ordo evolve --list [--from <version>] [--to <version>]  --host <h> ...
    the stored plans.";

/// `ordo evolve`.
pub(crate) async fn run(flags: Flags) -> Outcome {
    let mut allowed = vec![
        "--from", "--to", "--places", "--digest", "--run", "--list", "--tenant",
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
            "expectDigest": flags.named_value("--digest"),
            "run": flags.has("--run"),
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
    }
    Outcome::ok(out)
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
    out.push_str(&format!(
        "pinned     {} (was {})\n",
        row["pinned"]["to"].as_str().unwrap_or("?"),
        row["pinned"]["from"].as_str().unwrap_or("none")
    ));
    Outcome::ok(out)
}
