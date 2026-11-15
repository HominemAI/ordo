//! `ordo introspect --external <schema>.<table>`: the generator for external
//! entities (ruled 2026-11-15, `external-entities`, slice B).
//!
//! It reads one live table and prints the `external` block that translates
//! it, for a person to review as text. It never writes to the database and
//! never writes the program: the developer saves the block, reads it, and
//! applies it like any other change.
//!
//! The reader is `ordo_exec::migrate::live_shape` and `unique_keys`, the
//! Postgres reader `ordo introspect` will share (`migration.md` section 2).
//! Types come from the table in `migration.md` section 2a, with the
//! decisions below:
//!
//! | Postgres | Ordo |
//! |---|---|
//! | `text`, `varchar(n)`, `char(n)` | `text` |
//! | `smallint`, `integer`, `bigint` | `int` |
//! | `boolean` | `bool` |
//! | `date` | `date` |
//! | `timestamptz`, `timestamp` | `datetime` (bare `timestamp` is noted) |
//! | `numeric(p,s)` | `decimal(s)` |
//!
//! `numeric` becomes `decimal(s)` and not `money`: an Ordo `money` is whole
//! minor units held in a `bigint`, and reading a `numeric` column as one
//! would be wrong. Anything else (`jsonb`, arrays, `bytea`, `uuid` outside
//! the key, floats, enums, `numeric` with no scale) is written as a comment
//! naming the column and its Postgres type, so the file still parses and
//! the worklist is in front of the reviewer.
//!
//! The key is the table's primary key when it is one column. `--key` names
//! another unique, not null column. A composite primary key with no `--key`,
//! or a table with no key, is refused `external_no_key`, naming the fix.

use ordo_exec::migrate::{self, UniqueKey};
use ordo_schema::diff::LiveTable;

use crate::operator::{connect, Flags, Outcome};

/// Why the generator wrote nothing. Each says what the caller can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntrospectError {
    /// The table is not in the database (or this role cannot see it).
    /// Fix: check the name, or grant the role `USAGE` and `SELECT`.
    Missing { schema: String, table: String },
    /// No usable key. Fix: name a unique, not null column with `--key`.
    NoKey {
        schema: String,
        table: String,
        why: String,
    },
    /// A name that cannot be an Ordo name. Fix: pass `--name`.
    Name { name: String, why: String },
    /// The block the generator wrote does not compile, and the name is not
    /// the cause. Carries the compiler's message and the block it tried.
    Block { why: String, text: String },
    /// The database could not be read. Retry.
    Database(String),
}

impl IntrospectError {
    /// The greppable class.
    pub fn class(&self) -> &'static str {
        match self {
            IntrospectError::Missing { .. } => "external_missing",
            IntrospectError::NoKey { .. } => "external_no_key",
            IntrospectError::Name { .. } => "external_name",
            IntrospectError::Block { .. } => "external_block",
            IntrospectError::Database(_) => "database",
        }
    }
}

impl std::fmt::Display for IntrospectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntrospectError::Missing { schema, table } => write!(
                f,
                "external_missing: `{schema}.{table}` is not in this database, or this role \
                 cannot see it. Check the name, and that the role has USAGE on `{schema}` \
                 and SELECT on the table"
            ),
            IntrospectError::NoKey { schema, table, why } => write!(
                f,
                "external_no_key: `{schema}.{table}` has no key Ordo can use: {why}. Name a \
                 column that is unique and never null with `--key <column>`"
            ),
            IntrospectError::Name { name, why } => write!(
                f,
                "external_name: `{name}` cannot be the entity's name ({why}). Pass \
                 `--name <entity>`, a lowerCamel word"
            ),
            IntrospectError::Block { why, text } => write!(
                f,
                "external_block: the block does not compile: {why}\nThe block it tried:\n{text}"
            ),
            IntrospectError::Database(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for IntrospectError {}

/// What the generator reads of one table.
pub struct TableRead<'a> {
    pub schema: &'a str,
    pub table: &'a str,
    pub shape: &'a LiveTable,
    pub keys: &'a [UniqueKey],
    /// A table, whose keys and nulls the catalog states. A view or a
    /// materialized view has no constraints to read, so `--key` is taken as
    /// the developer's word for it.
    pub verifiable: bool,
}

