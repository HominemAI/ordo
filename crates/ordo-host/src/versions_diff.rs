//! **What changed between two stored versions**: `ordo versions diff` and
//! `std.deployment.diff` (`plan-versions.md` §4 row 2, review V-32).
//!
//! Two parts, each optional:
//!
//! - **The schema change**, from the first version to the second. It is
//!   the same `ordo_schema::diff` `ordo plan --against` runs, against the
//!   first version's DDL built in an empty shadow namespace
//!   ([`ordo_exec::shape::DIFF_SHADOW`]) instead of a live database. Each
//!   line is a change's taxonomy code, its words and its class, as the plan
//!   prints them. The namespace is dropped after.
//! - **The source**, as a unified diff of the two stored sources.
//!
//! One body for the CLI and the door, so both print the same answer: the
//! door returns [`VersionDiff::to_json`], and the CLI reads it back with
//! [`VersionDiff::from_json`] and renders it with the same function as the
//! direct path.

use serde_json::{json, Value};

use crate::schema_apply::Program;
use crate::versions::{VersionRefusal, Versions};
use ordo_exec::Pool;

/// Lines of context around each change in the source diff.
pub const CONTEXT: usize = 3;

/// The largest middle (after the common start and end) the source diff
/// compares line by line, in cells of its table. Past it, the middle is
/// shown as removed and added whole, which is still a true diff.
const MAX_CELLS: usize = 4_000_000;

/// One side of a diff, as a person reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    pub version: String,
    pub ordinal: Option<u64>,
    pub label: Option<String>,
}

impl Side {
    /// `#4 · 0.2.1`, or the name alone when the number is not known.
    pub fn named(&self) -> String {
        match self.ordinal {
            Some(n) => format!("#{n} · {}", self.version),
            None => self.version.clone(),
        }
    }
}

/// One schema change: its taxonomy code (`A2`), its words, its class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaLine {
    pub code: String,
    pub change: String,
    pub class: String,
}

/// What changed from `a` to `b`. A part not asked for is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionDiff {
    pub a: Side,
    pub b: Side,
    /// The schema changes, or why they could not be worked out (a stored
    /// source this Ordo release cannot compile).
    pub schema: Option<Result<Vec<SchemaLine>, String>>,
    /// The unified diff's lines, without line ends. Empty when the sources
    /// are the same.
    pub source: Option<Vec<String>>,
}

/// Diff two stored versions, each named by its name or `#n`. A name that
/// is not stored is refused naming the versions that are.
pub async fn diff(
    pool: &Pool,
    a: &str,
    b: &str,
    schema: bool,
    source: bool,
) -> Result<VersionDiff, VersionRefusal> {
    let v = Versions::on(pool);
    let a = v.resolve(a).await?;
    let b = v.resolve(b).await?;
    let (Some(stored_a), Some(stored_b)) = (v.get(&a).await?, v.get(&b).await?) else {
        // Resolved a moment ago, and versions are never deleted.
        return Err(VersionRefusal::UnknownVersion(a));
    };
    let side = |version: String| async {
        Ok::<_, VersionRefusal>(Side {
            ordinal: v.ordinal_of(&version).await?,
            label: v.provenance(&version).await?.label,
            version,
        })
    };
    let side_a = side(a).await?;
    let side_b = side(b).await?;
    let schema = if schema {
        Some(schema_changes(pool, &stored_a, &stored_b).await?)
    } else {
        None
    };
    let source = source.then(|| {
        unified(
            &stored_a.source,
            &stored_b.source,
            &side_a.named(),
            &side_b.named(),
            CONTEXT,
        )
    });
    Ok(VersionDiff {
        a: side_a,
        b: side_b,
        schema,
        source,
    })
}

