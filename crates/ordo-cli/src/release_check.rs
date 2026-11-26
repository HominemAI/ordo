//! `ordo release-check --direct`: the release check, run from the new
//! image before its tag changes (`plan-versions.md` §2.8, ruled
//! 2026-11-23, Q8).
//!
//! ```text
//! docker run --rm -e ORDO_PG_HOST=... <new image> release-check --direct
//! ```
//!
//! The image's entrypoint, `ordo-supervisor`, hands `release-check` to this
//! command and starts nothing else.
//!
//! It reads the control database `ORDO_PG_*` (or `--db`) names, serves
//! nothing, changes no pin, and writes one `$release_checks` row. The
//! `--direct` use is recorded before it runs and reaches the security log
//! at the next boot. The work is `ordo_host::release_check`.
//!
//! Exit 0 when every version passes, 1 when one fails. `--override
//! --reason <why>` records the deployment owner's decision to upgrade
//! anyway and exits 0; the failing versions' tenants close at the boot,
//! and every other tenant serves. `--json` prints the report as JSON.

use crate::operator::{Flags, Outcome};

pub(crate) async fn run(flags: Flags, cfg: ordo_exec::PoolConfig) -> Outcome {
    if let Err(e) = flags.only(&["--db", "--json", "--override", "--reason"]) {
        return Outcome::usage(format!("{e}\n"));
    }
    if !flags.positional.is_empty() {
        return Outcome::usage(
            "ordo release-check --direct [--db <name>] [--json] [--override --reason <why>]\n"
                .into(),
        );
    }
    let overriding = flags.has("--override");
    let reason = flags
        .named_value("--reason")
        .unwrap_or("")
        .trim()
        .to_string();
    if !overriding && !reason.is_empty() {
        return Outcome::usage(
            "`--reason` goes with `--override`: it records why the upgrade goes ahead\n".into(),
        );
    }
    if overriding && reason.is_empty() {
        return Outcome::usage(
            "an override records why: add `--reason <why>`, for example `--reason \"security \
             fix\"`\n"
                .into(),
        );
    }
    let tried = format!("database `{}` on {}:{}", cfg.db, cfg.host, cfg.port);
    let pool = match ordo_exec::Pool::connect(cfg.clone()).await {
        Ok(p) => p,
        Err(e) => {
            return Outcome::failed(format!(
                "the {tried} did not answer ({e}). ORDO_PG_HOST, ORDO_PG_PORT, ORDO_PG_USER and \
                 ORDO_PG_DB name the control database; `--db <name>` picks another\n"
            ))
        }
    };
    let report = match ordo_host::release_check::run(&pool, &cfg).await {
        Ok(r) => r,
        Err(e) => return Outcome::failed(format!("the release check could not read: {e}\n")),
    };
    let actor = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .map(|u| format!("cli:{u}"))
        .unwrap_or_else(|_| "cli:unknown".into());
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut out = String::new();
    let recorded = if overriding {
        if report.passed() {
            return Outcome::usage(
                "every version passes, so there is nothing to override; run it without \
                 `--override`\n"
                    .into(),
            );
        }
        ordo_host::release_check::record_override(&pool, &report, &actor, &reason, at).await
    } else {
        ordo_host::release_check::record(
            &pool,
            &report,
            ordo_host::release_check::Act::Command,
            &actor,
            "",
            at,
        )
        .await
        .map_err(|e| e.to_string())
    };
    if let Err(e) = recorded {
        return Outcome::failed(format!(
            "the release check ran and was not recorded in `$release_checks` ({e})\n"
        ));
    }
    if flags.has("--json") {
        out.push_str(&format!("{}\n", report.to_json()));
    } else {
        for line in report.lines() {
            out.push_str(&line);
            out.push('\n');
        }
    }
    if overriding {
        out.push_str(&format!(
            "override recorded: Ordo {} goes ahead; the tenants named above close at its boot \
             ({reason})\n",
            report.release
        ));
        return Outcome::ok(out);
    }
    if report.passed() {
        Outcome::ok(out)
    } else {
        Outcome::failed(out)
    }
}
