//! `ordo windows set|delete|skip|unskip|list`: maintenance windows, set by
//! the deployment owner (owner 2026-11-04). Locally through
//! `ordo_host::windows::Windows`, the one client for them, or over the
//! door through `std.deployment.setWindow` and its siblings
//! (`ordo_host::window_ops`) with `--host`. Both paths print through the
//! same functions here.
//!
//! Times are UTC, written `2026-11-20T02:00Z`. A length is seconds or
//! `90m`, `2h`, `1h30m`.

use ordo_host::window_ops;
use ordo_host::windows::{self, Schedule, Skip, Target, Window, WindowSpec};
use serde_json::json;

use crate::operator::{Flags, Outcome};

const USAGE: &str =
    "ordo windows set [--id <id>] (--all | --only <tenant>... | --except <tenant>...)\n\
     \x20   (--starts <utc> --ends <utc> | --cron \"<m h dom mon dow>\" --length <len>)\n\
     \x20   --message \"<text>\"\n\
     ordo windows delete <id>\n\
     ordo windows skip <window> --starts <utc> --ends <utc> [--id <id>] [--reason \"<text>\"]\n\
     ordo windows unskip <skip-id>\n\
     ordo windows list [<tenant>]\n\
     each takes --host <h> [--token <t> --space <s>];\n\
     times are UTC, `2026-11-20T02:00Z`; a length is seconds or `90m`, `2h`, `1h30m`";

const DOOR_FLAGS: [&str; 3] = ["--host", "--token", "--space"];

pub async fn run(flags: Flags) -> Outcome {
    let Some(sub) = flags.positional.first().cloned() else {
        return Outcome::usage(format!("{USAGE}\n"));
    };
    let mut flags = flags;
    flags.positional.remove(0);
    match sub.as_str() {
        "set" => set(flags).await,
        "delete" => delete(flags).await,
        "skip" => skip(flags).await,
        "unskip" => unskip(flags).await,
        "list" => list(flags).await,
        other => Outcome::usage(format!(
            "unknown `ordo windows` subcommand `{other}`; there are `set`, `delete`, `skip`, \
             `unskip` and `list`\n{USAGE}\n"
        )),
    }
}

fn allow(flags: &Flags, own: &[&str]) -> Result<(), Outcome> {
    let mut allowed = vec!["--as"];
    allowed.extend(own);
    allowed.extend(DOOR_FLAGS);
    flags.only(&allowed).map_err(Outcome::usage)?;
    if flags.named_value("--as").is_some() {
        return Err(Outcome::usage(
            "`--as` names who acted on the database; over the door it is the token's \
             principal\n"
                .into(),
        ));
    }
    Ok(())
}

fn refused(e: windows::WindowRefusal) -> Outcome {
    Outcome::failed(format!("REFUSED  {e}\n"))
}