/// The schema change from `a` to `b`: `b`'s schema diffed against `a`'s
/// DDL, built empty in a shadow namespace of its own.
async fn schema_changes(
    pool: &Pool,
    a: &crate::versions::Artifact,
    b: &crate::versions::Artifact,
) -> Result<Result<Vec<SchemaLine>, String>, VersionRefusal> {
    let compiled = |art: &crate::versions::Artifact| {
        Program::compile(&art.source).map_err(|e| {
            format!(
                "version `{}`'s stored source does not compile under this Ordo release: {e}",
                art.version
            )
        })
    };
    let (prior, declared) = match (compiled(a), compiled(b)) {
        (Ok(p), Ok(d)) => (p, d),
        (Err(e), _) | (_, Err(e)) => return Ok(Err(e)),
    };
    let mut nonce = [0u8; 8];
    getrandom::getrandom(&mut nonce)
        .map_err(|e| VersionRefusal::Store(ordo_exec::PoolError::db(format!("random: {e}"))))?;
    let ns = format!(
        "{}_{}",
        ordo_exec::shape::DIFF_SHADOW,
        nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let live = async {
        pool.build_shadow_in(&ns, &a.ddl).await?;
        ordo_exec::migrate::live_shape(pool, &ns).await
    }
    .await;
    // Dropped whatever happened; a failed drop leaves an empty namespace
    // and is not worth failing the answer for.
    let _ = pool
        .rows(&format!("DROP SCHEMA IF EXISTS {ns} CASCADE"))
        .await;
    let live = live?;
    Ok(Ok(ordo_schema::diff(
        &live,
        Some(&prior.schema),
        &declared.schema,
    )
    .iter()
    .map(|c| SchemaLine {
        code: c.code().to_string(),
        change: c.to_string(),
        class: c.class().to_string(),
    })
    .collect()))
}

impl VersionDiff {
    /// The op's result row. Hand-built, as [`crate::apply_op::Report`]'s
    /// is, because it is a wire format.
    pub fn to_json(&self) -> Value {
        let side =
            |s: &Side| json!({ "version": s.version, "ordinal": s.ordinal, "label": s.label });
        json!({
            "a": side(&self.a),
            "b": side(&self.b),
            "schema": match &self.schema {
                None => Value::Null,
                Some(Ok(lines)) => json!({ "changes": lines.iter().map(|l| json!({
                    "code": l.code, "change": l.change, "class": l.class,
                })).collect::<Vec<_>>() }),
                Some(Err(e)) => json!({ "error": e }),
            },
            "source": self.source,
        })
    }

    /// Read a diff back off the wire.
    pub fn from_json(v: &Value) -> Result<VersionDiff, String> {
        let side = |s: &Value| -> Result<Side, String> {
            Ok(Side {
                version: s["version"]
                    .as_str()
                    .ok_or("the diff's result named no version")?
                    .to_string(),
                ordinal: s["ordinal"].as_u64(),
                label: s["label"].as_str().map(str::to_string),
            })
        };
        let text = |l: &Value, k: &str| l[k].as_str().unwrap_or_default().to_string();
        let schema = match &v["schema"] {
            Value::Null => None,
            s if s["error"].is_string() => Some(Err(text(s, "error"))),
            s => Some(Ok(s["changes"]
                .as_array()
                .ok_or("the diff's schema part carried no changes")?
                .iter()
                .map(|l| SchemaLine {
                    code: text(l, "code"),
                    change: text(l, "change"),
                    class: text(l, "class"),
                })
                .collect())),
        };
        let source = match &v["source"] {
            Value::Null => None,
            s => Some(
                s.as_array()
                    .ok_or("the diff's source part is not a list of lines")?
                    .iter()
                    .map(|l| l.as_str().unwrap_or_default().to_string())
                    .collect(),
            ),
        };
        Ok(VersionDiff {
            a: side(&v["a"])?,
            b: side(&v["b"])?,
            schema,
            source,
        })
    }

    /// The diff as a person reads it. The one renderer, for both paths.
    pub fn render(&self) -> String {
        let mut out = format!("diff       {}  ->  {}\n", self.a.named(), self.b.named());
        for (s, word) in [(&self.a, "from"), (&self.b, "to")] {
            if let Some(l) = &s.label {
                out.push_str(&format!("label      {word} \"{l}\"\n"));
            }
        }
        match &self.schema {
            None => {}
            Some(Err(e)) => out.push_str(&format!("\nSCHEMA  not worked out: {e}\n")),
            Some(Ok(lines)) if lines.is_empty() => out.push_str("\nSCHEMA  no change\n"),
            Some(Ok(lines)) => {
                out.push_str(&format!("\nSCHEMA  ({} change(s))\n", lines.len()));
                for l in lines {
                    out.push_str(&format!(
                        "  {:<3} {}  (class {})\n",
                        l.code, l.change, l.class
                    ));
                }
            }
        }
        match &self.source {
            None => {}
            Some(lines) if lines.is_empty() => out.push_str("\nSOURCE  no change\n"),
            Some(lines) => {
                out.push_str("\nSOURCE\n");
                for l in lines {
                    out.push_str(l);
                    out.push('\n');
                }
            }
        }
        out
    }
}

// ── the source diff ─────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Gone,
    New,
}

/// A unified diff of `a` and `b`, `context` lines around each change. Empty
/// when they are the same.
pub fn unified(a: &str, b: &str, a_name: &str, b_name: &str, context: usize) -> Vec<String> {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let ops = edit_script(&a, &b);
    if ops.iter().all(|o| *o == Op::Same) {
        return Vec::new();
    }
    // Where each op starts in `a` and `b`.
    let mut at = Vec::with_capacity(ops.len() + 1);
    let (mut i, mut j) = (0usize, 0usize);
    for o in &ops {
        at.push((i, j));
        match o {
            Op::Same => {
                i += 1;
                j += 1;
            }
            Op::Gone => i += 1,
            Op::New => j += 1,
        }
    }
    at.push((i, j));

    let changed: Vec<usize> = (0..ops.len()).filter(|&k| ops[k] != Op::Same).collect();
    let mut out = vec![format!("--- {a_name}"), format!("+++ {b_name}")];
    let mut k = 0;
    while k < changed.len() {
        // One hunk: changes no more than 2 * context apart.
        let first = changed[k];
        let mut last = first;
        while k + 1 < changed.len() && changed[k + 1] - last <= 2 * context + 1 {
            k += 1;
            last = changed[k];
        }
        k += 1;
        let start = first.saturating_sub(context);
        let end = (last + context + 1).min(ops.len());
        let (a0, b0) = at[start];
        let (a1, b1) = at[end];
        let range = |from: usize, n: usize| {
            if n == 0 {
                format!("{from},0")
            } else if n == 1 {
                format!("{}", from + 1)
            } else {
                format!("{},{n}", from + 1)
            }
        };
        out.push(format!(
            "@@ -{} +{} @@",
            range(a0, a1 - a0),
            range(b0, b1 - b0)
        ));
        for (o, &(ai, bi)) in ops[start..end].iter().zip(&at[start..end]) {
            out.push(match o {
                Op::Same => format!(" {}", a[ai]),
                Op::Gone => format!("-{}", a[ai]),
                Op::New => format!("+{}", b[bi]),
            });
        }
    }
    out
}

