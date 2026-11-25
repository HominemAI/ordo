//! `ordo apply --branch` and `ordo branches`: dev branches as tenants on
//! their own version (`plan-versions.md` §3.3, increment 5), through the
//! door. The host does the work (`ordo_host::branches`); nothing here opens
//! a database.

use serde_json::{json, Value};

use crate::operator::{door_only, Flags, Outcome};

pub(crate) const USAGE: &str = "\
ordo apply <dir|file.ordo> --branch <name> [--seed <file>] [--label \"<text>\"]
    store the version in dev, unactivated, and serve it to the branch's own
    tenant: made the first time with a database of its own, you its owner.
    `--seed <file>` fills a new branch tenant: one JSON object per line,
    {\"op\": \"<name>\", \"params\": {...}}, each run through the door as you.
ordo branches list
    every branch, its tenant, version and when it was last applied. A branch
    not applied to for branch.idle_list_after (ORDO_BRANCH_IDLE_LIST_AFTER on
    the host, 30d by default) is marked idle. Nothing is removed for it.
ordo branches set idle-list-after <n>d|<n>h|<n>m|<n>s|default --reason \"...\"
    store branch.idle_list_after; it replaces the environment variable.
    `default` clears it, so the variable (then 30d) reads again.
ordo branches delete <name> --reason \"...\"
    remove a branch's tenant: its database is dropped and its keys destroyed.
    The only way a branch goes; a merge never removes one.
Each takes --host <h> (or ORDO_HOST), and --token or a signed-in session.";

/// How long the seed step waits for the door to serve a new branch tenant.
const SEED_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// One seed line: an op and its parameters.
fn seed_lines(path: &str) -> Result<Vec<(String, Value)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("{path} line {}: not JSON ({e})", i + 1))?;
        let Some(op) = v["op"].as_str().filter(|s| !s.is_empty()) else {
            return Err(format!(
                "{path} line {}: each line is {{\"op\": \"<name>\", \"params\": {{...}}}}",
                i + 1
            ));
        };
        let params = match &v["params"] {
            Value::Null => json!({}),
            p @ Value::Object(_) => p.clone(),
            _ => return Err(format!("{path} line {}: `params` is an object", i + 1)),
        };
        out.push((op.to_string(), params));
    }
    Ok(out)
}

/// `ordo apply <path> --branch <name>` over the door. `program` and `code`
/// are what the plain apply sends.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply(
    host: &crate::wire::Host,
    flags: &Flags,
    branch: &str,
    program: &str,
    code: Option<String>,
    provenance: &ordo_host::versions::Provenance,
    path: &str,
    db: &str,
) -> Outcome {
    // The seed file is read before anything is applied, so a bad one
    // refuses the command, not a branch half made.
    let seed = match flags.named_value("--seed") {
        Some(p) => match seed_lines(p) {
            Ok(lines) => Some((p.to_string(), lines)),
            Err(e) => return Outcome::usage(format!("{e}\n")),
        },
        None => None,
    };
    let mut params = json!({ "program": program, "branch": branch });
    if let Some(code) = code {
        params["code"] = Value::String(code);
    }
    if let Some(label) = &provenance.label {
        params["label"] = Value::String(label.clone());
    }
    if let Some(commit) = &provenance.git_commit {
        params["gitCommit"] = Value::String(commit.clone());
    }
    let row =
        match crate::wire::door_op(host, flags, ordo_host::apply_op::APPLY, params, true).await {
            Ok(r) => r,
            Err(o) => return o,
        };
    let report = match ordo_host::apply_op::Report::from_json(&row) {
        Ok(r) => r,
        Err(e) => return Outcome::failed(format!("REFUSED  {e}\n")),
    };
    let mut out = crate::operator::render_apply(&report, path, db);
    let b = &row["branch"];
    let space = b["space"].as_str().unwrap_or_default().to_string();
    let created = b["created"].as_bool().unwrap_or(false);
    out.push_str(&format!(
        "branch     {} → tenant {} ({space})\n",
        b["branch"].as_str().unwrap_or(branch),
        b["tenant"].as_str().unwrap_or_default(),
    ));
    out.push_str(&match (created, b["from"].as_str()) {
        (true, _) => format!(
            "pinned     {}  (a new tenant, a database of its own, you its owner)\n",
            report.version
        ),
        (false, Some(from)) => format!("pinned     {}  (was {from})\n", report.version),
        (false, None) => format!("pinned     {}  (already)\n", report.version),
    });
    let Some((file, lines)) = seed else {
        return Outcome::ok(out);
    };
    if !created {
        out.push_str(&format!(
            "seed       not run: {file} fills a new branch tenant only, and this one was made \
             before\n"
        ));
        return Outcome::ok(out);
    }
    for (i, (op, params)) in lines.iter().enumerate() {
        // A new tenant, and its move to the branch's version, reach the door
        // within its rebuild and pin reading times; until then it refuses at
        // the door, before anything runs, or answers that the space is
        // closed while its data reaches the version (nothing written). Both
        // are asked again, for at most `SEED_WAIT`; any other answer is final.
        let started = std::time::Instant::now();
        let done = loop {
            match crate::wire::door_op_in(host, flags, &space, op, params.clone(), true).await {
                Err(o)
                    if (o.report.contains("[access_denied]")
                        || o.report.contains("[space_unavailable]"))
                        && started.elapsed() < SEED_WAIT =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                other => break other,
            }
        };
        if let Err(o) = done {
            out.push_str(&format!(
                "seed       stopped at line {} of {file} (`{op}`), {} of {} run:\n{}",
                i + 1,
                i,
                lines.len(),
                o.report
            ));
            return Outcome::failed(out);
        }
    }
    out.push_str(&format!("seeded     {} op(s) from {file}\n", lines.len()));
    Outcome::ok(out)
}