/// What the caller chose.
#[derive(Default)]
pub struct Options {
    /// `--name`: the entity's name. Derived from the table otherwise.
    pub name: Option<String>,
    /// `--key`: the key column, when the primary key is not the one.
    pub key: Option<String>,
}

/// The Ordo spelling of a Postgres type, or why it has none.
pub enum Mapped {
    /// The type as written after `:`, and a note for the reviewer.
    Type(String, Option<&'static str>),
    Unmapped(&'static str),
}

/// `migration.md` section 2a, for one column type as `format_type` spells it.
pub fn map_type(pg: &str) -> Mapped {
    let lower = pg.trim().to_ascii_lowercase();
    let base = lower.split('(').next().unwrap_or(&lower).trim();
    match base {
        "text" | "character varying" | "character" | "citext" | "name" => {
            Mapped::Type("text".into(), None)
        }
        "smallint" | "integer" | "bigint" => Mapped::Type("int".into(), None),
        "boolean" => Mapped::Type("bool".into(), None),
        "date" => Mapped::Type("date".into(), None),
        "timestamp with time zone" => Mapped::Type("datetime".into(), None),
        "timestamp without time zone" => Mapped::Type(
            "datetime".into(),
            Some("no zone in the column, read as UTC"),
        ),
        "numeric" => match numeric_scale(&lower) {
            Some(s) => Mapped::Type(format!("decimal({s})"), None),
            None => Mapped::Unmapped("numeric with no scale has no fixed places"),
        },
        "uuid" => Mapped::Unmapped("a uuid is only a key here, never a field"),
        "jsonb" | "json" => Mapped::Unmapped("on the worklist: json is read whole, by hand"),
        "bytea" => Mapped::Unmapped("a blob is Ordo's own store, not a foreign column"),
        "real" | "double precision" => Mapped::Unmapped("a float has no exact decimal identity"),
        "interval" => Mapped::Unmapped("on the worklist: an interval may mix units"),
        t if t.ends_with("[]") => Mapped::Unmapped("on the worklist: arrays"),
        _ => Mapped::Unmapped("on the worklist: no Ordo type"),
    }
}

/// `numeric(10,2)` has scale 2, `numeric(10)` scale 0, bare `numeric` none.
fn numeric_scale(t: &str) -> Option<u8> {
    let inner = t.split_once('(')?.1.strip_suffix(')')?;
    match inner.split_once(',') {
        Some((_, s)) => s.trim().parse().ok(),
        None => inner.trim().parse::<u32>().ok().map(|_| 0),
    }
}

/// The key line's type: nothing for an integer key, which is the default.
fn key_type(pg: &str) -> Option<Option<&'static str>> {
    let lower = pg.to_ascii_lowercase();
    let base = lower.split('(').next().unwrap_or(&lower).trim();
    match base {
        "smallint" | "integer" | "bigint" => Some(None),
        "uuid" => Some(Some("uuid")),
        "text" | "character varying" | "character" | "citext" => Some(Some("text")),
        _ => None,
    }
}

/// `customer_id` to `customerId`: the lowerCamel Ordo word.
pub fn lower_camel(s: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for c in s.chars() {
        if !c.is_ascii_alphanumeric() {
            up = !out.is_empty();
            continue;
        }
        if out.is_empty() {
            out.push(c.to_ascii_lowercase());
        } else if up {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c);
        }
        up = false;
    }
    out
}

/// A foreign name as the file writes it: bare when it is a plain word,
/// quoted otherwise (`"customer_id"`). `all` quotes every name, the
/// fallback when a bare one collides with a reserved word.
fn spell(name: &str, all: bool) -> String {
    let bare = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    if bare && !all {
        name.to_string()
    } else {
        format!("\"{name}\"")
    }
}