/// The tenants a repeatable flag names, each value split on commas.
fn tenants(flags: &Flags, flag: &str) -> Vec<String> {
    flags
        .all_values(flag)
        .into_iter()
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

async fn set(flags: Flags) -> Outcome {
    if let Err(o) = allow(
        &flags,
        &[
            "--id",
            "--all",
            "--only",
            "--except",
            "--starts",
            "--ends",
            "--cron",
            "--length",
            "--message",
        ],
    ) {
        return o;
    }
    if !flags.positional.is_empty() {
        return Outcome::usage(format!("{USAGE}\n"));
    }
    let only = tenants(&flags, "--only");
    let except = tenants(&flags, "--except");
    let (kind, list) = match (flags.has("--all"), only.is_empty(), except.is_empty()) {
        (true, true, true) => ("all", vec![]),
        (false, false, true) => ("only", only),
        (false, true, false) => ("except", except),
        _ => {
            return Outcome::usage(
                "a window covers `--all` tenants, `--only <tenant>...` or `--except \
                 <tenant>...`: name one\n"
                    .into(),
            )
        }
    };
    let target = match Target::parse(kind, &list) {
        Ok(t) => t,
        Err(e) => return refused(e),
    };
    let schedule = match (
        flags.named_value("--starts"),
        flags.named_value("--ends"),
        flags.named_value("--cron"),
        flags.named_value("--length"),
    ) {
        (Some(s), Some(e), None, None) => match (windows::parse_utc(s), windows::parse_utc(e)) {
            (Ok(start), Ok(end)) => Schedule::Range { start, end },
            (Err(e), _) | (_, Err(e)) => return refused(e),
        },
        (None, None, Some(c), Some(l)) => match windows::parse_length(l) {
            Ok(length) => Schedule::Cron {
                cron: c.trim().to_string(),
                length,
            },
            Err(e) => return refused(e),
        },
        _ => {
            return Outcome::usage(
                "a window is `--starts <utc> --ends <utc>`, or `--cron \"<expr>\" --length \
                 <len>`: one or the other\n"
                    .into(),
            )
        }
    };
    let message = match flags.require("--message") {
        Ok(m) => m.to_string(),
        Err(e) => return Outcome::usage(format!("{e}: a window carries a message\n")),
    };
    let spec = WindowSpec {
        id: flags.named_value("--id").map(str::to_string),
        target,
        schedule,
        message,
    };
    let host = match crate::operator::door_only(&flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    let mut params = json!({
        "id": spec.id,
        "target": spec.target.kind(),
        "message": spec.message,
    });
    if !spec.target.tenants().is_empty() {
        params["tenants"] = json!(spec.target.tenants());
    }
    match &spec.schedule {
        Schedule::Range { start, end } => {
            params["starts"] = windows::format_utc(*start).into();
            params["ends"] = windows::format_utc(*end).into();
        }
        Schedule::Cron { cron, length } => {
            params["cron"] = cron.clone().into();
            params["lengthSecs"] = (*length).into();
        }
    }
    let row = match crate::wire::door_op(&host, &flags, window_ops::SET_WINDOW, params, true).await
    {
        Ok(r) => r,
        Err(o) => return o,
    };
    match Window::from_json(&row) {
        Some(w) => Outcome::ok(window_set(&w)),
        None => Outcome::failed("REFUSED  the door's answer is not a window\n".into()),
    }
}

/// What `ordo windows set` prints, on both paths.
pub fn window_set(w: &Window) -> String {
    let when = match &w.schedule {
        Schedule::Range { start, end } => format!(
            "{} to {}",
            windows::format_utc(*start),
            windows::format_utc(*end)
        ),
        Schedule::Cron { cron, length } => {
            format!("cron `{cron}` for {}", windows::format_length(*length))
        }
    };
    let tenants = w.target.tenants();
    format!(
        "window     {}\nwhen       {when} UTC\ncovers     {}{}\n",
        w.id,
        w.target.kind(),
        if tenants.is_empty() {
            String::new()
        } else {
            format!(" {}", tenants.join(", "))
        }
    )
}

async fn delete(flags: Flags) -> Outcome {
    if let Err(o) = allow(&flags, &[]) {
        return o;
    }
    let [id] = flags.positional.as_slice() else {
        return Outcome::usage(format!("{USAGE}\n"));
    };
    let host = match crate::operator::door_only(&flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    return match crate::wire::door_op(
        &host,
        &flags,
        window_ops::DELETE_WINDOW,
        json!({ "id": id }),
        true,
    )
    .await
    {
        Ok(_) => Outcome::ok(format!("deleted    window {id}\n")),
        Err(o) => o,
    };
}

async fn skip(flags: Flags) -> Outcome {
    if let Err(o) = allow(&flags, &["--id", "--starts", "--ends", "--reason"]) {
        return o;
    }
    let [window] = flags.positional.as_slice() else {
        return Outcome::usage(format!("{USAGE}\n"));
    };
    let (start, end) = match (flags.require("--starts"), flags.require("--ends")) {
        (Ok(s), Ok(e)) => match (windows::parse_utc(s), windows::parse_utc(e)) {
            (Ok(s), Ok(e)) => (s, e),
            (Err(e), _) | (_, Err(e)) => return refused(e),
        },
        (Err(e), _) | (_, Err(e)) => return Outcome::usage(format!("{e}\n")),
    };
    let id = flags.named_value("--id");
    let reason = flags.named_value("--reason");
    let host = match crate::operator::door_only(&flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    let row = match crate::wire::door_op(
        &host,
        &flags,
        window_ops::SKIP_WINDOW,
        json!({
            "window": window,
            "id": id,
            "starts": windows::format_utc(start),
            "ends": windows::format_utc(end),
            "reason": reason,
        }),
        true,
    )
    .await
    {
        Ok(r) => r,
        Err(o) => return o,
    };
    match Skip::from_json(&row) {
        Some(s) => Outcome::ok(skip_set(&s)),
        None => Outcome::failed("REFUSED  the door's answer is not a skip\n".into()),
    }
}

/// What `ordo windows skip` prints, on both paths.
pub fn skip_set(s: &Skip) -> String {
    format!(
        "skip       {}\nwindow     {}\nskips      {} to {} UTC\n",
        s.id,
        s.window,
        windows::format_utc(s.start),
        windows::format_utc(s.end)
    )
}

async fn unskip(flags: Flags) -> Outcome {
    if let Err(o) = allow(&flags, &[]) {
        return o;
    }
    let [id] = flags.positional.as_slice() else {
        return Outcome::usage(format!("{USAGE}\n"));
    };
    let host = match crate::operator::door_only(&flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    return match crate::wire::door_op(
        &host,
        &flags,
        window_ops::DELETE_SKIP,
        json!({ "id": id }),
        true,
    )
    .await
    {
        Ok(_) => Outcome::ok(format!("deleted    skip {id}\n")),
        Err(o) => o,
    };
}

async fn list(flags: Flags) -> Outcome {
    if let Err(o) = allow(&flags, &[]) {
        return o;
    }
    let tenant = match flags.positional.as_slice() {
        [] => None,
        [t] => Some(t.as_str()),
        _ => return Outcome::usage(format!("{USAGE}\n")),
    };
    let host = match crate::operator::door_only(&flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    let row = match crate::wire::door_op(
        &host,
        &flags,
        window_ops::WINDOWS,
        json!({ "tenant": tenant }),
        false,
    )
    .await
    {
        Ok(r) => r,
        Err(o) => return o,
    };
    match windows::Listing::from_json(&row) {
        Ok(l) => Outcome::ok(l.render()),
        Err(e) => Outcome::failed(format!("REFUSED  {e}\n")),
    }
}
