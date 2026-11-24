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
//!
//! **Consent is Postgres's** (owner ruling 2026-11-18, ask 17). The app being
//! read writes `reader note_viewer reads notes, notesAccess`, and the setting
//! `ORDO_READER_NOTE_VIEWER_ROLE` names the reader's database role.
//! [`grant_readers`] makes the grants that role holds in this deployment's
//! schema equal what the file says: `USAGE` on the schema, `SELECT` on the
//! named tables, and `SELECT` on the sign-in columns a `$caller` guard reads
//! ([`crate::sign_in_contract`], a contract between Ordo releases). It runs
//! at every apply and every start, so a table taken off the line is revoked, and a table a migration creates is granted
//! in the same apply. On the reader's side, [`check_access`] refuses a start
//! whose role lacks a grant, quoting the line to add, and refuses a role that
//! reads the peer without a grant at all (its owner, a superuser), unless
//! [`SHARED_ROLE_ENV`] is on.

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

/// The setting that names a reader's database role:
/// `ORDO_READER_<NAME>_ROLE`, the nickname upper-cased (owner, 2026-11-18).
/// The parser admits lowercase letters, digits and underscores in a reader's
/// nickname, so no two share a variable.
pub fn reader_role_env(reader: &str) -> String {
    format!("ORDO_READER_{}_ROLE", reader.to_ascii_uppercase())
}

/// The setting that lets two deployments read each other as one database
/// role, for local work: `ORDO_PEER_SHARED_ROLE=on`. Off by default. With
/// one role, grants decide nothing, so a reader refuses to start that way
/// unless this is on.
pub const SHARED_ROLE_ENV: &str = "ORDO_PEER_SHARED_ROLE";

/// The table that remembers which roles this deployment granted to, so a
/// reader line removed from the file is revoked on the next apply or start.
pub const READER_TABLE: &str = "$readergrant";

