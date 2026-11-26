//! **A move through a link that is not unique** (B14 with a named rule;
//! `forward-gaps` ask 7, owner: *"dev named"*).
//!
//! The link is a field of `many` naming a row of `one`, so one `one` row
//! has several `many` rows. A field moves either way:
//!
//! - **into `one`** (`street: text? prev(address.street)` in `person`):
//!   which `address` row gives the value is the rule's pick, `first` or
//!   `latest`, by a named field;
//! - **into `many`** (`city: text prev(person.city)` in `address`): which
//!   `address` rows take the value is the rule's pick, `all`, or the
//!   `first` or `latest` by a named field (the rest take the default).
//!
//! A rule may carry a condition, written in Ordo's own expression language
//! over `many`'s fields. Several rules are tried in order: a `many` row is
//! in the first rule whose condition it meets. Into `one`, the first rule
//! that has a row for a `one` row gives its value. A `many` row that meets
//! no rule gives or takes nothing; the pre-check lists such rows and the
//! move does not start while any remain.
//!
//! **One recompute for every writer.** The backfill worker and the dual
//! writes run the same statement over a set of `one` rows (a *group*): the
//! receiving rows of each group are set to what the rule gives them. Each
//! first locks the group's `one` rows (`FOR NO KEY UPDATE`, in key order),
//! then computes in a new statement, so the last writer to commit computes
//! over every write before it. The statement reads by key and by the
//! link's index, never a scan.
//!
//! Such a move is one way: the version after writes nothing back.

use serde_json::{json, Value};

use crate::{sql_lit, PoolError, TxConn};

/// Which rows of a group a rule pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Every row (into `many` only).
    All,
    /// The row with the smallest value of the rule's field.
    First,
    /// The row with the largest value of the rule's field.
    Latest,
}

impl Pick {
    pub fn as_str(self) -> &'static str {
        match self {
            Pick::All => "all",
            Pick::First => "first",
            Pick::Latest => "latest",
        }
    }

    pub fn parse(s: &str) -> Option<Pick> {
        match s {
            "all" => Some(Pick::All),
            "first" => Some(Pick::First),
            "latest" => Some(Pick::Latest),
            _ => None,
        }
    }
}

/// One rule, as the step renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRule {
    pub pick: Pick,
    /// The ordering column of `many`, for `first` and `latest`.
    pub by: Option<String>,
    /// The condition, rendered SQL over `many`'s own columns, unqualified.
    /// `None` meets every row.
    pub when: Option<String>,
}

/// A move through a link that is not unique, as `$renames` and the stored
/// plan hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickMove {
    pub one: String,
    pub many: String,
    /// The link, a column of `many`.
    pub link: String,
    /// The fields move from `one` into `many`.
    pub into_many: bool,
    /// The moved columns, one name in both tables.
    pub columns: Vec<String>,
    /// Each receiving column and its default (SQL), for a row that takes
    /// no value.
    pub cleared: Vec<(String, String)>,
    pub rules: Vec<PickRule>,
}

/// Ids per `IN` list.
const IDS: usize = 500;