/// Does the parser take this block?
fn parses(text: &str) -> Result<(), String> {
    crate::load_sources(&crate::Sources::from_parts(vec![(
        "external.ordo".into(),
        text.to_string(),
    )]))
    .map(|_| ())
}

/// Pick the key column and its type word.
fn pick_key(
    t: &TableRead,
    opts: &Options,
) -> Result<(String, Option<&'static str>), IntrospectError> {
    let no_key = |why: String| IntrospectError::NoKey {
        schema: t.schema.to_string(),
        table: t.table.to_string(),
        why,
    };
    let column = match &opts.key {
        Some(k) => {
            let unique = t
                .keys
                .iter()
                .any(|u| u.columns.len() == 1 && u.columns[0] == *k);
            if !t.shape.columns.contains_key(k) {
                return Err(no_key(format!("`--key {k}` is not a column of the table")));
            }
            if !unique && t.verifiable {
                return Err(no_key(format!(
                    "`--key {k}` is not unique: no single-column unique index covers it"
                )));
            }
            k.clone()
        }
        None => match t.keys.iter().find(|u| u.primary) {
            Some(u) if u.columns.len() == 1 => u.columns[0].clone(),
            Some(u) => {
                return Err(no_key(format!(
                    "its primary key is the composite ({}), and an external's id is one column",
                    u.columns.join(", ")
                )))
            }
            None => return Err(no_key("it has no primary key".into())),
        },
    };
    let col = &t.shape.columns[&column];
    if col.nullable && t.verifiable {
        return Err(no_key(format!("the key column `{column}` admits null")));
    }
    match key_type(&col.ty) {
        Some(word) => Ok((column, word)),
        None => Err(no_key(format!(
            "the key column `{column}` is `{}`, and a key is an integer, text or uuid",
            col.ty
        ))),
    }
}

/// The `external` block for one table, as text.
///
/// Pure: no database. The text is checked against the parser before it is
/// returned, so what comes back is a block the compiler takes.
pub fn external_block(t: &TableRead, opts: &Options) -> Result<String, IntrospectError> {
    let (key_column, key_word) = pick_key(t, opts)?;
    let name = opts.name.clone().unwrap_or_else(|| lower_camel(t.table));
    // The name alone first, so a name the parser refuses is blamed as one
    // and any other failure is shown as the compiler said it.
    let key_ty = key_word.map(|w| format!(": {w}")).unwrap_or_default();
    let alone = format!("external {name} from s.t shared {{\n  id{key_ty} <- k\n}}\n");
    if let Err(why) = parses(&alone) {
        return Err(IntrospectError::Name { name, why });
    }
    let mut last = (String::new(), String::new());
    for quote_all in [false, true] {
        let text = render(t, &name, &key_column, key_word, quote_all);
        match parses(&text) {
            Ok(()) => return Ok(text),
            Err(why) => last = (why, text),
        }
    }
    Err(IntrospectError::Block {
        why: last.0,
        text: last.1,
    })
}

