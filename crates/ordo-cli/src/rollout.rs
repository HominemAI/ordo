//! `ordo rollout`: rollouts through the door (`plan-versions.md` §3.7,
//! increment 10). Each subcommand calls one `std.deployment.rollout.*` op,
//! or `setVersionSetting` for a setting. None opens a database connection
//! (owner, 2026-11-05: the CLI goes through the door). The rollout runs in
//! the host; a closed terminal stops nothing.

use crate::operator::{door_only, Flags, Outcome};

pub(crate) const USAGE: &str = "\
ordo rollout start <version> [--strategy rolling|all|canary|group|query] [--batch-interval <5m>]
                   [--window <1h>] [--failure-threshold <n>] [--deployment-space first|last]
                   [--batch-size <n>] [--canary-count <n>] [--canary-wait <1h>]
                   [--group <tenant,tenant>] [--query \"members < 5\"] [--order \"members asc\"] [--limit <n>]
                   [--health-error-rate <1%>] [--health-min-calls <20>]
                   [--alert-channel stream|mail:<address>]
                   [--share-partners]
    start a rollout of a stored version. One runs at a time. canary, group and query move
    a first stage, then the rest once it has been healthy for --canary-wait, with at least
    --health-min-calls calls; fewer pause it. A query reads
    the tenant facts: tenant, name, version, createdAt, ageDays, members, requests30d,
    writes30d, personal, ownDatabase, paused, and rows.<entity>. --limit keeps the first n:
    --query true --order \"requests_30d asc\" --limit 5 moves the 5 least used first.
ordo rollout status [--id <n>]
    the open rollout, or the newest, and where each tenant stands.
ordo rollout resume
    resume a paused rollout.
ordo rollout skip <tenant> [--reason \"...\"]
    leave one tenant where it is.
ordo rollout retry <tenant>
    try one failed tenant again, or a tenant's refused move.
ordo rollout abort --reason \"...\"
    stop the rollout. Moved tenants stay; `ordo rollback` moves them back.
ordo rollout finish [--reason \"...\"]
    close the rollout by hand, the rest skipped.
ordo rollout hold <tenant> --reason \"...\" | --clear
    hold one tenant's moves, or lift the hold.
ordo rollout settings
    the rollout settings.
ordo rollout set <setting> <value> --reason \"...\" | ordo rollout set <setting> --clear --reason \"...\"
    change one: rollout.strategy, rollout.batch_interval, rollout.window,
    rollout.failure_threshold, rollout.deployment_space, rollout.canary_count,
    rollout.canary_wait, rollout.health_error_rate, rollout.health_min_calls, rollout.alert_channel,
    rollout.share_partners, rollout.group, rollout.query, rollout.order, rollout.limit.
Every subcommand takes --host <h> (or ORDO_HOST), and --token or a signed-in session.";

/// `ordo rollout`.
pub(crate) async fn run(flags: Flags) -> Outcome {
    let mut allowed = vec![
        "--strategy",
        "--batch-interval",
        "--window",
        "--failure-threshold",
        "--deployment-space",
        "--batch-size",
        "--canary-count",
        "--canary-wait",
        "--health-error-rate",
        "--health-min-calls",
        "--alert-channel",
        "--share-partners",
        "--group",
        "--query",
        "--order",
        "--limit",
        "--id",
        "--reason",
        "--clear",
    ];
    allowed.extend(crate::operator::DOOR_FLAGS);
    if let Err(e) = flags.only(&allowed) {
        return Outcome::usage(format!("{e}\n{USAGE}\n"));
    }
    let Some(sub) = flags.positional.first().cloned() else {
        return Outcome::usage(format!("{USAGE}\n"));
    };
    let args: Vec<String> = flags.positional[1..].to_vec();
    let need = |n: usize, what: &str| -> Result<(), Outcome> {
        if args.len() == n {
            Ok(())
        } else {
            Err(Outcome::usage(format!("{what}\n{USAGE}\n")))
        }
    };
    let (op, params) = match sub.as_str() {
        "start" => {
            if let Err(o) = need(1, "name the version: `ordo rollout start <version>`") {
                return o;
            }
            let int = |name: &str| -> Result<Option<i64>, Outcome> {
                match flags.named_value(name) {
                    None => Ok(None),
                    Some(v) => v.trim().parse::<i64>().map(Some).map_err(|_| {
                        Outcome::usage(format!("`{name}` is a whole number, not `{v}`\n"))
                    }),
                }
            };
            let (threshold, size) = match (int("--failure-threshold"), int("--batch-size")) {
                (Ok(t), Ok(s)) => (t, s),
                (Err(o), _) | (_, Err(o)) => return o,
            };
            let canary_count = match int("--canary-count") {
                Ok(n) => n,
                Err(o) => return o,
            };
            let limit = match int("--limit") {
                Ok(n) => n,
                Err(o) => return o,
            };
            let min_calls = match int("--health-min-calls") {
                Ok(n) => n,
                Err(o) => return o,
            };
            (
                ordo_host::rollouts::START,
                serde_json::json!({
                    "version": args[0],
                    "strategy": flags.named_value("--strategy"),
                    "batchInterval": flags.named_value("--batch-interval"),
                    "window": flags.named_value("--window"),
                    "failureThreshold": threshold,
                    "deploymentSpace": flags.named_value("--deployment-space"),
                    "batchSize": size,
                    "canaryCount": canary_count,
                    "canaryWait": flags.named_value("--canary-wait"),
                    "healthErrorRate": flags.named_value("--health-error-rate"),
                    "healthMinCalls": min_calls,
                    "alertChannel": flags.named_value("--alert-channel"),
                    "sharePartners": flags.has("--share-partners").then_some(true),
                    "group": flags.named_value("--group"),
                    "query": flags.named_value("--query"),
                    "order": flags.named_value("--order"),
                    "limit": limit,
                }),
            )
        }
        "status" => {
            let id = match flags.named_value("--id") {
                None => None,
                Some(v) => match v.trim().parse::<i64>() {
                    Ok(n) => Some(n),
                    Err(_) => {
                        return Outcome::usage(format!("`--id` is a rollout's number, not `{v}`\n"))
                    }
                },
            };
            (ordo_host::rollouts::STATUS, serde_json::json!({ "id": id }))
        }
        "resume" => (ordo_host::rollouts::RESUME, serde_json::json!({})),
        "skip" => {
            if let Err(o) = need(1, "name the tenant: `ordo rollout skip <tenant>`") {
                return o;
            }
            (
                ordo_host::rollouts::SKIP,
                serde_json::json!({ "tenant": args[0], "reason": flags.named_value("--reason") }),
            )
        }
        "retry" => {
            if let Err(o) = need(1, "name the tenant: `ordo rollout retry <tenant>`") {
                return o;
            }
            (
                ordo_host::rollouts::RETRY,
                serde_json::json!({ "tenant": args[0] }),
            )
        }
        "abort" => {
            let Some(reason) = flags.named_value("--reason") else {
                return Outcome::usage(format!("an abort says why: `--reason \"...\"`\n{USAGE}\n"));
            };
            (
                ordo_host::rollouts::ABORT,
                serde_json::json!({ "reason": reason }),
            )
        }
        "finish" => (
            ordo_host::rollouts::FINISH,
            serde_json::json!({ "reason": flags.named_value("--reason") }),
        ),
        "hold" => {
            if let Err(o) = need(1, "name the tenant: `ordo rollout hold <tenant>`") {
                return o;
            }
            let clear = flags.has("--clear");
            if !clear && flags.named_value("--reason").is_none() {
                return Outcome::usage(format!(
                    "a hold says why: `--reason \"...\"`, or `--clear` lifts it\n{USAGE}\n"
                ));
            }
            (
                ordo_host::rollouts::HOLD,
                serde_json::json!({
                    "tenant": args[0], "reason": flags.named_value("--reason"), "clear": clear,
                }),
            )
        }
        "settings" => (
            ordo_host::deploy_ops::VERSION_SETTINGS,
            serde_json::json!({}),
        ),
        "set" => {
            let clear = flags.has("--clear");
            let ok = if clear {
                args.len() == 1
            } else {
                args.len() == 2
            };
            if !ok {
                return Outcome::usage(format!(
                    "`ordo rollout set <setting> <value> --reason \"...\"`, or `--clear` with no \
                     value\n{USAGE}\n"
                ));
            }
            let Some(setting) = ordo_host::rollouts::setting_name(&args[0]) else {
                return Outcome::usage(format!(
                    "`{}` is not a rollout setting; there are {}\n",
                    args[0],
                    ordo_host::rollouts::SETTINGS.join(", ")
                ));
            };
            let Some(reason) = flags.named_value("--reason") else {
                return Outcome::usage(format!("a change says why: `--reason \"...\"`\n{USAGE}\n"));
            };
            (
                ordo_host::deploy_ops::SET_VERSION_SETTING,
                serde_json::json!({
                    "setting": setting,
                    "value": if clear { None } else { args.get(1).cloned() },
                    "reason": reason,
                }),
            )
        }
        other => {
            return Outcome::usage(format!("unknown subcommand `{other}`\n{USAGE}\n"));
        }
    };
    let host = match door_only(&flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    let write = op != ordo_host::rollouts::STATUS && op != ordo_host::deploy_ops::VERSION_SETTINGS;
    let row = match crate::wire::door_op(&host, &flags, op, params, write).await {
        Ok(r) => r,
        Err(o) => return o,
    };
    Outcome::ok(match sub.as_str() {
        "skip" => format!(
            "skipped    {} in rollout {}\n",
            row["tenant"].as_str().unwrap_or("?"),
            row["rollout"]
        ),
        "retry" => match row["rollout"].as_i64() {
            Some(id) => format!(
                "queued     {} again in rollout {id}\n",
                row["tenant"].as_str().unwrap_or("?")
            ),
            None => format!(
                "reopened  {}'s move to {}; the catch-up tries it again\n",
                row["tenant"].as_str().unwrap_or("?"),
                row["move"]["to"].as_str().unwrap_or("?")
            ),
        },
        "hold" => {
            if row["held"].as_bool() == Some(true) {
                format!(
                    "held       {}: {}\n",
                    row["tenant"].as_str().unwrap_or("?"),
                    row["reason"].as_str().unwrap_or("")
                )
            } else {
                format!("unheld     {}\n", row["tenant"].as_str().unwrap_or("?"))
            }
        }
        "settings" | "set" => settings(&row["rollout"]),
        _ => render(&row),
    })
}

fn settings(r: &serde_json::Value) -> String {
    let group: Vec<&str> = r["group"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| g.as_str())
        .collect();
    format!(
        "rollout.strategy           {}\n\
         rollout.batch_interval     {}\n\
         rollout.window             {}\n\
         rollout.failure_threshold  {}\n\
         rollout.deployment_space   {}\n\
         rollout.canary_count       {}\n\
         rollout.canary_wait        {}\n\
         rollout.health_error_rate  {}\n\
         rollout.health_min_calls   {}\n\
         rollout.alert_channel      {}\n\
         rollout.share_partners     {}\n\
         rollout.group              {}\n\
         rollout.query              {}\n\
         rollout.order              {}\n\
         rollout.limit              {}\n",
        match r["strategy"].as_str() {
            Some(s) => s.to_string(),
            None => format!(
                "unset: {} here ({})",
                r["strategyDefault"].as_str().unwrap_or("rolling"),
                if r["strategyDefault"].as_str() == Some("all") {
                    "this deployment is dev, so `ordo apply` moves every tenant at once"
                } else {
                    "`ordo apply` starts a rolling rollout; dev moves every tenant at once"
                }
            ),
        },
        ordo_host::windows::format_length(r["batchInterval"].as_i64().unwrap_or(0)),
        ordo_host::windows::format_length(r["window"].as_i64().unwrap_or(0)),
        r["failureThreshold"].as_i64().unwrap_or(0),
        r["deploymentSpace"].as_str().unwrap_or("?"),
        r["canaryCount"].as_i64().unwrap_or(0),
        ordo_host::windows::format_length(r["canaryWait"].as_i64().unwrap_or(0)),
        r["healthErrorRate"].as_str().unwrap_or("?"),
        r["healthMinCalls"].as_i64().unwrap_or(0),
        r["alertChannel"].as_str().unwrap_or("stream"),
        if r["sharePartners"].as_bool() == Some(true) {
            "on"
        } else {
            "off"
        },
        if group.is_empty() {
            "none".to_string()
        } else {
            group.join(",")
        },
        r["query"].as_str().unwrap_or("none"),
        r["order"].as_str().unwrap_or("by tenant id"),
        r["limit"]
            .as_i64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "none: every tenant the query selects".into()),
    )
}

/// A rollout and its tenants, as a person reads it.
pub(crate) fn render(row: &serde_json::Value) -> String {
    if row["id"].is_null() {
        return "rollout    none yet. `ordo rollout start <version>` starts one\n".into();
    }
    let n = |k: &str| row["counts"][k].as_u64().unwrap_or(0);
    let mut out = format!(
        "rollout    {} to {} (from {}), {}\nstate      {}{}\n",
        row["id"],
        row["target"].as_str().unwrap_or("?"),
        row["from"].as_str().unwrap_or("nothing"),
        row["strategy"].as_str().unwrap_or("?"),
        row["state"].as_str().unwrap_or("?"),
        match row["reason"].as_str().filter(|r| !r.is_empty()) {
            Some(r) => format!(": {r}"),
            None => String::new(),
        },
    );
    out.push_str(&format!(
        "batches    {} of up to {} tenant(s), every {}\n",
        row["batches"],
        row["batchSize"],
        ordo_host::windows::format_length(row["batchInterval"].as_i64().unwrap_or(0)),
    ));
    if let Some(sel) = row["selection"].as_str().filter(|s| !s.is_empty()) {
        out.push_str(&format!("first      {sel}\n"));
    }
    match row["gate"].as_str() {
        Some("pending") => out.push_str(&format!(
            "gate       the rest wait until the first stage has been healthy for {} \
             ({} of {} calls failed on the server; the limit is {}; it needs {} calls)\n",
            ordo_host::windows::format_length(row["canaryWait"].as_i64().unwrap_or(0)),
            row["health"]["faults"].as_u64().unwrap_or(0),
            row["health"]["calls"].as_u64().unwrap_or(0),
            row["healthErrorRate"].as_str().unwrap_or("?"),
            row["healthMinCalls"].as_i64().unwrap_or(0),
        )),
        Some("passed") => out.push_str("gate       the first stage was healthy; the rest move\n"),
        _ => {}
    }
    if let Some(step) = row["deploymentStep"].as_object() {
        out.push_str(&format!(
            "deployment its own space moves {}: {}\n",
            row["deploymentSpace"].as_str().unwrap_or("first"),
            step.get("state").and_then(|s| s.as_str()).unwrap_or("?"),
        ));
    }
    out.push_str(&format!(
        "tenants    {} moved, {} queued, {} waiting, {} moving, {} failed, {} skipped\n",
        n("moved"),
        n("queued"),
        n("waiting"),
        n("moving"),
        n("failed"),
        n("skipped"),
    ));
    if let Some(leader) = row["leader"].as_str() {
        out.push_str(&format!("leader     {leader}\n"));
    }
    for t in row["tenants"].as_array().into_iter().flatten() {
        let state = t["state"].as_str().unwrap_or("?");
        if matches!(state, "moved" | "queued") {
            continue;
        }
        out.push_str(&format!(
            "  {:<8} {}{}\n",
            state,
            t["tenant"].as_str().unwrap_or("?"),
            match t["reason"].as_str().filter(|r| !r.is_empty()) {
                Some(r) => format!(": {r}"),
                None => String::new(),
            }
        ));
    }
    out
}
