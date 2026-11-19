//! Peers: other Ordo deployments in the same database (owner rulings
//! 2026-11-16 and 2026-11-17, `example-apps-notes-v2-plus-note_viewer` asks
//! 1, 2, 5 and 10).
//!
//! A program writes `peer journal`, and an external `from journal.notes`
//! reads the peer's `notes`. The file never names the peer's schema: the
//! setting `ORDO_PEER_JOURNAL_SCHEMA` does, read here through the same
//! reader and the same check as the deployment's own `ORDO_PG_SCHEMA`
//! ([`crate::schema_setting`], [`crate::check_schema_setting`]).
//!
//! A plain external may swap the schema its file writes with
//! `ORDO_EXTERNAL_<SCHEMA>_SCHEMA` (owner, 2026-11-17, ask 12):
//! `ORDO_EXTERNAL_CRM_SCHEMA=crm_dev` reads `crm_dev.customers` where the file
//! says `crm.customers`. Unset, the schema is as written. It is read here too,
//! by the same reader and check. A name that is both a `peer` and the schema
//! of a plain external cannot occur: with a `peer` line, every external
//! `from <name>.` is the peer's, and its `ORDO_EXTERNAL_..._SCHEMA` is not
//! read. A schema written in quotes that is not lowercase letters, digits and
//! underscores (`"Legacy"`) has no clean variable name and is not swapped.
//!
//! Two steps, both before a program serves:
//!
//! - [`resolve`] reads each peer's setting, and each plain external's
//!   optional one, and writes the schema into the program's externals. No
//!   I/O. A missing peer setting or a bad value is a [`SchemaRefusal`] naming
//!   the variable and what to set.
//! - [`check`] asks the database whether each peer schema holds an Ordo
//!   deployment: the tables `$caller` is resolved through must be there,
//!   and the peer's `$authconnection`, which it writes when it starts.
//!
//! Then [`link`], at start and every minute after: which of this
//! deployment's sign-in connections are the same provider as one of the
//! peer's (owner ruling 2026-11-18). Two sign-ins are the same person when
//! the provider fingerprint (`$authconnection`) and the subject match. The
//! connection name plays no part. The map is kept in `$peerlink`, which the
//! `$caller` guard joins ([`LINK_TABLE`]).

use std::collections::BTreeSet;

use ordo_schema::{GuardBody, Schema};
use ordo_syntax::decl::{GuardAtom, GuardExpr};

use crate::{check_schema_setting, schema_setting, sql_lit, Pool, PoolError, DEFAULT_SCHEMA};

/// The setting that names a peer's schema: `ORDO_PEER_<NAME>_SCHEMA`, the
/// nickname upper-cased (owner, 2026-11-17, ask 10). The parser admits only
/// lowercase letters and digits in a nickname, so no two share a variable.
pub fn schema_env(peer: &str) -> String {
    format!("ORDO_PEER_{}_SCHEMA", peer.to_ascii_uppercase())
}

/// The optional setting that swaps a plain external's schema:
/// `ORDO_EXTERNAL_<SCHEMA>_SCHEMA`, the schema as the file writes it,
/// upper-cased (owner, 2026-11-17, ask 12). `None` when the written name has
/// no clean variable name: anything but lowercase letters, digits and
/// underscores, which only a quoted name can hold.
pub fn external_schema_env(written: &str) -> Option<String> {
    let clean = written
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && written
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    clean.then(|| format!("ORDO_EXTERNAL_{}_SCHEMA", written.to_ascii_uppercase()))
}

/// The tables a peer schema must hold for Ordo to resolve `$caller` there:
/// the command log every deployment keeps, its members, and the directory
/// that links a sign-in subject to a person.
pub const PEER_TABLES: [&str; 4] = ["$commandlog", "members", "$authenticator", "$principal"];