/// The shortest edit script from `a` to `b` by longest common
/// subsequence, over the part between their common start and end.
fn edit_script(a: &[&str], b: &[&str]) -> Vec<Op> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut ops = vec![Op::Same; pre];
    let (n, m) = (am.len(), bm.len());
    if n.saturating_mul(m) > MAX_CELLS {
        ops.extend(std::iter::repeat_n(Op::Gone, n));
        ops.extend(std::iter::repeat_n(Op::New, m));
    } else {
        // lcs[i][j]: the common subsequence of am[i..] and bm[j..].
        let w = m + 1;
        let mut lcs = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * w + j] = if am[i] == bm[j] {
                    lcs[(i + 1) * w + j + 1] + 1
                } else {
                    lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if am[i] == bm[j] {
                ops.push(Op::Same);
                i += 1;
                j += 1;
            } else if lcs[(i + 1) * w + j] >= lcs[i * w + j + 1] {
                ops.push(Op::Gone);
                i += 1;
            } else {
                ops.push(Op::New);
                j += 1;
            }
        }
        ops.extend(std::iter::repeat_n(Op::Gone, n - i));
        ops.extend(std::iter::repeat_n(Op::New, m - j));
    }
    ops.extend(std::iter::repeat_n(Op::Same, suf));
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_text_has_no_diff() {
        assert!(unified("a\nb\n", "a\nb\n", "x", "y", 3).is_empty());
    }

    #[test]
    fn a_changed_line_reads_as_a_unified_diff() {
        let a = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let b = "1\n2\n3\n4\nfive\n6\n7\n8\n9\n";
        assert_eq!(
            unified(a, b, "#1 · 0.1.0", "#2 · 0.2.0", 3),
            vec![
                "--- #1 · 0.1.0",
                "+++ #2 · 0.2.0",
                "@@ -2,7 +2,7 @@",
                " 2",
                " 3",
                " 4",
                "-5",
                "+five",
                " 6",
                " 7",
                " 8",
            ]
        );
    }

    #[test]
    fn far_apart_changes_are_two_hunks_and_an_added_end_is_counted() {
        let lines: Vec<String> = (1..=20).map(|n| n.to_string()).collect();
        let a = lines.join("\n") + "\n";
        let mut changed = lines.clone();
        changed[1] = "two".into();
        changed[18] = "nineteen".into();
        changed.push("21".into());
        let b = changed.join("\n") + "\n";
        let d = unified(&a, &b, "a", "b", 1);
        let hunks: Vec<&String> = d.iter().filter(|l| l.starts_with("@@")).collect();
        assert_eq!(hunks.len(), 2, "{d:#?}");
        assert_eq!(hunks[0], "@@ -1,3 +1,3 @@");
        assert_eq!(hunks[1], "@@ -18,3 +18,4 @@");
        assert!(d.contains(&"+21".to_string()));
    }

    #[test]
    fn an_empty_side_counts_from_zero() {
        assert_eq!(
            unified("", "x\n", "a", "b", 3),
            vec!["--- a", "+++ b", "@@ -0,0 +1 @@", "+x"]
        );
    }

    #[test]
    fn a_diff_round_trips_through_the_wire_shape() {
        let d = VersionDiff {
            a: Side {
                version: "0.1.0".into(),
                ordinal: Some(1),
                label: Some("first".into()),
            },
            b: Side {
                version: "0.2.0".into(),
                ordinal: Some(2),
                label: None,
            },
            schema: Some(Ok(vec![SchemaLine {
                code: "A2".into(),
                change: "new optional field `a.tag`".into(),
                class: "A".into(),
            }])),
            source: Some(vec!["--- a".into(), "+++ b".into()]),
        };
        assert_eq!(VersionDiff::from_json(&d.to_json()).unwrap(), d);
        let only_source = VersionDiff {
            schema: Some(Err("does not compile".into())),
            source: None,
            ..d
        };
        assert_eq!(
            VersionDiff::from_json(&only_source.to_json()).unwrap(),
            only_source
        );
    }
}