fn idle_words(secs: u64) -> String {
    match secs {
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3_600 == 0 => format!("{}h", s / 3_600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// `ordo branches list|delete`.
pub(crate) async fn run(flags: Flags) -> Outcome {
    let mut allowed = vec!["--reason"];
    allowed.extend(crate::operator::DOOR_FLAGS);
    if let Err(e) = flags.only(&allowed) {
        return Outcome::usage(format!("{e}\n{USAGE}\n"));
    }
    let host = match door_only(&flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    use ordo_host::branches as b;
    let args: Vec<&str> = flags.positional.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["list"] => {
            let row = match crate::wire::door_op(&host, &flags, b::BRANCHES, json!({}), false).await
            {
                Ok(r) => r,
                Err(o) => return o,
            };
            let idle = row["idleAfter"].as_u64().unwrap_or(b::IDLE_DEFAULT_SECS);
            let all = row["branches"].as_array().cloned().unwrap_or_default();
            let mut out = String::new();
            if let Some(p) = row["idleProblem"].as_str() {
                out.push_str(&format!("problem    {p}\n"));
            }
            if all.is_empty() {
                out.push_str("no branches\n");
                return Outcome::ok(out);
            }
            for r in &all {
                let s = |k: &str| r[k].as_str().unwrap_or_default().to_string();
                out.push_str(&format!(
                    "{:<24} {:<28} {:<10} applied {}{}\n",
                    s("branch"),
                    s("tenant"),
                    s("version"),
                    ordo_host::versions::utc_minute(r["appliedAt"].as_u64().unwrap_or(0)),
                    if r["idle"].as_bool() == Some(true) {
                        format!("  idle (unused {}+)", idle_words(idle))
                    } else {
                        String::new()
                    },
                ));
            }
            out.push_str(&format!(
                "{} branch(es). Idle after {} ({}). `ordo branches delete <name> --reason \
                 \"...\"` removes one.\n",
                all.len(),
                idle_words(idle),
                b::IDLE_SETTING
            ));
            Outcome::ok(out)
        }
        ["set", "idle-list-after", value] => {
            let Some(reason) = flags.named_value("--reason") else {
                return Outcome::usage(
                    "`--reason \"<text>\"` says why, and is kept with the change\n".into(),
                );
            };
            let row = match crate::wire::door_op(
                &host,
                &flags,
                b::SET_SETTING,
                json!({ "value": value, "reason": reason }),
                true,
            )
            .await
            {
                Ok(r) => r,
                Err(o) => return o,
            };
            Outcome::ok(format!(
                "set        {} is {} (kept with who, when and why)\n",
                b::IDLE_SETTING,
                idle_words(row["idleAfter"].as_u64().unwrap_or(b::IDLE_DEFAULT_SECS))
            ))
        }
        ["delete", name] => {
            let Some(reason) = flags.named_value("--reason") else {
                return Outcome::usage(
                    "`--reason \"<text>\"` says why, and is kept with the change\n".into(),
                );
            };
            let row = match crate::wire::door_op(
                &host,
                &flags,
                b::DELETE_BRANCH,
                json!({ "branch": name, "reason": reason }),
                true,
            )
            .await
            {
                Ok(r) => r,
                Err(o) => return o,
            };
            let s = |k: &str| row["branch"][k].as_str().unwrap_or_default().to_string();
            Outcome::ok(format!(
                "deleted    branch {} (tenant {}, {}): its database dropped, its keys destroyed\n",
                s("branch"),
                s("tenant"),
                s("space")
            ))
        }
        _ => Outcome::usage(format!("{USAGE}\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_file_is_one_op_per_line_and_a_bad_line_is_named() {
        let dir = std::env::temp_dir().join(format!("ordo-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.jsonl");
        std::fs::write(
            &good,
            "# notes\n{\"op\": \"createNote\", \"params\": {\"title\": \"a\"}}\n\n{\"op\": \"x\"}\n",
        )
        .unwrap();
        let lines = seed_lines(good.to_str().unwrap()).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].0, "createNote");
        assert_eq!(lines[1].1, json!({}));
        let bad = dir.join("bad.jsonl");
        std::fs::write(&bad, "{\"params\": {}}\n").unwrap();
        let e = seed_lines(bad.to_str().unwrap()).unwrap_err();
        assert!(e.contains("line 1"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_idle_setting_reads_back_in_its_own_units() {
        assert_eq!(idle_words(30 * 86_400), "30d");
        assert_eq!(idle_words(7_200), "2h");
        assert_eq!(idle_words(90), "90s");
    }
}