/// Why a program's external schemas cannot be resolved: a peer's setting, or
/// a plain external's swap. Each names the setting.
#[derive(Debug)]
pub enum SchemaRefusal {
    /// The setting is not set.
    Unset { peer: String, var: String },
    /// The setting (a peer's, or a plain external's swap) is not a schema
    /// name Ordo accepts.
    Invalid { var: String, detail: String },
    /// The setting names this deployment's own schema.
    Own { var: String, schema: String },
    /// The schema holds no Ordo deployment: one of [`PEER_TABLES`] is
    /// missing.
    NotOrdo {
        var: String,
        schema: String,
        missing: Vec<String>,
    },
    /// The peer schema holds an Ordo deployment that has not started with
    /// this release: its `$authconnection` is not there.
    NotStarted {
        peer: String,
        var: String,
        schema: String,
    },
    /// `$caller` is used on the peer's tables, and the two deployments share
    /// no sign-in provider, so nobody would match.
    NoSharedProvider {
        peer: String,
        var: String,
        unmatched: Vec<Unmatched>,
    },
    /// One connection name points at one provider here and another in the
    /// peer.
    NameConflict { peer: String, name: String },
    /// The database could not be asked.
    Database(PoolError),
}

impl SchemaRefusal {
    /// The setting the refusal is about, when it is about one.
    pub fn var(&self) -> Option<&str> {
        match self {
            SchemaRefusal::Unset { var, .. }
            | SchemaRefusal::Invalid { var, .. }
            | SchemaRefusal::Own { var, .. }
            | SchemaRefusal::NotOrdo { var, .. }
            | SchemaRefusal::NotStarted { var, .. }
            | SchemaRefusal::NoSharedProvider { var, .. } => Some(var),
            SchemaRefusal::NameConflict { .. } | SchemaRefusal::Database(_) => None,
        }
    }
}

impl std::fmt::Display for SchemaRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaRefusal::Unset { peer, var } => write!(
                f,
                "{var} is not set. The program declares `peer {peer}`, and the peer's \
                 schema comes from this setting. Set {var} to the schema the other \
                 deployment's ORDO_PG_SCHEMA names (`{DEFAULT_SCHEMA}` when it sets none)."
            ),
            SchemaRefusal::Invalid { detail, .. } => f.write_str(detail),
            SchemaRefusal::Own { var, schema } => write!(
                f,
                "{var} is `{schema}`, this deployment's own schema. A peer is another \
                 deployment: set {var} to the schema that deployment's ORDO_PG_SCHEMA names."
            ),
            SchemaRefusal::NotOrdo {
                var,
                schema,
                missing,
            } => {
                let list = missing
                    .iter()
                    .map(|t| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let verb = if missing.len() == 1 { "is" } else { "are" };
                write!(
                    f,
                    "{var} is `{schema}`, which holds no Ordo deployment: {list} {verb} not \
                     there. Set {var} to the schema an Ordo deployment's ORDO_PG_SCHEMA \
                     names, and start that deployment once so its tables exist."
                )
            }
            SchemaRefusal::NotStarted { peer, var, schema } => write!(
                f,
                "{var} is `{schema}`, which has no `$authconnection` table yet. A \
                 deployment writes it when it starts with sign-in on, and this one \
                 matches people by it. Start {peer} first, then this deployment."
            ),
            SchemaRefusal::NoSharedProvider {
                peer,
                var,
                unmatched,
            } => {
                write!(
                    f,
                    "this program compares `$caller` with a field of {peer}'s tables, and \
                     this deployment shares no sign-in provider with {peer} ({var}), so \
                     nobody here would match anybody there. Give both deployments a \
                     connection to the same provider (the same ORDO_AUTH_<NAME>_ISSUER or \
                     ORDO_AUTH_<NAME>_PRESET; the names may differ), or remove `$caller` \
                     from the guards and filters on {peer}'s tables."
                )?;
                if !unmatched.is_empty() {
                    write!(f, " Not matching: {}.", unmatched_list(unmatched))?;
                }
                Ok(())
            }
            SchemaRefusal::NameConflict { peer, name } => write!(
                f,
                "sign-in connection `{name}` points at one provider here and another in \
                 {peer}. People are matched by provider and never by name, and a name \
                 that means two providers is a mistake or an attack, so this deployment \
                 does not start. Give one of the two connections a new name \
                 (ORDO_AUTH_METHODS and its ORDO_AUTH_<NAME>_ settings) in the deployment \
                 that added it last."
            ),
            SchemaRefusal::Database(e) => write!(f, "the peer check could not read: {e}"),
        }
    }
}

impl std::error::Error for SchemaRefusal {}

