//! The settings screens' route (`plan-console.md` 4.2, 4.6 and 4.10,
//! step 6).
//!
//! | Route | Reads |
//! |---|---|
//! | `GET settings` | `std.deployment.settings` at the door (G7), and the console's live runtime revision |
//!
//! One answer for both screens, Sign-in settings and Every setting: each
//! setting as the deployment took it (value, default, range, source; a
//! secret only as set or unset), what the console's live revision gives
//! it, and **drift** where the two differ.
//!
//! A change is not made here. The page sends it to `runtime/apply`, which
//! files a new revision and restarts the deployment with it, behind the
//! console owner's touch, with the automatic rollback (`crate::runtime`).
//! `sealed` says a change to it goes under `secrets`, sealed by the console.
//! In door-only mode there is no revision and no apply: the answer says
//! `editable: false`, and the screens are read-only.
//!
//! Either half may be missing: a deployment that is down or not linked
//! still shows the console's revision, and a door-only console still shows
//! what the door says. Drift needs both.

use std::collections::BTreeMap;

use super::*;
use crate::runtime::spec::{is_secret, SecretRef, Spec, APPLIED_BY_ENV, REVISION_ENV};

read!(settings_route, settings_get);

/// The door op (G7).
pub(super) const SETTINGS_OP: &str = "std.deployment.settings";

/// The settings route, under `/d/<handle>/api/settings`.
pub(super) fn routes() -> Router<Console> {
    Router::new().route("/d/{handle}/api/settings", get(settings_route))
}

async fn settings_get(
    c: &Console,
    call: &mut Call,
    headers: &HeaderMap,
) -> Result<Reply, ConsoleError> {
    let who = c.who(call, headers, Need::Signed).await?;
    let (door_rows, door_error) = match who.door(c) {
        Ok(link) => match link.op(SETTINGS_OP, true, &json!({})).await {
            Ok(rows) => (Some(rows), None),
            Err(e) => (None, Some(e.to_string())),
        },
        Err(e) => (None, Some(e.to_string())),
    };
    let (revision, spec) = match &c.0.runtime {
        Some(sup) => {
            let (n, spec) = sup.current().await?;
            (Some(n), Some(spec))
        }
        None => (None, None),
    };
    Ok(Reply::json(json!({
        "mode": if spec.is_some() { "supervise" } else { "door-only" },
        "editable": spec.is_some(),
        "revision": revision,
        "doorError": door_error,
        "secretSettings": crate::runtime::spec::SECRET_SETTINGS,
        "rows": merge(door_rows.as_deref(), spec.as_ref()),
    })))
}

/// The variables the supervisor sets on the deployment itself for each
/// start: the revision label and the identity wiring. Never drift.
fn console_own(name: &str) -> bool {
    name == REVISION_ENV || name == APPLIED_BY_ENV || name.starts_with("ORDO_IDENTITY_")
}

/// What the console's revision gives a setting, for the screen. A secret
/// is named with its kind and when it changed, never its value or path.
fn console_side(name: &str, spec: &Spec) -> Value {
    if let Some(v) = spec.settings.get(name) {
        // A value the deployment never shows is not shown here either.
        if ordo_env::hides_value(name) {
            return json!({ "set": true });
        }
        return json!({ "set": true, "value": v });
    }
    match spec.secrets.get(name) {
        Some(SecretRef::Managed { changed_at, .. } | SecretRef::Held { changed_at }) => {
            json!({ "set": true, "secret": "managed", "changedAt": changed_at })
        }
        Some(SecretRef::Referenced { changed_at, .. }) => {
            json!({ "set": true, "secret": "referenced", "changedAt": changed_at })
        }
        None => json!({ "set": false }),
    }
}

