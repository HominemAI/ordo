//! `ordo tenant-builds`: a named build serves one stored version to the
//! tenants named, while the rest follow the active version
//! (`plan-versions.md` §3.8, increment 16). Each subcommand calls one door
//! op of `ordo_host::tenant_builds`; none opens a database.

use ordo_host::tenant_builds as tb;

use crate::operator::{door_only, numbered, Flags, Outcome};

pub(crate) const USAGE: &str = "\
ordo tenant-builds list
    every build, its version and the tenants it holds.
ordo tenant-builds set <name> <version> --reason \"...\"
    make a build, or set it to another version: every tenant it holds moves.
ordo tenant-builds join <name> <tenant>
    pin one tenant to the build's version. Activations and rollouts leave it there.
ordo tenant-builds leave <tenant>
    take one tenant out of its build, back to the active version.
ordo tenant-builds remove <name> --reason \"...\"
    remove a build that holds no tenant.
Every subcommand takes --host <h> (or ORDO_HOST), and --token or a signed-in session.";

/// `ordo tenant-builds`.
pub(crate) async fn run(flags: Flags) -> Outcome {
    let mut allowed = vec!["--reason"];
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
    let reason = || {
        flags
            .named_value("--reason")
            .map(str::to_string)
            .ok_or_else(|| {
                Outcome::usage(format!("a change says why: `--reason \"...\"`\n{USAGE}\n"))
            })
    };
    let (op, params) = match sub.as_str() {
        "list" => {
            if let Err(o) = need(0, "`ordo tenant-builds list` takes nothing more") {
                return o;
            }
            (tb::BUILDS, serde_json::json!({}))
        }
        "set" => {
            if let Err(o) = need(
                2,
                "`ordo tenant-builds set <name> <version> --reason \"...\"`",
            ) {
                return o;
            }
            let reason = match reason() {
                Ok(r) => r,
                Err(o) => return o,
            };
            (
                tb::SET_BUILD,
                serde_json::json!({ "name": args[0], "version": args[1], "reason": reason }),
            )
        }
        "join" => {
            if let Err(o) = need(2, "`ordo tenant-builds join <name> <tenant>`") {
                return o;
            }
            (
                tb::JOIN_BUILD,
                serde_json::json!({ "name": args[0], "tenant": args[1] }),
            )
        }
        "leave" => {
            if let Err(o) = need(1, "`ordo tenant-builds leave <tenant>`") {
                return o;
            }
            (tb::LEAVE_BUILD, serde_json::json!({ "tenant": args[0] }))
        }
        "remove" => {
            if let Err(o) = need(1, "`ordo tenant-builds remove <name> --reason \"...\"`") {
                return o;
            }
            let reason = match reason() {
                Ok(r) => r,
                Err(o) => return o,
            };
            (
                tb::REMOVE_BUILD,
                serde_json::json!({ "name": args[0], "reason": reason }),
            )
        }
        other => {
            return Outcome::usage(format!(
                "unknown `ordo tenant-builds` subcommand `{other}`; there are `list`, `set`, \
                 `join`, `leave` and `remove`\n{USAGE}\n"
            ))
        }
    };
    let host = match door_only(&flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    let row = match crate::wire::door_op(&host, &flags, op, params, op != tb::BUILDS).await {
        Ok(r) => r,
        Err(o) => return o,
    };
    Outcome::ok(report(&sub, &row))
}

/// What a subcommand printed, from the door's answer.
fn report(sub: &str, row: &serde_json::Value) -> String {
    let s = |v: &serde_json::Value| v.as_str().unwrap_or("?").to_string();
    let version = |r: &serde_json::Value| numbered(r["ordinal"].as_u64(), &s(&r["version"]));
    let tenants = |r: &serde_json::Value| -> Vec<String> {
        r["tenants"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t.as_str().map(str::to_string))
            .collect()
    };
    match sub {
        "list" => {
            let builds = row["builds"].as_array().cloned().unwrap_or_default();
            if builds.is_empty() {
                return "no tenant builds; every tenant follows the active version\n".into();
            }
            let mut out = String::new();
            for b in &builds {
                let held: Vec<&serde_json::Value> =
                    b["tenants"].as_array().into_iter().flatten().collect();
                out.push_str(&format!(
                    "build      {}  {}  {} tenant(s)  set by {} at {}: {}\n",
                    s(&b["name"]),
                    version(b),
                    held.len(),
                    s(&b["setBy"]),
                    ordo_host::versions::utc_minute(b["setAt"].as_u64().unwrap_or(0)),
                    s(&b["reason"]),
                ));
                for t in held {
                    out.push_str(&format!("  tenant   {}\n", s(&t["tenant"])));
                }
            }
            out
        }
        "set" => {
            let t = tenants(row);
            format!(
                "build      {} serves {}\nmoved      {} tenant(s){}\n",
                s(&row["build"]),
                version(row),
                t.len(),
                if t.is_empty() {
                    String::new()
                } else {
                    format!(": {}", t.join(", "))
                }
            )
        }
        "join" => format!(
            "joined     {} to build {} on {}\n",
            tenants(row).join(", "),
            s(&row["build"]),
            version(row)
        ),
        "leave" => format!(
            "left       {} out of build {}, now on {} (the active version)\n",
            tenants(row).join(", "),
            s(&row["build"]),
            version(row)
        ),
        _ => format!(
            "removed    build {} (it served {})\n",
            s(&row["build"]),
            s(&row["version"])
        ),
    }
}