/// [`resolve_with`] over this process's environment.
pub fn resolve(schema: &mut Schema, own: &str) -> Result<(), SchemaRefusal> {
    resolve_with(schema, own, schema_setting)
}

/// Write each peer's schema into the externals that read it. `own` is this
/// deployment's schema, which a peer may not name. `lookup` reads a setting
/// by name; a test hands in its own.
pub fn resolve_with(
    schema: &mut Schema,
    own: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<(), SchemaRefusal> {
    for peer in schema.peers.clone() {
        let var = schema_env(&peer);
        let Some(value) = lookup(&var) else {
            return Err(SchemaRefusal::Unset { peer, var });
        };
        check_schema_setting(
            &var,
            &value,
            &format!("Set {var} to the schema the other deployment's ORDO_PG_SCHEMA names."),
        )
        .map_err(|detail| SchemaRefusal::Invalid {
            var: var.clone(),
            detail,
        })?;
        if value == own {
            return Err(SchemaRefusal::Own { var, schema: value });
        }
        for e in &mut schema.externals {
            if let Some(src) = e.external.as_mut() {
                if let Some(p) = src.peer.as_mut().filter(|p| p.name == peer) {
                    p.schema = Some(value.clone());
                }
            }
        }
    }
    // Plain externals: the optional swap. Each written schema is read once.
    for e in &mut schema.externals {
        let Some(src) = e.external.as_mut() else {
            continue;
        };
        if src.peer.is_some() {
            continue;
        }
        let Some(var) = external_schema_env(&src.schema) else {
            continue;
        };
        let Some(value) = lookup(&var) else {
            src.swapped = None;
            continue;
        };
        check_schema_setting(
            &var,
            &value,
            &format!(
                "Set {var} to the schema that holds `{}` in this environment, or unset it \
                 to read `{}` as the file writes it.",
                src.table, src.schema
            ),
        )
        .map_err(|detail| SchemaRefusal::Invalid {
            var: var.clone(),
            detail,
        })?;
        src.swapped = Some(value);
    }
    Ok(())
}

/// Whether each resolved peer schema holds an Ordo deployment.
pub async fn check(pool: &Pool, schema: &Schema) -> Result<(), SchemaRefusal> {
    for peer in &schema.peers {
        let Some(pg) = schema
            .externals
            .iter()
            .filter_map(|e| e.external.as_ref()?.peer.as_ref())
            .find(|p| &p.name == peer)
            .and_then(|p| p.schema.clone())
        else {
            continue;
        };
        let probes = PEER_TABLES
            .iter()
            .map(|t| {
                format!(
                    "to_regclass({}) IS NULL",
                    sql_lit(&format!("{}.{}", quote(&pg), quote(t)))
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let rows = pool
            .rows(&format!("SELECT {probes}"))
            .await
            .map_err(SchemaRefusal::Database)?;
        let missing: Vec<String> = rows
            .first()
            .map(|r| {
                r.iter()
                    .zip(PEER_TABLES)
                    .filter(|(gone, _)| gone.as_deref() == Some("t"))
                    .map(|(_, t)| t.to_string())
                    .collect()
            })
            .unwrap_or_default();
        if !missing.is_empty() {
            return Err(SchemaRefusal::NotOrdo {
                var: schema_env(peer),
                schema: pg,
                missing,
            });
        }
        if !has_table(pool, Some(&pg), CONNECTIONS).await? {
            return Err(SchemaRefusal::NotStarted {
                peer: peer.clone(),
                var: schema_env(peer),
                schema: pg,
            });
        }
    }
    Ok(())
}

// ── matching people across two deployments ──────────────────────────────

/// Each deployment's record of its sign-in connections: name, kind,
/// fingerprint, and since 2026-11-18 the subject claim (`ordo-host`'s
/// `federation::CONNECTION_DDL`).
pub const CONNECTIONS: &str = "$authconnection";

/// The map from this deployment's sign-in connections to the peer's, as the
/// `$caller` guard joins it. One row per (peer schema, local prefix, remote
/// prefix), each prefix `<kind>:<connection>:`. Rewritten by [`link`].
pub const LINK_TABLE: &str = "$peerlink";

pub const LINK_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$peerlink" (
  "peer"   text NOT NULL,
  "local"  text NOT NULL,
  "remote" text NOT NULL,
  PRIMARY KEY ("peer", "local", "remote")
)"#;

/// How often a running deployment rebuilds the map.
pub const REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// One sign-in connection as `$authconnection` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub name: String,
    pub kind: String,
    pub fingerprint: String,
    /// Empty when the row predates the column and the connection has not
    /// started since.
    pub subject_claim: String,
}

/// A connection of this deployment that matches nobody in the peer, and
/// why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmatched {
    pub connection: String,
    pub why: String,
}

fn unmatched_list(list: &[Unmatched]) -> String {
    list.iter()
        .map(|u| format!("`{}` ({})", u.connection, u.why))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Whether [`link`] runs at start, where a conflict refuses, or on the
/// minute, where it drops the connection and logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    Start,
    Refresh,
}

/// What [`link`] found for one peer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerLinks {
    /// The nickname of the `peer` line.
    pub peer: String,
    /// The peer's Postgres schema.
    pub schema: String,
    /// `(local prefix, remote prefix)`, each `<kind>:<connection>:`.
    pub pairs: Vec<(String, String)>,
    /// This deployment's connections that match nobody in the peer.
    pub unmatched: Vec<Unmatched>,
    /// Connections dropped on a refresh because their name means another
    /// provider in the peer.
    pub dropped: Vec<String>,
    /// People here (principal ids) who match more than one account in the
    /// peer. They read none of the peer's guarded rows.
    pub ambiguous: Vec<String>,
}