/// Whether the door's row and the console's revision disagree, and how.
/// `None` is agreement, or nothing to compare.
fn drift(row: &Value, name: &str, spec: &Spec) -> Option<Value> {
    if console_own(name) || row["kind"] == "layer" {
        return None;
    }
    let source = row["source"].as_str().unwrap_or("default");
    let door_set = row["set"].as_bool().unwrap_or(false);
    let secret = row["secret"].as_bool().unwrap_or(false);
    let ours = spec.settings.get(name);
    let our_secret = spec.secrets.contains_key(name);
    let op = row["storedBy"].as_str().unwrap_or("its platform op");
    if source == "stored" {
        // A value a platform op stored wins over the variable.
        let differs = match ours {
            Some(v) => row["value"].as_str() != Some(v.as_str()),
            None => our_secret,
        };
        return differs.then(|| {
            json!({
                "kind": "stored",
                "message": format!(
                    "The deployment keeps the value {op} stored, which wins over {name} after \
                     the first boot. Change it with {op}, or this setting has no effect."
                ),
            })
        });
    }
    match (ours, our_secret) {
        (Some(_), _) if secret => (!door_set).then(|| {
            json!({ "kind": "not-taken", "message": format!(
                "The console's revision sets {name}, and the running deployment does not have it. \
                 Restart it, or check the revision came up.") })
        }),
        (Some(v), _) => {
            let door = row["value"].as_str();
            (door != Some(v.as_str())).then(|| {
                json!({ "kind": "not-taken", "message": match door {
                    Some(d) => format!(
                        "The console's revision sets {name} to \"{}\", and the running deployment has \"{}\". Restart it, or check the revision came up.",
                        short(v, 80), short(d, 80)),
                    None => format!(
                        "The console's revision sets {name} to \"{}\", and the running deployment does not have it. Restart it, or check the revision came up.",
                        short(v, 80)),
                } })
            })
        }
        (None, true) => (!door_set).then(|| {
            json!({ "kind": "not-taken", "message": format!(
                "The console's revision holds the secret {name}, and the running deployment does \
                 not have it. Restart it, or check the revision came up.") })
        }),
        (None, false) => door_set.then(|| {
            json!({ "kind": "outside", "message": format!(
                "The running deployment has {name} from {source}, and the console's revision does \
                 not set it: it was set outside the console.") })
        }),
    }
}

/// At most `max` characters of a value, for a message.
fn short(v: &str, max: usize) -> String {
    let mut s: String = v.chars().take(max).collect();
    if v.chars().count() > max {
        s.push('…');
    }
    s
}