const READER_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$readergrant" (
  "role"   text PRIMARY KEY,
  "reader" text NOT NULL
)"#;

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
    /// A `reader` line's role setting is not set.
    ReaderUnset { reader: String, var: String },
    /// A `reader` line's role is not a role of this database server.
    ReaderRoleMissing {
        reader: String,
        var: String,
        role: String,
    },
    /// A `reader` line's role is this deployment's own role (or one that
    /// owns it), so a grant decides nothing.
    ReaderOwnRole {
        reader: String,
        var: String,
        role: String,
    },
    /// The grants were sent and the catalog does not show them: this
    /// deployment's role does not own its schema or its tables.
    CannotGrant {
        role: String,
        own: String,
        schema: String,
    },
    /// [`SHARED_ROLE_ENV`] holds a value that is not a switch.
    SharedRoleValue { value: String },
    /// This deployment's role lacks a grant on a peer table it reads.
    NotGranted {
        peer: String,
        role: String,
        schema: String,
        missing: Vec<String>,
        line: String,
    },
    /// This deployment's role reads the peer without any grant: it owns the
    /// peer's schema or tables, is a member of that owner, or is a
    /// superuser.
    SharedRole {
        peer: String,
        role: String,
        why: String,
    },
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
            | SchemaRefusal::NoSharedProvider { var, .. }
            | SchemaRefusal::ReaderUnset { var, .. }
            | SchemaRefusal::ReaderRoleMissing { var, .. }
            | SchemaRefusal::ReaderOwnRole { var, .. } => Some(var),
            SchemaRefusal::SharedRoleValue { .. } | SchemaRefusal::SharedRole { .. } => {
                Some(SHARED_ROLE_ENV)
            }
            SchemaRefusal::NameConflict { .. }
            | SchemaRefusal::CannotGrant { .. }
            | SchemaRefusal::NotGranted { .. }
            | SchemaRefusal::Database(_) => None,
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
            SchemaRefusal::ReaderUnset { reader, var } => write!(
                f,
                "{var} is not set. The program declares `reader {reader}`, and the role \
                 that deployment reads with comes from this setting. Set {var} to the \
                 database role in the other deployment's ORDO_PG_USER, or remove the \
                 `reader {reader}` line."
            ),
            SchemaRefusal::ReaderRoleMissing { reader, var, role } => write!(
                f,
                "{var} is `{role}`, which is not a role of this database server. Create the \
                 role `{role}` and give it to the deployment `reader {reader}` names, or set \
                 {var} to the role in that deployment's ORDO_PG_USER."
            ),
            SchemaRefusal::ReaderOwnRole { reader, var, role } => write!(
                f,
                "{var} is `{role}`, the role this deployment connects as, so `reader {reader}` \
                 can read every table here and a grant decides nothing. Give each deployment \
                 its own role and set {var} to the reader's. For local work only, set \
                 {SHARED_ROLE_ENV}=on."
            ),
            SchemaRefusal::CannotGrant { role, own, schema } => write!(
                f,
                "the grants for role `{role}` in schema `{schema}` did not take. This \
                 deployment's role `{own}` must own schema `{schema}` and its tables to \
                 grant on them. Make `{own}` their owner (ALTER SCHEMA and ALTER TABLE \
                 ... OWNER TO), then apply or restart."
            ),
            SchemaRefusal::SharedRoleValue { value } => write!(
                f,
                "{SHARED_ROLE_ENV} is `{value}`. Set it to `on` or `off`, or unset it."
            ),
            SchemaRefusal::NotGranted {
                peer,
                role,
                schema,
                missing,
                line,
            } => {
                let list = missing
                    .iter()
                    .map(|t| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "role `{role}` cannot read {list} in {peer} (schema `{schema}`). The \
                     deployment being read decides what its readers see. Add this line to \
                     {peer}'s definition file, with a nickname of your choice for <name>:\n  \
                     {line}\nthen set ORDO_READER_<NAME>_ROLE={role} in {peer}'s settings, \
                     and apply or restart {peer}."
                )
            }
            SchemaRefusal::SharedRole { peer, role, why } => write!(
                f,
                "this deployment reads {peer} as role `{role}`, which {why}. It can read \
                 every table of {peer} without a grant, so {peer}'s `reader` line decides \
                 nothing. Give each deployment its own database role (ORDO_PG_USER). For \
                 local work only, set {SHARED_ROLE_ENV}=on."
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
    // The subject claim by its column, never through the whole row: a
    // reader is granted the contract's columns only (`sign_in_contract`).
    let has_claim = first_is_true(
        &pool
            .rows(&format!(
                "SELECT EXISTS (SELECT 1 FROM pg_attribute a \
                 JOIN pg_class c ON c.oid = a.attrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = {} AND c.relname = {} \
                 AND a.attname = 'subject_claim' AND NOT a.attisdropped)",
                match pg {
                    Some(s) => sql_lit(s),
                    None => "current_schema()".to_string(),
                },
                sql_lit(CONNECTIONS)
            ))
            .await
            .map_err(SchemaRefusal::Database)?,
    );
    let claim = if has_claim {
        r#"c."subject_claim""#
    } else {
        "''"
    };
    let rows = pool
        .rows(&format!(
            r#"SELECT c."name", c."kind", c."fingerprint", coalesce({claim}, '')
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

// ── consent: the reader line and its grants ─────────────────────────────

/// [`SHARED_ROLE_ENV`] read: `on`, `1`, `true`, `yes` allow; unset, blank,
/// `off`, `0`, `false`, `no` refuse. Anything else is a refusal.
pub fn shared_role_value(v: Option<&str>) -> Result<bool, SchemaRefusal> {
    match v.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("off") | Some("0") | Some("false") | Some("no") => Ok(false),
        Some("on") | Some("1") | Some("true") | Some("yes") => Ok(true),
        Some(_) => Err(SchemaRefusal::SharedRoleValue {
            value: v.unwrap_or_default().to_string(),
        }),
    }
}

/// [`shared_role_value`] over this process's environment.
pub fn shared_role_allowed() -> Result<bool, SchemaRefusal> {
    shared_role_value(std::env::var(SHARED_ROLE_ENV).ok().as_deref())
}

/// One privilege a role holds in a schema: a table (`column` empty for the
/// whole table) and the privilege's name.
type Held = BTreeSet<(String, String, String)>;

/// Each `reader` line's nickname and the role its `ORDO_READER_<NAME>_ROLE`
/// names, or the refusal naming the variable that is unset.
fn reader_roles(
    schema: &Schema,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Vec<(String, String)>, SchemaRefusal> {
    let mut roles = Vec::new();
    for r in &schema.readers {
        let var = reader_role_env(&r.name);
        let Some(role) = lookup(&var) else {
            return Err(SchemaRefusal::ReaderUnset {
                reader: r.name.clone(),
                var,
            });
        };
        roles.push((r.name.clone(), role));
    }
    Ok(roles)
}

async fn current_role(pool: &Pool) -> Result<String, SchemaRefusal> {
    Ok(pool
        .rows("SELECT current_user::text")
        .await
        .map_err(SchemaRefusal::Database)?
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default())
}

/// One reader's role: it must exist, and must not be this deployment's own
/// unless [`SHARED_ROLE_ENV`] allows it. `true` when it is the own role and
/// that is allowed, so no grants are made.
async fn check_reader_role(
    pool: &Pool,
    own: &str,
    reader: &str,
    role: &str,
    shared_ok: bool,
) -> Result<bool, SchemaRefusal> {
    let var = reader_role_env(reader);
    if !role_exists(pool, role).await? {
        return Err(SchemaRefusal::ReaderRoleMissing {
            reader: reader.to_string(),
            var,
            role: role.to_string(),
        });
    }
    if role == own || is_member(pool, own, role).await? {
        if !shared_ok {
            return Err(SchemaRefusal::ReaderOwnRole {
                reader: reader.to_string(),
                var,
                role: role.to_string(),
            });
        }
        return Ok(true);
    }
    Ok(false)
}

/// **The reader check, granting nothing**: what [`grant_readers_with`]
/// refuses before it changes a grant. Each `reader` line has its role
/// setting, the role exists, and it is not this deployment's own (unless
/// [`SHARED_ROLE_ENV`] allows that). The promotion gate runs it on the
/// target before it stores a version (`plan-versions.md` §3.1).
pub async fn check_readers_with(
    pool: &Pool,
    schema: &Schema,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<(), SchemaRefusal> {
    if schema.readers.is_empty() {
        return Ok(());
    }
    let shared_ok = shared_role_value(lookup(SHARED_ROLE_ENV).as_deref())?;
    let roles = reader_roles(schema, &lookup)?;
    let own = current_role(pool).await?;
    for (reader, role) in &roles {
        check_reader_role(pool, &own, reader, role, shared_ok).await?;
    }
    Ok(())
}

/// [`check_readers_with`] over this process's environment.
pub async fn check_readers(pool: &Pool, schema: &Schema) -> Result<(), SchemaRefusal> {
    check_readers_with(pool, schema, schema_setting).await
}

/// [`grant_readers_with`] over this process's environment.
pub async fn grant_readers(pool: &Pool, schema: &Schema) -> Result<Vec<String>, SchemaRefusal> {
    grant_readers_with(pool, schema, schema_setting).await
}

/// Make the grants each reader's role holds in this deployment's schema
/// equal what the `reader` lines say, and revoke everything from a role
/// whose line is gone. Returns one line per reader for the start log.
///
/// Each role gets `USAGE` on the schema, `SELECT` on its tables, and
/// `SELECT` on the sign-in contract's columns ([`crate::sign_in_contract`])
/// of each sign-in table it does not name whole, and on the release this
/// deployment runs. A table not created yet is granted by the apply that creates
/// it. Nothing is sent when the catalog already agrees, so a start of an
/// unchanged deployment writes nothing.
pub async fn grant_readers_with(
    pool: &Pool,
    schema: &Schema,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Vec<String>, SchemaRefusal> {
    let shared_ok = shared_role_value(lookup(SHARED_ROLE_ENV).as_deref())?;
    let roles = reader_roles(schema, &lookup)?;
    let recorded = has_table(pool, None, READER_TABLE).await?;
    if roles.is_empty() && !recorded {
        return Ok(Vec::new());
    }
    let pg = pool.schema().to_string();
    let own = current_role(pool).await?;

    // What each role should hold.
    let present = tables_in(pool, &pg).await?;
    // **A reader reads only columns the read app still writes** (owner
    // ruling 2026-11-23, Q1; `plan-evolution-federation.md` §2). A column
    // a version left behind (`$orphans`) and `schema` does not declare
    // again is never written, so a table holding one is granted by column,
    // without it. The reader's check then names the column it lost.
    let mut orphaned: std::collections::BTreeMap<String, BTreeSet<String>> = Default::default();
    for o in crate::orphans::live(pool)
        .await
        .map_err(SchemaRefusal::Database)?
    {
        let declared = schema
            .native(&o.entity)
            .is_some_and(|e| e.field(&o.field).is_some());
        if !o.is_entity() && !declared {
            orphaned
                .entry(o.entity.clone())
                .or_default()
                .extend(o.columns.iter().cloned());
        }
    }
    let mut want: std::collections::BTreeMap<String, (String, Held)> = Default::default();
    let mut out = Vec::new();
    for (r, (reader, role)) in schema.readers.iter().zip(&roles) {
        if check_reader_role(pool, &own, reader, role, shared_ok).await? {
            out.push(format!(
                "reader      {reader}: role `{role}` is this deployment's own \
                 ({SHARED_ROLE_ENV}=on); no grants are made"
            ));
            continue;
        }
        let entry = want
            .entry(role.clone())
            .or_insert_with(|| (reader.clone(), Held::new()));
        for t in r.tables.iter().filter(|t| present.contains_key(*t)) {
            match (orphaned.get(t), present.get(t)) {
                (Some(gone), Some(cols)) => {
                    for c in cols.iter().filter(|c| !gone.contains(*c)) {
                        entry.1.insert((t.clone(), c.clone(), "SELECT".into()));
                    }
                }
                _ => {
                    entry.1.insert((t.clone(), String::new(), "SELECT".into()));
                }
            }
        }
        out.push(format!(
            "reader      {reader}: role `{role}` reads {}",
            r.tables.join(", ")
        ));
    }
    // The sign-in contract (owner ruling 2026-11-23, Q16): the columns a
    // `$caller` guard reads, retired ones kept for the load window, and the
    // release this deployment runs, for the reader's release check.
    let mut contract = crate::sign_in_contract::columns();
    contract.push((
        crate::sign_in_contract::RELEASE_TABLE,
        vec![crate::sign_in_contract::RELEASE_COLUMN],
    ));
    for (_, held) in want.values_mut() {
        for (t, cols) in &contract {
            let whole = held.contains(&(t.to_string(), String::new(), "SELECT".into()));
            let Some(have) = present.get(*t) else {
                continue;
            };
            if whole {
                continue;
            }
            for c in cols.iter().filter(|c| have.contains(**c)) {
                held.insert((t.to_string(), c.to_string(), "SELECT".into()));
            }
        }
    }

    // Roles granted before whose line is gone.
    let mut gone: Vec<String> = Vec::new();
    if recorded {
        let rows = pool
            .rows(r#"SELECT "role" FROM "$readergrant" ORDER BY 1"#)
            .await
            .map_err(SchemaRefusal::Database)?;
        for r in rows {
            let Some(role) = r.into_iter().next().flatten() else {
                continue;
            };
            if !want.contains_key(&role) && role != own && role_exists(pool, &role).await? {
                gone.push(role);
            }
        }
    }

    let s = quote(&pg);
    let mut sql = String::new();
    for role in &gone {
        let r = quote(role);
        sql += &format!("REVOKE ALL ON ALL TABLES IN SCHEMA {s} FROM {r}; ");
        sql += &format!("REVOKE USAGE ON SCHEMA {s} FROM {r}; ");
    }
    let mut changed: Vec<&String> = Vec::new();
    for (role, (_, held)) in &want {
        let now = held_by(pool, &pg, role).await?;
        if now == *held && has_usage(pool, &pg, role).await? {
            continue;
        }
        changed.push(role);
        sql += &grant_sql(&pg, role, held);
    }
    let record_changed = if recorded {
        let rows = pool
            .rows(r#"SELECT "role", "reader" FROM "$readergrant" ORDER BY 1"#)
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
        let then: Vec<(String, String)> = want
            .iter()
            .map(|(role, (reader, _))| (role.clone(), reader.clone()))
            .collect();
        now != then
    } else {
        true
    };
    if sql.is_empty() && !record_changed {
        return Ok(out);
    }
    if !recorded {
        pool.rows(READER_DDL)
            .await
            .map_err(SchemaRefusal::Database)?;
    }
    // The table by its literal name: a platform table, never an entity's.
    sql += r#"DELETE FROM "$readergrant";"#;
    if !want.is_empty() {
        let values = want
            .iter()
            .map(|(role, (reader, _))| format!("({}, {})", sql_lit(role), sql_lit(reader)))
            .collect::<Vec<_>>()
            .join(", ");
        sql += &format!(r#" INSERT INTO "$readergrant" ("role", "reader") VALUES {values};"#);
    }
    // One implicit transaction. Two replicas starting at once may race on
    // the catalog row; the loser checks the result below, which is what
    // counts.
    let sent = pool.rows(&sql).await;
    for role in &changed {
        let (_, held) = &want[*role];
        let now = held_by(pool, &pg, role).await?;
        if now != *held || !has_usage(pool, &pg, role).await? {
            if let Err(e) = sent {
                return Err(SchemaRefusal::Database(e));
            }
            return Err(SchemaRefusal::CannotGrant {
                role: role.to_string(),
                own: own.clone(),
                schema: pg.clone(),
            });
        }
    }
    if let Err(e) = sent {
        if changed.is_empty() {
            return Err(SchemaRefusal::Database(e));
        }
    }
    Ok(out)
}

/// The statements that set one role's grants in schema `pg` to `held`:
/// everything revoked, then usage and each grant, in one batch.
fn grant_sql(pg: &str, role: &str, held: &Held) -> String {
    let (s, r) = (quote(pg), quote(role));
    let mut sql = format!(
        "REVOKE ALL ON ALL TABLES IN SCHEMA {s} FROM {r}; GRANT USAGE ON SCHEMA {s} TO {r}; "
    );
    let mut columns: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for (t, c, _) in held {
        if c.is_empty() {
            sql += &format!("GRANT SELECT ON {s}.{} TO {r}; ", quote(t));
        } else {
            columns.entry(t).or_default().push(c);
        }
    }
    for (t, cols) in columns {
        let list = cols.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", ");
        sql += &format!("GRANT SELECT ({list}) ON {s}.{} TO {r}; ", quote(t));
    }
    sql
}

/// Every table in `pg`, with its column names.
async fn tables_in(
    pool: &Pool,
    pg: &str,
) -> Result<std::collections::BTreeMap<String, BTreeSet<String>>, SchemaRefusal> {
    let rows = pool
        .rows(&format!(
            "SELECT c.relname::text, a.attname::text FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
             WHERE n.nspname = {} AND c.relkind IN ('r', 'p')",
            sql_lit(pg)
        ))
        .await
        .map_err(SchemaRefusal::Database)?;
    let mut out: std::collections::BTreeMap<String, BTreeSet<String>> = Default::default();
    for r in rows {
        let at = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        out.entry(at(0)).or_default().insert(at(1));
    }
    Ok(out)
}

/// What `role` holds in `pg` by a grant made to it by name: table
/// privileges with an empty column, column privileges with theirs.
async fn held_by(pool: &Pool, pg: &str, role: &str) -> Result<Held, SchemaRefusal> {
    let rows = pool
        .rows(&format!(
            "WITH r AS (SELECT oid FROM pg_roles WHERE rolname = {role}) \
             SELECT c.relname::text, '', g.privilege_type::text FROM pg_class c \
               JOIN pg_namespace n ON n.oid = c.relnamespace \
               CROSS JOIN LATERAL aclexplode(c.relacl) g \
               WHERE n.nspname = {pg} AND g.grantee = (SELECT oid FROM r) \
             UNION \
             SELECT c.relname::text, a.attname::text, g.privilege_type::text FROM pg_attribute a \
               JOIN pg_class c ON c.oid = a.attrelid \
               JOIN pg_namespace n ON n.oid = c.relnamespace \
               CROSS JOIN LATERAL aclexplode(a.attacl) g \
               WHERE n.nspname = {pg} AND a.attacl IS NOT NULL \
               AND g.grantee = (SELECT oid FROM r)",
            role = sql_lit(role),
            pg = sql_lit(pg),
        ))
        .await
        .map_err(SchemaRefusal::Database)?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let at = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
            (at(0), at(1), at(2))
        })
        .collect())
}

async fn has_usage(pool: &Pool, pg: &str, role: &str) -> Result<bool, SchemaRefusal> {
    let rows = pool
        .rows(&format!(
            "SELECT has_schema_privilege({}, n.oid, 'USAGE') FROM pg_namespace n \
             WHERE n.nspname = {}",
            sql_lit(role),
            sql_lit(pg)
        ))
        .await
        .map_err(SchemaRefusal::Database)?;
    Ok(first_is_true(&rows))
}

async fn role_exists(pool: &Pool, role: &str) -> Result<bool, SchemaRefusal> {
    let rows = pool
        .rows(&format!(
            "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = {})",
            sql_lit(role)
        ))
        .await
        .map_err(SchemaRefusal::Database)?;
    Ok(first_is_true(&rows))
}

/// Whether `role` holds the rights of `of` (is a member of it).
async fn is_member(pool: &Pool, of: &str, role: &str) -> Result<bool, SchemaRefusal> {
    let rows = pool
        .rows(&format!(
            "SELECT pg_has_role({}, {}, 'MEMBER')",
            sql_lit(role),
            sql_lit(of)
        ))
        .await
        .map_err(SchemaRefusal::Database)?;
    Ok(first_is_true(&rows))
}

/// [`check_access`] with [`SHARED_ROLE_ENV`] read from this process's
/// environment.
pub async fn check_access_env(
    pool: &Pool,
    schema: &Schema,
    callers: &BTreeSet<String>,
) -> Result<Vec<String>, SchemaRefusal> {
    check_access(pool, schema, callers, shared_role_allowed()?).await
}

/// The reader's side, before the peer is read at all: this deployment's
/// role must hold `USAGE` on each peer schema and `SELECT` on every peer
/// table an external reads, and on the sign-in contract's columns when `$caller`
/// is used on that peer (`callers`, [`callers`]). A missing grant refuses
/// the start and quotes the `reader` line to add to the peer's file.
///
/// A role that reads the peer with no grant at all (a superuser, the
/// owner of the peer's schema or of one of its tables, a member of that
/// owner, or a member of `pg_read_all_data`) is refused unless
/// `shared_ok`, because the peer's `reader` line would then decide
/// nothing. A table that is not there is left to [`check`].
pub async fn check_access(
    pool: &Pool,
    schema: &Schema,
    callers: &BTreeSet<String>,
    shared_ok: bool,
) -> Result<Vec<String>, SchemaRefusal> {
    let mut out = Vec::new();
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
        let rows = pool
            .rows(&format!(
                "SELECT current_user::text, r.rolsuper, \
                   pg_has_role(current_user, n.nspowner, 'MEMBER'), \
                   EXISTS (SELECT 1 FROM pg_class c WHERE c.relnamespace = n.oid \
                     AND c.relkind IN ('r', 'p', 'v', 'm', 'f') \
                     AND pg_has_role(current_user, c.relowner, 'MEMBER')), \
                   EXISTS (SELECT 1 FROM pg_roles a WHERE a.rolname = 'pg_read_all_data' \
                     AND pg_has_role(current_user, a.oid, 'MEMBER')), \
                   has_schema_privilege(current_user, n.oid, 'USAGE') \
                 FROM pg_namespace n, pg_roles r \
                 WHERE n.nspname = {} AND r.rolname = current_user",
                sql_lit(&pg)
            ))
            .await
            .map_err(SchemaRefusal::Database)?;
        let Some(row) = rows.first() else {
            // No such schema: `check` says so.
            continue;
        };
        let at = |i: usize| row.get(i).cloned().flatten().unwrap_or_default();
        let role = at(0);
        let why = if at(1) == "t" {
            Some("is a superuser".to_string())
        } else if at(2) == "t" {
            Some(format!("owns schema `{pg}` or is a member of its owner"))
        } else if at(3) == "t" {
            Some(format!(
                "owns tables in schema `{pg}` or is a member of their owner"
            ))
        } else if at(4) == "t" {
            Some("is a member of `pg_read_all_data`".to_string())
        } else {
            None
        };
        if let Some(why) = why {
            if !shared_ok {
                return Err(SchemaRefusal::SharedRole {
                    peer: peer.clone(),
                    role,
                    why,
                });
            }
            out.push(format!(
                "peer        {peer}: role `{role}` {why} ({SHARED_ROLE_ENV}=on); the \
                 peer's `reader` line decides nothing"
            ));
            continue;
        }
        let usage = at(5) == "t";
        let mut tables: Vec<String> = Vec::new();
        for e in &schema.externals {
            let Some(src) = e.external.as_ref() else {
                continue;
            };
            if src.peer.as_ref().is_some_and(|p| &p.name == peer) && !tables.contains(&src.table) {
                tables.push(src.table.clone());
            }
        }
        let mut missing: Vec<String> = Vec::new();
        let present = tables_in(pool, &pg).await?;
        for t in &tables {
            if present.contains_key(t) && !(usage && can_select(pool, &pg, t, &[]).await?) {
                missing.push(t.clone());
            }
        }
        if callers.contains(peer) {
            for (t, cols) in crate::sign_in_contract::columns() {
                let Some(have) = present.get(t) else {
                    continue;
                };
                let cols: Vec<&str> = cols.iter().copied().filter(|c| have.contains(*c)).collect();
                if !(usage && can_select(pool, &pg, t, &cols).await?) {
                    missing.push(t.to_string());
                }
            }
        }
        if !missing.is_empty() {
            return Err(SchemaRefusal::NotGranted {
                peer: peer.clone(),
                role,
                schema: pg,
                missing,
                line: format!("reader <name> reads {}", tables.join(", ")),
            });
        }
    }
    Ok(out)
}

/// Whether this deployment's role may `SELECT` the table, or, given
/// columns, each of those columns. Asked of the catalog, by oid, so a
/// schema it has no `USAGE` on is not an error.
async fn can_select(
    pool: &Pool,
    pg: &str,
    table: &str,
    columns: &[&str],
) -> Result<bool, SchemaRefusal> {
    let test = if columns.is_empty() {
        // Granted whole, or by column: a read app grants a table by column
        // once a column of it is orphaned (`crate::orphans`). Which mapped
        // columns are readable is the external check's to say, by name.
        "(has_table_privilege(c.oid, 'SELECT') OR has_any_column_privilege(c.oid, 'SELECT'))"
            .to_string()
    } else {
        columns
            .iter()
            .map(|col| format!("has_column_privilege(c.oid, {}, 'SELECT')", sql_lit(col)))
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let rows = pool
        .rows(&format!(
            "SELECT {test} FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = {} AND c.relname = {}",
            sql_lit(pg),
            sql_lit(table)
        ))
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