impl PeerLinks {
    /// The list a start prints: the connections that will not match.
    pub fn start_lines(&self) -> Vec<String> {
        let p = &self.peer;
        let mut out = vec![if self.unmatched.is_empty() {
            format!(
                "peer        {p}: every sign-in connection matches ({} link(s))",
                self.pairs.len()
            )
        } else {
            format!(
                "peer        {p}: these sign-in connections will not match in {p}: {}",
                unmatched_list(&self.unmatched)
            )
        }];
        out.extend(self.alarm_lines());
        out
    }

    /// What needs the deployment owner: a dropped connection, and the people
    /// who match more than one account.
    pub fn alarm_lines(&self) -> Vec<String> {
        let p = &self.peer;
        let mut out = Vec::new();
        for name in &self.dropped {
            out.push(format!(
                "peer        {p}: connection `{name}` now points at another provider in {p} \
                 and is dropped from the map. Give one of the two a new name."
            ));
        }
        if !self.ambiguous.is_empty() {
            out.push(format!(
                "peer        {p}: {} person(s) here match more than one account in {p} and \
                 read nothing there until one is left. Merge or retire the extra accounts \
                 in {p}: {}",
                self.ambiguous.len(),
                self.ambiguous.join(", ")
            ));
        }
        out
    }
}

/// The map for one peer, from both deployments' connections. No I/O.
///
/// A local connection links to every peer connection with the same kind and
/// fingerprint. It links to nothing when its subject claim can change hands
/// (`mutable`, the identity crate's `MUTABLE_SUBJECT_CLAIMS`), even when the
/// deployment owner acknowledged that claim, or when its subject claim is
/// not recorded. The third value is every name that means one provider here
/// and another in the peer; those link to nothing.
pub fn plan_links(
    peer: &str,
    local: &[Connection],
    remote: &[Connection],
    mutable: &[&str],
) -> (Vec<(String, String)>, Vec<Unmatched>, Vec<String>) {
    let mut pairs = Vec::new();
    let mut unmatched = Vec::new();
    let mut conflicts = Vec::new();
    let mut local: Vec<&Connection> = local.iter().collect();
    local.sort_by(|a, b| a.name.cmp(&b.name));
    for c in local {
        let why = |w: String| Unmatched {
            connection: c.name.clone(),
            why: w,
        };
        if remote
            .iter()
            .any(|r| r.name == c.name && (r.kind != c.kind || r.fingerprint != c.fingerprint))
        {
            conflicts.push(c.name.clone());
            continue;
        }
        if c.kind != "oidc" && c.kind != "oauth2" {
            unmatched.push(why(format!("a `{}` connection does not link", c.kind)));
            continue;
        }
        // An OIDC subject is always `sub`.
        let claim = if c.kind == "oidc" && c.subject_claim.is_empty() {
            "sub"
        } else {
            c.subject_claim.as_str()
        };
        if claim.is_empty() {
            unmatched.push(why(
                "its subject claim is not recorded yet; start this deployment once with \
                 the connection in ORDO_AUTH_METHODS"
                    .into(),
            ));
            continue;
        }
        if mutable.contains(&claim) {
            unmatched.push(why(format!(
                "its subject claim `{claim}` can change hands, so it never links; set \
                 ORDO_AUTH_{}_SUBJECT_CLAIM to the provider's fixed id",
                c.name.to_ascii_uppercase().replace('-', "_")
            )));
            continue;
        }
        let mut found = false;
        for r in remote
            .iter()
            .filter(|r| r.kind == c.kind && r.fingerprint == c.fingerprint)
        {
            found = true;
            pairs.push((
                format!("{}:{}:", c.kind, c.name),
                format!("{}:{}:", r.kind, r.name),
            ));
        }
        if !found {
            unmatched.push(why(format!(
                "no connection in {peer} signs in through this provider"
            )));
        }
    }
    pairs.sort();
    (pairs, unmatched, conflicts)
}