fn render(
    t: &TableRead,
    name: &str,
    key_column: &str,
    key_word: Option<&str>,
    quote_all: bool,
) -> String {
    let head = format!(
        "external {name} from {}.{} shared {{\n",
        spell(t.schema, quote_all),
        spell(t.table, quote_all)
    );
    let probe = |line: &str| {
        parses(&format!(
            "{head}  id <- {}\n  {line}\n}}\n",
            spell(key_column, quote_all)
        ))
    };
    let mut out = format!(
        "# Generated by `ordo introspect --external {}.{}`. Review it before you apply it.\n",
        t.schema, t.table
    );
    out.push_str(&head);
    match key_word {
        Some(w) => out.push_str(&format!("  id: {w} <- {}\n", spell(key_column, quote_all))),
        None => out.push_str(&format!("  id <- {}\n", spell(key_column, quote_all))),
    }
    // `id` is the key line's name, so a field cannot take it.
    let mut used: Vec<String> = vec!["id".to_string()];
    for (column, c) in &t.shape.columns {
        if column == key_column {
            continue;
        }
        let ty = match map_type(&c.ty) {
            Mapped::Type(ty, note) => (ty, note),
            Mapped::Unmapped(why) => {
                out.push_str(&format!("  # unmapped: `{column}` is `{}`; {why}\n", c.ty));
                continue;
            }
        };
        let field = lower_camel(column);
        let line = format!(
            "{field}: {}{} <- {}",
            ty.0,
            if c.nullable { "?" } else { "" },
            spell(column, quote_all)
        );
        if used.contains(&field) {
            out.push_str(&format!(
                "  # unmapped: `{column}` would be a second `{field}`; give it a name by hand\n"
            ));
            continue;
        }
        if let Err(e) = probe(&line) {
            out.push_str(&format!(
                "  # unmapped: `{column}` ({}) cannot be the field `{field}`: {e}\n",
                c.ty
            ));
            continue;
        }
        used.push(field);
        match ty.1 {
            Some(note) => out.push_str(&format!("  {line}  # {note}\n")),
            None => out.push_str(&format!("  {line}\n")),
        }
    }
    out.push_str("}\n");
    out
}

/// Split `schema.table`.
fn split_target(s: &str) -> Option<(&str, &str)> {
    let (a, b) = s.split_once('.')?;
    (!a.is_empty() && !b.is_empty() && !b.contains('.')).then_some((a, b))
}

/// Read the live table and generate its block.
pub async fn generate(
    pool: &ordo_exec::Pool,
    schema: &str,
    table: &str,
    opts: &Options,
) -> Result<String, IntrospectError> {
    let db = |e: ordo_exec::PoolError| IntrospectError::Database(e.to_string());
    // The same reader the apply-time check uses, so a view or a partitioned
    // table the runtime reads is one the generator writes.
    let Some(columns) = migrate::table_columns(pool, schema, table)
        .await
        .map_err(db)?
    else {
        return Err(IntrospectError::Missing {
            schema: schema.to_string(),
            table: table.to_string(),
        });
    };
    let verifiable = migrate::is_table(pool, schema, table).await.map_err(db)?;
    let live = LiveTable {
        columns,
        ..Default::default()
    };
    let keys = migrate::unique_keys(pool, schema, table)
        .await
        .map_err(db)?;
    external_block(
        &TableRead {
            schema,
            table,
            shape: &live,
            keys: &keys,
            verifiable,
        },
        opts,
    )
}

/// `ordo introspect --external <schema>.<table> [--name <entity>]
/// [--key <column>] [--out <file>] [--db <database>]`.
pub(crate) fn run(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--external", "--name", "--key", "--out", "--db"]) {
        return Outcome::usage(format!("{e}\n"));
    }
    let Some(target) = flags.named_value("--external") else {
        return Outcome::usage(
            "ordo introspect --external <schema>.<table> [--name <entity>] [--key <column>] \
             [--out <file>] [--db <database>]\n(the whole-database form is not built yet)\n"
                .into(),
        );
    };
    let Some((schema, table)) = split_target(target) else {
        return Outcome::usage(format!(
            "`--external {target}`: write the schema and the table, `crm.customers`\n"
        ));
    };
    let opts = Options {
        name: flags.named_value("--name").map(str::to_string),
        key: flags.named_value("--key").map(str::to_string),
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return Outcome::failed(format!("runtime: {e}")),
    };
    let text = rt.block_on(async {
        let pool = connect(&flags).await?;
        generate(&pool, schema, table, &opts)
            .await
            .map_err(|e| Outcome::failed(format!("{e}\n")))
    });
    let text = match text {
        Ok(t) => t,
        Err(o) => return o,
    };
    match flags.named_value("--out") {
        Some(path) => match std::fs::write(path, &text) {
            Ok(()) => Outcome::ok(format!("{path}\n")),
            Err(e) => Outcome::failed(format!("{path}: {e}\n")),
        },
        None => Outcome::ok(text),
    }
}
