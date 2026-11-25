//! **Grants a move killed stay killed after a revert** (`plan-versions.md`
//! §2.5, review V-53; increment 11b part 2).
//!
//! A capability grant (a share window, an app's consent) pins what each op
//! meant when it was made. The door decides from the token alone
//! (`ordo_identity::grants`), so when a tenant moves from X to Y and Y
//! widens an op, the grant is refused on Y, and when the tenant is moved
//! back in its soak the pins agree again and the grant would live again.
//! A grant that died at the switch must stay dead until the person
//! consents again, as a redeploy that widened an op kills it.
//!
//! So the switch of a move with dual writes records, in `$grant_kills` in
//! the control database, every live grant in the tenant that Y refuses,
//! judged as the door judges it. Every node reads the table with the moves
//! (`crate::pin_stream`), and the door refuses a recorded grant as
//! `GrantKilledByMove`, naming both versions and the fix (share again).
//! A grant made after the switch is never recorded, so a share made again
//! reads.
//!
//! This is the one place the door reads registry state for a capability
//! grant, in the direction `ordo_identity::grants` allows: a grant is
//! refused only where the table was read, and a node that has not read it
//! refuses on Y anyway.

use std::collections::{BTreeMap, HashMap};

use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_identity::OpPin;

/// The table, in the control database.
pub const DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$grant_kills\" ( \
  \"space\" text NOT NULL, \
  \"grant_id\" text NOT NULL, \
  \"from\" text NOT NULL, \
  \"to\" text NOT NULL, \
  \"at\" bigint NOT NULL, \
  PRIMARY KEY (\"space\", \"grant_id\") \
)";

/// Every grant a move killed: `(space, grant id)` to `(from, to)`.
pub type Kills = HashMap<(String, String), (String, String)>;

/// Whether a grant pinned as `pins` is refused by version `to`, whose ops
/// pin as `current`: the door's own test (`AccessEngine::pin_refusal`). A
/// pin made under another version meets the sharing rule; one made under
/// `to` or under no version is a redeploy, killed by any expansion.
pub fn refused_by(
    pins: &BTreeMap<String, OpPin>,
    to: &str,
    current: &BTreeMap<String, OpPin>,
) -> bool {
    pins.iter().any(|(op, pinned)| {
        let Some(now) = current.get(op) else {
            return true;
        };
        match pinned.version.as_deref() {
            Some(was) if was != to => ordo_identity::disagreement(pinned, now).is_some(),
            _ => ordo_identity::compare(pinned, now).is_fatal(),
        }
    })
}

/// **At a move's switch from `from` to `to`**: record every live grant in
/// `space` that `to` refuses. The grants made in the tenant are read from
/// the grant registry (`$capabilitygrant`); `to`'s ops from its stored
/// source. Returns the grants recorded. A registry not made yet holds none.
pub async fn record_at_switch(
    control: &Pool,
    space: &str,
    from: &str,
    to: &str,
    at: u64,
) -> Result<Vec<String>, String> {
    let rows = match control
        .rows(&format!(
            "SELECT \"grant_id\", \"pin\" FROM \"$capabilitygrant\" \
             WHERE \"$space\" = {} AND \"revoked_at\" IS NULL AND \"expires\" > {at}",
            sql_lit(space)
        ))
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_missing_table() => return Ok(Vec::new()),
        Err(e) => return Err(format!("the grant registry: {e}")),
    };
    let mut grants: BTreeMap<String, BTreeMap<String, OpPin>> = BTreeMap::new();
    for r in rows {
        let mut it = r.into_iter();
        let (Some(Some(id)), Some(Some(pin))) = (it.next(), it.next()) else {
            continue;
        };
        let pins = grants.entry(id).or_default();
        if let Some(one) = ordo_identity::decode_pins(&pin) {
            pins.extend(one);
        }
    }
    if grants.is_empty() {
        return Ok(Vec::new());
    }
    let artifact = crate::versions::Versions::on(control)
        .get(to)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("version `{to}` is not stored"))?;
    let fixture = crate::Fixture::from_source(&artifact.source, "")
        .map_err(|e| format!("version `{to}` does not compile here: {e}"))?;
    let mut current = BTreeMap::new();
    for s in crate::build_specs(&fixture) {
        if let Some(mut pin) = crate::op_pin(&fixture, &s.name) {
            pin.version = Some(to.to_string());
            current.insert(s.name, pin);
        }
    }
    let killed: Vec<String> = grants
        .into_iter()
        // A pin with no op shape (a grant from before pins) cannot be
        // judged, and the door does not judge it either.
        .filter(|(_, pins)| !pins.is_empty() && refused_by(pins, to, &current))
        .map(|(id, _)| id)
        .collect();
    if killed.is_empty() {
        return Ok(killed);
    }
    control.rows(DDL).await.map_err(|e| e.to_string())?;
    let values: Vec<String> = killed
        .iter()
        .map(|id| {
            format!(
                "({}, {}, {}, {}, {at})",
                sql_lit(space),
                sql_lit(id),
                sql_lit(from),
                sql_lit(to)
            )
        })
        .collect();
    control
        .rows(&format!(
            "INSERT INTO \"$grant_kills\" (\"space\", \"grant_id\", \"from\", \"to\", \"at\") \
             VALUES {} ON CONFLICT (\"space\", \"grant_id\") DO NOTHING",
            values.join(", ")
        ))
        .await
        .map_err(|e| e.to_string())?;
    Ok(killed)
}

/// Every recorded kill, for a node's read.
pub async fn read_all(control: &Pool) -> Result<Kills, PoolError> {
    let rows = match control
        .rows("SELECT \"space\", \"grant_id\", \"from\", \"to\" FROM \"$grant_kills\"")
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_missing_table() => return Ok(Kills::new()),
        Err(e) => return Err(e),
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let mut it = r.into_iter();
            Some(((it.next()??, it.next()??), (it.next()??, it.next()??)))
        })
        .collect())
}