/// The peers whose tables this program compares with `$caller`, by
/// nickname: in a guard on a peer's table, or in an op that reads one.
///
/// The op test is by the op's text: an op that names a peer's table and
/// `$caller` counts. It may count an op that compares `$caller` with a
/// local field only. That errs toward refusing a start that shares no
/// provider.
pub fn callers(schema: &Schema, registry: &ordo_sema::Registry) -> BTreeSet<String> {
    fn walk(g: &GuardExpr) -> bool {
        g.disjuncts.iter().flatten().any(|a| match a {
            GuardAtom::Cmp { caller, .. } => *caller,
            GuardAtom::Bare { .. } => false,
            GuardAtom::Step { inner, .. } => walk(inner),
        })
    }
    let mut out = BTreeSet::new();
    let mut peer_entities: Vec<(String, String)> = Vec::new();
    for e in &schema.externals {
        let Some(p) = e.external.as_ref().and_then(|x| x.peer.as_ref()) else {
            continue;
        };
        peer_entities.push((e.name.clone(), p.name.clone()));
        if let Some(g) = &e.guards {
            let uses = [&g.read, &g.write]
                .iter()
                .any(|b| matches!(b, GuardBody::Walk(w) if walk(w)));
            if uses {
                out.insert(p.name.clone());
            }
        }
    }
    for name in registry.names() {
        let Some(op) = registry.get(name) else {
            continue;
        };
        let text = format!("{op:?}");
        if !text.contains("name: \"caller\"") {
            continue;
        }
        for (entity, peer) in &peer_entities {
            if text.contains(&format!("\"{entity}\"")) {
                out.insert(peer.clone());
            }
        }
    }
    out
}

/// Build the map for every peer of `schema` and write it to `$peerlink`.
///
/// At [`Pass::Start`] it refuses when the peer has no `$authconnection`,
/// when a connection name means two providers, and when a peer in
/// `callers` ([`callers`]) shares no provider. On [`Pass::Refresh`] a name
/// conflict drops that connection and is reported in
/// [`PeerLinks::dropped`]; a missing table links nothing. A connection
/// added to the peer is picked up on the next pass, and matches nothing
/// until then. There is no fallback to matching by name.
pub async fn link(
    pool: &Pool,
    schema: &Schema,
    callers: &BTreeSet<String>,
    mutable: &[&str],
    pass: Pass,
) -> Result<Vec<PeerLinks>, SchemaRefusal> {
    let mut out = Vec::new();
    if schema.peers.is_empty() {
        return Ok(out);
    }
    if pool.rows(LINK_DDL).await.is_err() {
        pool.rows(LINK_DDL).await.map_err(SchemaRefusal::Database)?;
    }
    let local = connections(pool, None).await?;
    let tenant_owned = tenant_connections(pool).await?;
    for peer in &schema.peers {
        let Some(pg) = schema
            .externals
            .iter()
            .filter_map(|e| e.external.as_ref()?.peer.as_ref())
            .find(|p| &p.name == peer)
            .and_then(|p| p.schema.clone())
        else {
            continue;
        };
        let remote = if has_table(pool, Some(&pg), CONNECTIONS).await? {
            connections(pool, Some(&pg)).await?
        } else if pass == Pass::Start {
            return Err(SchemaRefusal::NotStarted {
                peer: peer.clone(),
                var: schema_env(peer),
                schema: pg,
            });
        } else {
            Vec::new()
        };
        let (pairs, mut unmatched, conflicts) = plan_links(peer, &local, &remote, mutable);
        if pass == Pass::Start {
            if let Some(name) = conflicts.first() {
                return Err(SchemaRefusal::NameConflict {
                    peer: peer.clone(),
                    name: name.clone(),
                });
            }
        }
        if tenant_owned {
            unmatched.push(Unmatched {
                connection: "tenant-owned providers".into(),
                why: "a tenant's own sign-in providers never link to a peer".into(),
            });
        }
        if pass == Pass::Start && pairs.is_empty() && callers.contains(peer) {
            return Err(SchemaRefusal::NoSharedProvider {
                peer: peer.clone(),
                var: schema_env(peer),
                unmatched,
            });
        }
        write_links(pool, &pg, &pairs).await?;
        let ambiguous = ambiguous(pool, &pg).await?;
        out.push(PeerLinks {
            peer: peer.clone(),
            schema: pg,
            pairs,
            unmatched,
            dropped: if pass == Pass::Refresh {
                conflicts
            } else {
                Vec::new()
            },
            ambiguous,
        });
    }
    Ok(out)
}