fn q(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

const SP: &str = "\"$space\"";

impl PickMove {
    /// The entity the values come from.
    pub fn giving(&self) -> &str {
        if self.into_many {
            &self.one
        } else {
            &self.many
        }
    }

    /// The entity whose rows take them.
    pub fn receiving(&self) -> &str {
        if self.into_many {
            &self.many
        } else {
            &self.one
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "one": self.one, "many": self.many, "link": self.link,
            "intoMany": self.into_many, "columns": self.columns,
            "cleared": self.cleared.iter().map(|(c, d)| json!([c, d])).collect::<Vec<_>>(),
            "rules": self.rules.iter().map(|r| json!({
                "pick": r.pick.as_str(), "by": r.by, "when": r.when,
            })).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(v: &Value) -> Option<PickMove> {
        let strings = |k: &str| -> Option<Vec<String>> {
            v[k].as_array()?
                .iter()
                .map(|c| c.as_str().map(String::from))
                .collect()
        };
        Some(PickMove {
            one: v["one"].as_str()?.to_string(),
            many: v["many"].as_str()?.to_string(),
            link: v["link"].as_str()?.to_string(),
            into_many: v["intoMany"].as_bool()?,
            columns: strings("columns")?,
            cleared: v["cleared"]
                .as_array()?
                .iter()
                .map(|c| Some((c[0].as_str()?.to_string(), c[1].as_str()?.to_string())))
                .collect::<Option<Vec<_>>>()?,
            rules: v["rules"]
                .as_array()?
                .iter()
                .map(|r| {
                    Some(PickRule {
                        pick: Pick::parse(r["pick"].as_str()?)?,
                        by: r["by"].as_str().map(String::from),
                        when: r["when"].as_str().map(String::from),
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        })
    }

    /// The number of the first rule a row of `many` meets, or null.
    /// Inside `FROM many`, so the conditions' columns are its own.
    fn rule_case(&self) -> String {
        let arms = self
            .rules
            .iter()
            .enumerate()
            .map(|(i, r)| match &r.when {
                Some(w) => format!("WHEN COALESCE(({w}), false) THEN {}", i + 1),
                None => format!("WHEN TRUE THEN {}", i + 1),
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("CASE {arms} END")
    }

    /// The order within each rule: its field, then the id, ascending for
    /// `first` and descending for `latest`. A null sorts last either way.
    fn keys(&self) -> String {
        let mut keys = Vec::new();
        for (i, r) in self.rules.iter().enumerate() {
            let dir = if r.pick == Pick::Latest {
                "DESC"
            } else {
                "ASC"
            };
            if let Some(by) = &r.by {
                keys.push(format!(
                    "CASE WHEN r.\"$rule\" = {} THEN r.{} END {dir} NULLS LAST",
                    i + 1,
                    q(by)
                ));
            }
            keys.push(format!(
                "CASE WHEN r.\"$rule\" = {} THEN r.\"id\" END {dir} NULLS LAST",
                i + 1
            ));
        }
        keys.push("r.\"id\"".into());
        keys.join(", ")
    }

    /// The rows of `many` in the groups `groups` (a condition on `one` as
    /// `g`), each with its rule (`$rule`), its rank in its group (`$n`)
    /// and its rank in its rule within the group (`$m`).
    fn ranked(&self, groups: &str) -> String {
        let l = q(&self.link);
        format!(
            "SELECT r.*, \
             row_number() OVER (PARTITION BY r.{SP}, r.{l} ORDER BY r.\"$rule\" ASC NULLS LAST, \
             {keys}) AS \"$n\", \
             row_number() OVER (PARTITION BY r.{SP}, r.{l}, r.\"$rule\" ORDER BY {keys}) AS \"$m\" \
             FROM (SELECT m.*, {case} AS \"$rule\" FROM {many} m) r \
             WHERE EXISTS (SELECT 1 FROM {one} g WHERE g.{SP} = r.{SP} AND g.\"id\" = r.{l} \
             AND ({groups}))",
            keys = self.keys(),
            case = self.rule_case(),
            many = q(&self.many),
            one = q(&self.one),
        )
    }

    /// Each receiving row of the groups, `$s`, `$i`, then the value of each
    /// moved column the rule gives it.
    pub fn values(&self, groups: &str) -> String {
        let defaults: std::collections::BTreeMap<&str, &str> = self
            .cleared
            .iter()
            .map(|(c, d)| (c.as_str(), d.as_str()))
            .collect();
        let default = |c: &str| defaults.get(c).copied().unwrap_or("NULL").to_string();
        let l = q(&self.link);
        if self.into_many {
            let win = self
                .rules
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let paired = if r.pick == Pick::All {
                        "TRUE"
                    } else {
                        "k.\"$m\" = 1"
                    };
                    format!("WHEN {} THEN {paired}", i + 1)
                })
                .collect::<Vec<_>>()
                .join(" ");
            let cols = self
                .columns
                .iter()
                .map(|c| {
                    format!(
                        "CASE WHEN (CASE k.\"$rule\" {win} ELSE FALSE END) THEN g.{c} ELSE {d} END \
                         AS {c}",
                        c = q(c),
                        d = default(c)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "SELECT k.{SP} AS \"$s\", k.\"id\" AS \"$i\", {cols} FROM ({ranked}) k \
                 JOIN {one} g ON g.{SP} = k.{SP} AND g.\"id\" = k.{l} WHERE ({groups})",
                ranked = self.ranked(groups),
                one = q(&self.one),
            )
        } else {
            let cols = self
                .columns
                .iter()
                .map(|c| {
                    format!(
                        "CASE WHEN k.\"id\" IS NOT NULL THEN k.{c} ELSE {d} END AS {c}",
                        c = q(c),
                        d = default(c)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "SELECT g.{SP} AS \"$s\", g.\"id\" AS \"$i\", {cols} FROM {one} g \
                 LEFT JOIN ({ranked}) k ON k.{SP} = g.{SP} AND k.{l} = g.\"id\" \
                 AND k.\"$n\" = 1 AND k.\"$rule\" IS NOT NULL WHERE ({groups})",
                ranked = self.ranked(groups),
                one = q(&self.one),
            )
        }
    }

    fn cols(&self, prefix: &str) -> String {
        self.columns
            .iter()
            .map(|c| format!("{prefix}{}", q(c)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Set each receiving row of the groups to what the rule gives it,
    /// where it holds something else. `bump` raises the row's revision, as
    /// a write does.
    pub fn update_sql(&self, groups: &str, bump: bool) -> String {
        format!(
            "UPDATE {t} x SET ({c}) = ROW({vc}){rev} FROM ({values}) v \
             WHERE x.{SP} = v.\"$s\" AND x.\"id\" = v.\"$i\" \
             AND ROW({xc}) IS DISTINCT FROM ROW({vc})",
            t = q(self.receiving()),
            c = self.cols(""),
            vc = self.cols("v."),
            xc = self.cols("x."),
            rev = if bump {
                ", \"rev\" = x.\"rev\" + 1"
            } else {
                ""
            },
            values = self.values(groups),
        )
    }

    /// Lock the groups' `one` rows, in key order.
    pub fn lock_sql(&self, groups: &str) -> String {
        format!(
            "SELECT 1 FROM {one} g WHERE ({groups}) ORDER BY g.{SP}, g.\"id\" FOR NO KEY UPDATE",
            one = q(&self.one)
        )
    }

    /// The first receiving row, `(space, id)`, that holds other values than
    /// the rule gives it.
    pub fn mismatch_sql(&self) -> String {
        format!(
            "SELECT x.{SP}, x.\"id\" FROM {t} x JOIN ({values}) v \
             ON x.{SP} = v.\"$s\" AND x.\"id\" = v.\"$i\" \
             WHERE ROW({xc}) IS DISTINCT FROM ROW({vc}) ORDER BY x.{SP}, x.\"id\" LIMIT 1",
            t = q(self.receiving()),
            values = self.values("TRUE"),
            xc = self.cols("x."),
            vc = self.cols("v."),
        )
    }

    /// The rows of `many` that meet no rule, `(space, id, count of all)`,
    /// the first `limit` by key.
    pub fn unmatched_sql(&self, limit: usize) -> String {
        format!(
            "SELECT r.{SP}, r.\"id\"::text, count(*) OVER () FROM \
             (SELECT m.{SP}, m.\"id\", {case} AS \"$rule\" FROM {many} m) r \
             WHERE r.\"$rule\" IS NULL ORDER BY r.{SP}, r.\"id\" LIMIT {limit}",
            case = self.rule_case(),
            many = q(&self.many),
        )
    }

    /// Up to `limit` receiving rows and what the rule gives each: `(space,
    /// id, the giving row's id or null, the rule's number or null, values as
    /// text...)`.
    pub fn preview_sql(&self, limit: usize) -> String {
        let l = q(&self.link);
        let texts = self
            .columns
            .iter()
            .map(|c| format!("v.{}::text", q(c)))
            .collect::<Vec<_>>()
            .join(", ");
        let groups = format!(
            "(g.{SP}, g.\"id\") IN (SELECT {SP}, \"id\" FROM {one} ORDER BY {SP}, \"id\" \
             LIMIT {limit})",
            one = q(&self.one)
        );
        if self.into_many {
            format!(
                "SELECT v.\"$s\", v.\"$i\"::text, h.{l}::text, NULL::text, {texts} \
                 FROM ({values}) v JOIN {many} h ON h.{SP} = v.\"$s\" AND h.\"id\" = v.\"$i\" \
                 ORDER BY v.\"$s\", h.{l}, v.\"$i\" LIMIT {limit}",
                values = self.values(&groups),
                many = q(&self.many),
            )
        } else {
            format!(
                "SELECT v.\"$s\", v.\"$i\"::text, k.\"id\"::text, k.\"$rule\"::text, {texts} \
                 FROM ({values}) v LEFT JOIN ({ranked}) k ON k.{SP} = v.\"$s\" \
                 AND k.{l} = v.\"$i\" AND k.\"$n\" = 1 AND k.\"$rule\" IS NOT NULL \
                 ORDER BY v.\"$s\", v.\"$i\" LIMIT {limit}",
                values = self.values(&groups),
                ranked = self.ranked(&groups),
            )
        }
    }
}

/// The moves through a link that is not unique a version writes with: those
/// whose giving entity it still declares with the moved columns (the
/// version before). The version after writes nothing back.
pub fn pick_mirrors(
    schema: &ordo_schema::Schema,
    renames: &[crate::overlay::Rename],
) -> Vec<PickMove> {
    renames
        .iter()
        .filter(|r| r.is_merge())
        .filter_map(|r| {
            let v: Value = serde_json::from_str(r.split.as_deref()?).ok()?;
            let p = PickMove::from_json(&v["pick"])?;
            let first = p.columns.first()?;
            let giving = schema.native(p.giving())?;
            giving
                .fields
                .iter()
                .any(|f| f.column() == *first)
                .then_some(p)
        })
        .collect()
}

/// Run each move over the rows this write wrote and deleted, `(entity, row
/// id)`, in `space`, inside `tx`. `before` holds the link each written or
/// deleted row of `many` held before the write, from its before image. A
/// row the write's own statements returned is read by `ctid`
/// ([`crate::overlay::RowCapture`]); any other by key.
pub async fn apply_picks(
    tx: &TxConn,
    space: &str,
    moves: &[PickMove],
    written: &[(String, String)],
    before: &[(String, String)],
    captured: &crate::overlay::RowCapture,
) -> Result<(), PoolError> {
    for p in moves {
        let mut groups: Vec<String> = before
            .iter()
            .filter(|(e, _)| *e == p.many)
            .map(|(_, l)| l.clone())
            .collect();
        let ids = |table: &str| -> Vec<String> {
            let mut v: Vec<String> = written
                .iter()
                .filter(|(e, _)| e == table)
                .map(|(_, id)| id.clone())
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        // A row of `one` the version before writes gives its values to its
        // `many` rows.
        if p.into_many {
            groups.extend(ids(&p.one));
        }
        // A row of `many` it writes: its group, by its link now.
        let many = ids(&p.many);
        if !many.is_empty() {
            let refs: Vec<&str> = many.iter().map(String::as_str).collect();
            let (by_ctid, by_key) = captured.split_rows(&p.many, space, &refs);
            let mut ats = Vec::new();
            for chunk in by_ctid.chunks(IDS) {
                let tids = chunk
                    .iter()
                    .map(|(_, t)| sql_lit(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                ats.push(format!("ctid = ANY(ARRAY[{tids}]::tid[])"));
            }
            for chunk in by_key.chunks(IDS) {
                let list = chunk
                    .iter()
                    .map(|i| sql_lit(i))
                    .collect::<Vec<_>>()
                    .join(", ");
                ats.push(format!("{SP} = {} AND \"id\" IN ({list})", sql_lit(space)));
            }
            for at in ats {
                let rows = tx
                    .rows(&format!(
                        "SELECT {}::text FROM {} WHERE {at}",
                        q(&p.link),
                        q(&p.many)
                    ))
                    .await?;
                groups.extend(
                    rows.into_iter()
                        .filter_map(|r| r.into_iter().next().flatten()),
                );
            }
        }
        groups.sort_unstable();
        groups.dedup();
        for chunk in groups.chunks(IDS) {
            let list = chunk
                .iter()
                .map(|i| sql_lit(i))
                .collect::<Vec<_>>()
                .join(", ");
            let at = format!("g.{SP} = {} AND g.\"id\" IN ({list})", sql_lit(space));
            tx.rows(&p.lock_sql(&at)).await?;
            tx.rows(&p.update_sql(&at, true)).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latest() -> PickMove {
        PickMove {
            one: "person".into(),
            many: "address".into(),
            link: "person".into(),
            into_many: false,
            columns: vec!["street".into()],
            cleared: vec![("street".into(), "NULL".into())],
            rules: vec![
                PickRule {
                    pick: Pick::Latest,
                    by: Some("at".into()),
                    when: Some("(\"kind\" IS NOT DISTINCT FROM 'home')".into()),
                },
                PickRule {
                    pick: Pick::First,
                    by: Some("at".into()),
                    when: None,
                },
            ],
        }
    }

    #[test]
    fn a_pick_reads_back_as_it_was_written() {
        let p = latest();
        assert_eq!(PickMove::from_json(&p.to_json()), Some(p.clone()));
        let mut all = p;
        all.into_many = true;
        all.rules = vec![PickRule {
            pick: Pick::All,
            by: None,
            when: None,
        }];
        assert_eq!(PickMove::from_json(&all.to_json()), Some(all.clone()));
        assert_eq!(all.giving(), "person");
        assert_eq!(all.receiving(), "address");
    }

    #[test]
    fn the_rules_are_tried_in_order_and_ranked_by_their_fields() {
        let p = latest();
        let case = p.rule_case();
        assert!(
            case.starts_with("CASE WHEN COALESCE(((\"kind\" IS NOT DISTINCT FROM 'home')), false) THEN 1 WHEN TRUE THEN 2"),
            "{case}"
        );
        let keys = p.keys();
        assert!(keys.contains("r.\"at\" END DESC NULLS LAST"), "{keys}");
        assert!(keys.contains("r.\"at\" END ASC NULLS LAST"), "{keys}");
        let sql = p.update_sql("TRUE", true);
        assert!(sql.starts_with("UPDATE \"person\" x"), "{sql}");
        assert!(sql.contains("\"rev\" = x.\"rev\" + 1"), "{sql}");
    }
}