/// The door's rows with the console's side and drift on each, plus a row
/// for each setting the revision gives that the door did not name. With
/// no door answer, the revision's settings alone, drift unknown.
pub(super) fn merge(door: Option<&[Value]>, spec: Option<&Spec>) -> Vec<Value> {
    let mut rows: Vec<Value> = door.map(<[Value]>::to_vec).unwrap_or_default();
    // A secret the console seals under a name the deployment does not know
    // as a secret reaches it as `NAME_FILE`, which the door names as it is.
    // It is the secret's row: set, from a file, its path not shown.
    if let Some(spec) = spec {
        for name in spec.secrets.keys() {
            if rows.iter().any(|r| r["name"] == name.as_str()) {
                continue;
            }
            let file = format!("{name}_FILE");
            if let Some(r) = rows.iter_mut().find(|r| r["name"] == file.as_str()) {
                r["name"] = json!(name);
                r["secret"] = json!(true);
                r["source"] = json!("file");
                r["var"] = json!(file);
                r["value"] = Value::Null;
            }
        }
    }
    let named: std::collections::BTreeSet<String> = rows
        .iter()
        .filter_map(|r| r["name"].as_str().map(str::to_string))
        .collect();
    if let Some(spec) = spec {
        for r in rows.iter_mut() {
            let Some(name) = r["name"].as_str().map(str::to_string) else {
                continue;
            };
            if r["kind"] == "layer" {
                continue;
            }
            r["sealed"] = json!(is_secret(&name));
            r["console"] = console_side(&name, spec);
            if door.is_some() {
                r["drift"] = drift(r, &name, spec).unwrap_or(Value::Null);
            }
        }
        let mut extra: BTreeMap<&str, Value> = BTreeMap::new();
        for name in spec.settings.keys().chain(spec.secrets.keys()) {
            if named.contains(name) || console_own(name) {
                continue;
            }
            let secret = spec.secrets.contains_key(name) || ordo_env::hides_value(name);
            let mut row = json!({
                "kind": "setting",
                "name": name,
                "group": "Not in the registry",
                "listed": false,
                "secret": secret,
                "sealed": is_secret(name),
                "set": Value::Null,
                "source": Value::Null,
                "console": console_side(name, spec),
                "drift": Value::Null,
            });
            if door.is_some() {
                row["set"] = json!(false);
                row["drift"] = json!({ "kind": "not-taken", "message": format!(
                    "The console's revision sets {name}, and the running deployment does not \
                     name it. It is not a setting the deployment reads, or the revision has not \
                     come up.") });
            }
            extra.insert(name, row);
        }
        rows.extend(extra.into_values());
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn door_row(name: &str, source: &str, value: Option<&str>, secret: bool) -> Value {
        json!({
            "kind": "setting", "name": name, "group": "g", "secret": secret,
            "set": source != "default", "source": source, "value": value,
            "storedBy": Value::Null,
        })
    }

    fn spec(settings: &[(&str, &str)], secrets: &[&str]) -> Spec {
        let mut s = Spec::default();
        for (k, v) in settings {
            s.settings.insert(k.to_string(), v.to_string());
        }
        for k in secrets {
            s.secrets.insert(
                k.to_string(),
                SecretRef::Managed {
                    version: 1,
                    changed_at: 9,
                },
            );
        }
        s
    }

    fn find<'a>(rows: &'a [Value], name: &str) -> &'a Value {
        rows.iter().find(|r| r["name"] == name).expect(name)
    }

    #[test]
    fn agreement_is_no_drift_and_a_difference_is() {
        let door = vec![
            door_row("ORDO_PG_PORT", "environment", Some("6543"), false),
            door_row("ORDO_PG_DB", "environment", Some("old"), false),
            door_row("ORDO_SESSION_SECS", "default", None, false),
            door_row("ORDO_CORS_ORIGINS", "environment", Some("https://x"), false),
        ];
        let s = spec(
            &[
                ("ORDO_PG_PORT", "6543"),
                ("ORDO_PG_DB", "new"),
                ("ORDO_SESSION_SECS", "600"),
            ],
            &[],
        );
        let rows = merge(Some(&door), Some(&s));
        assert!(find(&rows, "ORDO_PG_PORT")["drift"].is_null());
        assert_eq!(find(&rows, "ORDO_PG_DB")["drift"]["kind"], "not-taken");
        assert_eq!(
            find(&rows, "ORDO_SESSION_SECS")["drift"]["kind"],
            "not-taken"
        );
        assert_eq!(find(&rows, "ORDO_CORS_ORIGINS")["drift"]["kind"], "outside");
        assert_eq!(find(&rows, "ORDO_PG_PORT")["console"]["value"], "6543");
    }

    #[test]
    fn a_secret_drifts_only_on_set_or_unset_and_no_value_is_shown() {
        let door = vec![
            door_row("ORDO_TOKEN_KEY", "file", None, true),
            door_row("ORDO_PAGE_KEY", "default", None, true),
        ];
        let s = spec(&[], &["ORDO_TOKEN_KEY", "ORDO_PAGE_KEY"]);
        let rows = merge(Some(&door), Some(&s));
        assert!(find(&rows, "ORDO_TOKEN_KEY")["drift"].is_null());
        assert_eq!(find(&rows, "ORDO_PAGE_KEY")["drift"]["kind"], "not-taken");
        assert_eq!(
            find(&rows, "ORDO_TOKEN_KEY")["console"]["secret"],
            "managed"
        );
    }

    #[test]
    fn a_stored_value_that_wins_is_drift_naming_the_op() {
        let mut guest = door_row("ORDO_AUTH_GUEST", "stored", Some("off"), false);
        guest["storedBy"] = json!("std.deployment.setGuest");
        let s = spec(&[("ORDO_AUTH_GUEST", "on")], &[]);
        let rows = merge(Some(&[guest.clone()]), Some(&s));
        let d = &find(&rows, "ORDO_AUTH_GUEST")["drift"];
        assert_eq!(d["kind"], "stored");
        assert!(
            d["message"]
                .as_str()
                .unwrap()
                .contains("std.deployment.setGuest"),
            "{d}"
        );
        let same = spec(&[("ORDO_AUTH_GUEST", "off")], &[]);
        assert!(find(&merge(Some(&[guest]), Some(&same)), "ORDO_AUTH_GUEST")["drift"].is_null());
    }

    #[test]
    fn without_the_door_the_revision_shows_and_drift_is_unknown() {
        let s = spec(&[("ORDO_PG_PORT", "1")], &["ORDO_TOKEN_KEY"]);
        let rows = merge(None, Some(&s));
        assert_eq!(rows.len(), 2);
        assert!(find(&rows, "ORDO_PG_PORT")["drift"].is_null());
        assert!(find(&rows, "ORDO_TOKEN_KEY")["source"].is_null());
        // Door-only: the door's rows, with no console side.
        let door = vec![door_row("ORDO_PG_PORT", "environment", Some("1"), false)];
        let rows = merge(Some(&door), None);
        assert!(rows[0].get("console").is_none() && rows[0].get("drift").is_none());
    }

    #[test]
    fn a_secret_under_a_name_the_deployment_does_not_know_is_its_file_row() {
        let door = vec![door_row(
            "ORDO_FAKE_TOKEN_FILE",
            "environment",
            Some("/run/secrets/x"),
            false,
        )];
        let rows = merge(Some(&door), Some(&spec(&[], &["ORDO_FAKE_TOKEN"])));
        assert_eq!(rows.len(), 1, "{rows:?}");
        let r = find(&rows, "ORDO_FAKE_TOKEN");
        assert!(r["drift"].is_null(), "{r}");
        assert!(r["value"].is_null());
        assert!(!serde_json::to_string(&rows)
            .unwrap()
            .contains("/run/secrets"));
    }

    #[test]
    fn the_console_s_own_variables_never_drift() {
        let door = vec![door_row(REVISION_ENV, "environment", Some("4"), false)];
        let rows = merge(Some(&door), Some(&spec(&[], &[])));
        assert!(find(&rows, REVISION_ENV)["drift"].is_null());
    }
}