/// Replace the peer's rows in `$peerlink`, in one implicit transaction, and
/// only when they changed.
async fn write_links(
    pool: &Pool,
    pg: &str,
    pairs: &[(String, String)],
) -> Result<(), SchemaRefusal> {
    let rows = pool
        .rows(&format!(
            r#"SELECT "local", "remote" FROM "$peerlink" WHERE "peer" = {} ORDER BY 1, 2"#,
            sql_lit(pg)
        ))
        .await
        .map_err(SchemaRefusal::Database)?;
    let now: Vec<(String, String)> = rows
        .into_iter()
        .map(|r| {
            (
                r.first().cloned().flatten().unwrap_or_default(),
                r.get(1).cloned().flatten().unwrap_or_default(),
            )
        })
        .collect();
    if now == pairs {
        return Ok(());
    }
    let mut sql = format!(r#"DELETE FROM "$peerlink" WHERE "peer" = {};"#, sql_lit(pg));
    if !pairs.is_empty() {
        let values = pairs
            .iter()
            .map(|(l, r)| format!("({}, {}, {})", sql_lit(pg), sql_lit(l), sql_lit(r)))
            .collect::<Vec<_>>()
            .join(", ");
        sql += &format!(
            r#" INSERT INTO "$peerlink" ("peer", "local", "remote") VALUES {values}
                ON CONFLICT DO NOTHING;"#
        );
    }
    pool.rows(&sql).await.map_err(SchemaRefusal::Database)?;
    Ok(())
}

/// People here whose links reach more than one live account in the peer.
async fn ambiguous(pool: &Pool, pg: &str) -> Result<Vec<String>, SchemaRefusal> {
    if !has_table(pool, None, "$authenticator").await? {
        return Ok(Vec::new());
    }
    let p = quote(pg);
    let rows = pool
        .rows(&format!(
            r#"SELECT la."principal" FROM "$authenticator" AS la
               JOIN "$peerlink" AS lk ON lk."peer" = {lit}
                 AND left(la."authenticator", length(lk."local")) = lk."local"
               JOIN {p}."$authenticator" AS pa
                 ON pa."authenticator" = lk."remote" || substr(la."authenticator", length(lk."local") + 1)
                 AND pa."retired_case" = ''
               JOIN {p}."$principal" AS pp ON pp."id" = pa."principal" AND pp."deleted_at" = 0
               WHERE la."retired_case" = ''
               GROUP BY la."principal"
               HAVING count(DISTINCT pa."principal") > 1
               ORDER BY 1"#,
            lit = sql_lit(pg),
        ))
        .await
        .map_err(SchemaRefusal::Database)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect())
}

/// The connections `$authconnection` records, here (`None`) or in a peer.
/// The subject claim is read through `to_jsonb`, so a row from before the
/// column reads as empty.
async fn connections(pool: &Pool, pg: Option<&str>) -> Result<Vec<Connection>, SchemaRefusal> {
    if pg.is_none() && !has_table(pool, None, CONNECTIONS).await? {
        return Ok(Vec::new());
    }
    let table = match pg {
        Some(s) => format!("{}.{}", quote(s), quote(CONNECTIONS)),
        None => quote(CONNECTIONS),
    };
    let rows = pool
        .rows(&format!(
            r#"SELECT c."name", c."kind", c."fingerprint",
                      coalesce(to_jsonb(c) ->> 'subject_claim', '')
               FROM {table} AS c ORDER BY 1"#
        ))
        .await
        .map_err(SchemaRefusal::Database)?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let at = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
            Connection {
                name: at(0),
                kind: at(1),
                fingerprint: at(2),
                subject_claim: at(3),
            }
        })
        .collect())
}

/// Whether this deployment holds any tenant-owned sign-in provider.
async fn tenant_connections(pool: &Pool) -> Result<bool, SchemaRefusal> {
    if !has_table(pool, None, "$tenantconnection").await? {
        return Ok(false);
    }
    let rows = pool
        .rows(r#"SELECT EXISTS (SELECT 1 FROM "$tenantconnection")"#)
        .await
        .map_err(SchemaRefusal::Database)?;
    Ok(first_is_true(&rows))
}

async fn has_table(pool: &Pool, pg: Option<&str>, table: &str) -> Result<bool, SchemaRefusal> {
    let name = match pg {
        Some(s) => format!("{}.{}", quote(s), quote(table)),
        None => quote(table),
    };
    let rows = pool
        .rows(&format!(
            "SELECT to_regclass({}) IS NOT NULL",
            sql_lit(&name)
        ))
        .await
        .map_err(SchemaRefusal::Database)?;
    Ok(first_is_true(&rows))
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn first_is_true(rows: &[crate::TextRow]) -> bool {
    rows.first()
        .and_then(|r| r.first().cloned().flatten())
        .as_deref()
        == Some("t")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, kind: &str, fp: &str, claim: &str) -> Connection {
        Connection {
            name: name.into(),
            kind: kind.into(),
            fingerprint: fp.into(),
            subject_claim: claim.into(),
        }
    }

    const MUTABLE: [&str; 2] = ["login", "email"];

    #[test]
    fn the_provider_links_and_the_name_does_not() {
        // Same issuer under two names: linked.
        let (pairs, un, cf) = plan_links(
            "journal",
            &[c("corp", "oidc", "F1", "sub")],
            &[c("identity", "oidc", "F1", "sub")],
            &MUTABLE,
        );
        assert_eq!(
            pairs,
            [("oidc:corp:".to_string(), "oidc:identity:".to_string())]
        );
        assert!(un.is_empty() && cf.is_empty());
        // Same name, another issuer: a conflict, never a link.
        let (pairs, _, cf) = plan_links(
            "journal",
            &[c("corp", "oidc", "F1", "sub")],
            &[c("corp", "oidc", "F2", "sub")],
            &MUTABLE,
        );
        assert!(pairs.is_empty());
        assert_eq!(cf, ["corp"]);
        // No provider in common: listed with the reason.
        let (pairs, un, _) = plan_links(
            "journal",
            &[c("corp", "oidc", "F1", "")],
            &[c("other", "oidc", "F2", "sub")],
            &MUTABLE,
        );
        assert!(pairs.is_empty());
        assert!(un[0].why.contains("no connection in journal"), "{un:?}");
    }

    #[test]
    fn a_changeable_subject_claim_never_links() {
        let (pairs, un, _) = plan_links(
            "journal",
            &[
                c("gh", "oauth2", "F1", "login"),
                c("dc", "oauth2", "F3", ""),
            ],
            &[
                c("gh", "oauth2", "F1", "login"),
                c("dc", "oauth2", "F3", "id"),
            ],
            &MUTABLE,
        );
        assert!(pairs.is_empty());
        assert!(un.iter().any(|u| u.connection == "gh"
            && u.why.contains("`login` can change hands")
            && u.why.contains("ORDO_AUTH_GH_SUBJECT_CLAIM")));
        assert!(un
            .iter()
            .any(|u| u.connection == "dc" && u.why.contains("not recorded")));
    }
}
