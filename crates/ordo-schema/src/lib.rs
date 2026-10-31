//! P2 — the schema graph and its DDL.
//!
//! Entity declarations in, a resolved graph out: ref edges pointed at real
//! entities, implicit `id`/`rev`/`depth` materialised, ownership and tree
//! marks checked, unique and sealed marks recorded. The graph is the thing
//! the binder resolves against and the thing the DDL is printed from, so
//! there is exactly one place that knows what a field is.
//!
//! # Scope, stated plainly
//!
//! This is the slice of P2 that the composition corpus's execution tier
//! needs, not all of P2. Specifically **not** implemented here, and rejected
//! by name rather than silently mishandled:
//!
//! - `acl` derivation and per-entity effective guard tables (P2/G15). Nothing
//!   in this crate derives a link entity or computes a guard.
//! - Dot-path `check` constraints and their trigger mechanism.
//! - Real sealing. A `sealed` field gets its declared payload column and, when
//!   `indexed`, a blind-index digest column — but no key is derived and
//!   nothing is encrypted, because that is F0's (`ordo-crypto`) work and F0
//!   does not exist. Reading a sealed field is refused downstream in
//!   `ordo-sema`; it is never quietly served as plaintext.
//! - Per-space columns. `UNIQUE (space, hmac)` degrades to `UNIQUE (hmac)`
//!   here because spaces are an F-track concept this crate cannot see.

use std::collections::{HashMap, HashSet};

use ordo_syntax::decl::{
    EntityDecl, FieldKind, GuardAtom, GuardClause, GuardExpr, Program, ScalarTy,
};
use ordo_syntax::span::Span;
use ordo_syntax::Expr;

pub mod check;
pub mod ddl;
pub mod diff;
pub mod function;
pub mod image;
pub mod json;
pub mod routine;

pub use ddl::{ddl_for, SPACE_COLUMN};
pub use diff::{diff, diff_with, Change, Class, LiveColumn, LiveShape, LiveTable, PlacesChoice};

/// A resolved field type. Refs carry the resolved target entity name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldTy {
    Scalar(ScalarTy),
    Ref {
        target: String,
        owned: bool,
        tree: bool,
    },
}

impl FieldTy {
    /// The type a *value* of this field has in an expression. A ref's value
    /// is the target's id — dotting is what reaches the target's fields.
    pub fn value_ty(&self) -> ScalarTy {
        match self {
            FieldTy::Scalar(s) => *s,
            FieldTy::Ref { .. } => ScalarTy::Id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: FieldTy,
    pub optional: bool,
    pub unique: bool,
    pub sealed: bool,
    pub indexed: bool,
    /// `sealed indexed refinable` — the field accepts a caller's `$refine`
    /// condition (`std-library.md` §2.1 rule 1, `definition-language.md`
    /// cost classes).
    ///
    /// **Sealed fields default to `false` and open fields are always
    /// refinable**, which is why this flag reads as a sealed-field opt-in
    /// even though it is stored on every field: refining a sealed field is a
    /// disclosure — it answers "is this row's ciphertext this value?" — so
    /// the capability is authored, lands in the op's manifest, and therefore
    /// lands on the consent page. It also carries a **cost**: the flag is
    /// the only reason `ordo_plan::cost::refine_field_class` is ever asked
    /// about a sealed field, and the answer can be host-side work
    /// (§7.1).
    pub refinable: bool,
    /// `blob private` — client-sealed custody (DL §2). The platform holds
    /// no key for these bytes.
    ///
    /// Carried through the schema so the shape summary, the consent page
    /// and every runtime refusal can name the tier. **Every path that
    /// would have to touch the plaintext refuses**: F7 Phase 6 owns the
    /// passkey-PRF derivation, the device wrapping and the presigned
    /// direct-to-bucket upload that make it servable.
    pub private: bool,
    /// `blob inline` — the bytes live in the sealed payload store rather
    /// than in the space's object store (DL §2, pin 2).
    ///
    /// **A storage-tier flag, orthogonal to `sealed`.** DL is explicit
    /// that the three custody tiers *"compose with `inline` orthogonally"*,
    /// so this does not imply sealing and sealing does not imply this. What
    /// it does decide is where the bytes are, and therefore three things
    /// that follow from that: the column type
    /// ([`ddl::column_ty`]), whether the object store is reached at all,
    /// and whether the garbage story is `$blobtomb`'s two-phase sweep or
    /// the pruning transaction's existing anti-join.
    ///
    /// Only `blob` may carry it. On any other type it is a refusal with
    /// the word pointed at, because `inline` on a `text` field is not a
    /// no-op — it is an author believing something about where their data
    /// lives.
    pub inline: bool,
    /// `normalize(lower)` — runs before the blind-index digest.
    pub normalize: Option<String>,
    /// The closed value set a `text` field is pinned to, in declaration
    /// order — DL's `enum("read", "write", "admin")`.
    ///
    /// There is no *declaration* form for this yet (DL lists `enum(…)` as
    /// lean), and this is deliberately not half of one: it is the pin the
    /// `acl` derivation needs for `level`, which the spec spells as an
    /// enum and which nothing else can be trusted to spell. The value type
    /// stays `text` — an enum's ordering is a property of the ladder, not
    /// of a new scalar — so every consumer that asks `value_ty()` is
    /// unaffected, while the DDL gets a `CHECK` and the write path gets a
    /// refusal. Without it `level: "root"` is a value outside the declared
    /// ladder, and after level ordering lands that is a value whose
    /// position in the order is undefined.
    pub enum_values: Option<Vec<String>>,
    /// `title: text(64)` — the declared length bound of a `text` field, in
    /// characters (DL §2, Types). Two enforcers, one number: an open
    /// column gets a row invariant `char_length(col) <= n` among the
    /// entity's `checks` ([`check::text_bound_checks`]), so the DDL, the
    /// schema diff and the artifact all carry it; and the write path refuses an
    /// over-long value before any SQL runs, which is the only enforcer a
    /// sealed column can have (the database holds ciphertext and cannot
    /// count its characters). The derived std ops carry it onto their
    /// parameters so the door refuses first. `None` for every other type
    /// and for an unbounded `text`.
    pub text_max: Option<u32>,
    pub default: Option<Expr>,
    /// A public op may serve this field to a visitor who is not signed in
    /// (`plan-syntax-additions.md` §1). **Resolved**: the field's own
    /// `public`, or its entity's `public` line, or an owner's `public*`
    /// reaching it. Never set on a sealed field or a `blob private` one;
    /// the schema refuses those (`public_sealed`, `public_private`).
    pub public: bool,
    /// Platform-provided (`id`, `rev`, `depth`): addressable by name, but not
    /// part of `[*]`, which means the fields the author declared.
    pub implicit: bool,
    pub span: Span,
}

impl Field {
    /// Column holding the field's value.
    pub fn column(&self) -> String {
        self.name.clone()
    }

    /// Column holding the blind index of a `sealed indexed` field. The digest
    /// is a separate column because the payload is opaque by construction.
    pub fn index_column(&self) -> String {
        format!("{}$bidx", self.name)
    }

    /// Does this field hold **content** — bytes the platform stores and
    /// serves — rather than a value?
    ///
    /// The predicate every content refusal shares, so they all mean the
    /// same thing rather than each spelling `matches!(..., Blob)`. A ref
    /// answers `false`; so does `uri`, which holds an address, and an
    /// address is a value.
    pub fn is_content(&self) -> bool {
        match &self.ty {
            FieldTy::Scalar(s) => s.is_content(),
            FieldTy::Ref { .. } => false,
        }
    }
}

/// Where an entity's effective guard came from — printed by `ordo check`,
/// because "looks guarded, isn't" dies by disclosure, not by hoping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardSource {
    Own,
    /// Inherited down the owned subtree from this ancestor (G6), with the
    /// walk rebased through the ownership path in `rebase`.
    Inherited {
        from: String,
    },
    /// Derived by `acl` (R-G14).
    AclDerived,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardBody {
    Walk(GuardExpr),
    /// `access <verb> open` — deliberately, greppably unguarded (G9).
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guards {
    pub read: GuardBody,
    pub write: GuardBody,
    pub source: GuardSource,
    /// Ownership-path prefix an inherited walk is rebased through:
    /// taskNotes inherits tasks' guard as `task.<walk>` (G6).
    pub rebase: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    /// Provided by the platform, not declared by an author.
    ///
    /// DL §3: a platform entity's fields are *"all read-only to domain
    /// code"* and *"there is no domain-authored path to minting one"*.
    /// Nothing enforced that — `members` was an ordinary writable entity, so
    /// `set members { id: $id, role: "owner" }` compiled, registered and ran,
    /// and `deployment_engine` builds the door's access engine by reading
    /// the very column a domain op could write. An editor could promote
    /// themselves, and everyone else, to owner.
    pub platform: bool,
    pub name: String,
    /// The typed-id prefix (`prod` in `prod_01J8…`): explicit `(key: …)`
    /// or derived — camelCase words, each abbreviated by the common-word
    /// table or truncated to 4 chars, hyphen-joined. Minting is the write
    /// path's job; the KEY is a schema fact, fixed here so every log line
    /// ever emitted stays self-describing.
    pub key: String,
    pub fields: Vec<Field>,
    /// True when some ref on this entity is marked `tree`.
    pub is_tree: bool,
    /// The effective row-level guard, resolved: own clauses, inherited
    /// down the owned subtree, or acl-derived. None = unguarded.
    pub guards: Option<Guards>,
    /// Declared `acl (…)` levels, in order (the order IS the ordering).
    pub acl_levels: Option<Vec<String>>,
    /// Multi-column unique keys, each a field list in declared order —
    /// DL's `unique (project, member)`. A single-column key stays on the
    /// field (`Field::unique`); this is only for the composite form, which
    /// no field can carry on its own.
    ///
    /// Two producers, and they are the same rule reached two ways: the
    /// author's `unique (a, b)` clause, and the `acl` derivation's
    /// `unique (<root>, member)` — R-G17's converge-or-create grant has no
    /// key to converge on without it, and a second grant to the same
    /// member would otherwise create a second row. The derivation came
    /// first (9e built the syntax onto the pin it had already made), which
    /// is why every consumer already existed.
    pub unique_keys: Vec<Vec<String>>,
    /// Declared `index (a, b)` — extra secondary indexes (DL §3, "Indexes
    /// [settled]"). Ref fields index automatically and single-column
    /// uniqueness carries its own index, so this holds only what the graph
    /// does not already imply. Non-unique, so it constrains nothing and
    /// can raise no error class at runtime; it is a cost decision the
    /// author states and the DDL carries out. The indexes the table
    /// actually carries, refs included, are [`Entity::all_indexes`].
    pub indexes: Vec<Vec<String>>,
    /// Row invariants (`check used <= seats`) — validated row-local, lowered
    /// to real CHECK constraints by the DDL. See [`check`] for the scope and
    /// the refusals.
    pub checks: Vec<check::CheckRule>,
    /// `std sealed` (DL §3): the derived std ops include this entity's
    /// sealed fields. Read by [`std_ops`] and nothing else; a hand-written
    /// op's manifest is its own.
    pub std_sealed: bool,
    /// The entity line `public` / `public*`, its own or reached through an
    /// owner's `public*`. Every declared field of a marked entity is public.
    pub public: Option<EntityPublic>,
    pub span: Span,
}

/// Where an entity's `public` line came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityPublic {
    /// `public*`: the mark reaches the entities this one owns.
    pub cascade: bool,
    /// The owner whose `public*` reached this entity; `None` for its own
    /// line.
    pub from: Option<String>,
    /// The line that carries the mark (the owner's, when inherited).
    pub span: Span,
}

impl Entity {
    /// Does any declared field of this entity reach a public op?
    pub fn has_public(&self) -> bool {
        self.fields.iter().any(|f| f.public)
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// The fields `[*]` expands to: what the author wrote, in source order.
    pub fn declared(&self) -> impl Iterator<Item = &Field> {
        self.fields.iter().filter(|f| !f.implicit)
    }

    /// Every plain index this entity's table carries: the declared ones
    /// ([`Entity::indexes`]), then **one per ref field** (DL §3, "Ref
    /// fields index automatically", reported fix 25y).
    ///
    /// A ref is what a join walks and what a delete of its target checks
    /// for referrers. The composite foreign key does not index the
    /// referencing side, so without this each of those is a scan of the
    /// child table. The index is `(space, ref)` and named as the field
    /// modifier `index` names it (`task$board$ix`), so `board: ref Board
    /// index` and the automatic index are one index.
    ///
    /// **Left out where another index already leads with the ref.** A
    /// btree on `(space, board, x)` answers every lookup on `(space,
    /// board)`, so a second index would cost a write per row and answer
    /// nothing new. That covers a declared `index (board, x)`, a composite
    /// `unique (board, member)` and a `unique` ref field, each of which
    /// Postgres backs with exactly such an index.
    ///
    /// Read by the DDL, the schema diff and the apply. The artifact keeps
    /// [`Entity::indexes`], what the author declared.
    pub fn all_indexes(&self) -> Vec<Vec<String>> {
        let leads = |name: &str| {
            self.indexes
                .iter()
                .chain(&self.unique_keys)
                .any(|k| k.first().is_some_and(|c| c == name))
        };
        let mut out = self.indexes.clone();
        for f in &self.fields {
            let unique = f.unique && !f.implicit;
            if matches!(f.ty, FieldTy::Ref { .. }) && !f.sealed && !unique && !leads(&f.name) {
                out.push(vec![f.name.clone()]);
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub domain: Option<String>,
    pub entities: Vec<Entity>,
    by_name: HashMap<String, usize>,
    /// The program's `release member` rule, resolved against the entities
    /// (27aq). `None` is **locked**: no member leaves on their own.
    pub release: Option<ordo_syntax::ReleaseDecl>,
    /// The program's routines (checklist 12a), checked against the
    /// entities by [`routine::check`]. Not ops: nothing registers them where
    /// a caller can reach them.
    pub routines: Vec<ordo_syntax::RoutineDecl>,
    /// The program's `secret` declarations (`plan-backend-functions.md`
    /// §5): each name and the hosts it may be sent to. One per name.
    pub secrets: Vec<ordo_syntax::SecretDecl>,
    /// The program's backend functions, checked by [`function::check`].
    /// Called through `POST /v1/fn/<name>`, never as ops.
    pub functions: Vec<ordo_syntax::FunctionDecl>,
}

impl Schema {
    pub fn entity(&self, name: &str) -> Option<&Entity> {
        self.by_name.get(name).map(|i| &self.entities[*i])
    }

    /// Every ref as a multigraph edge, in declaration order (deterministic:
    /// ambiguity errors enumerate paths, and enumeration order must not
    /// churn goldens). Tree self-loops excluded.
    pub fn ref_edges(&self) -> Vec<RefEdge> {
        let mut out = Vec::new();
        for e in &self.entities {
            for f in &e.fields {
                if let FieldTy::Ref { target, tree, .. } = &f.ty {
                    if *tree {
                        continue;
                    }
                    out.push(RefEdge {
                        from: e.name.clone(),
                        field: f.name.clone(),
                        to: target.clone(),
                        optional: f.optional,
                    });
                }
            }
        }
        out
    }
}

/// One declared ref, viewed as an edge of the undirected multigraph match
/// inference walks (DL ~1589). Parallel edges stay distinct entries — that
/// is the point: `transfers.fromAccount` and `.toAccount` are two paths.
/// Tree self-loops are excluded (`under`/`above` walk those, never joins).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefEdge {
    pub from: String,
    pub field: String,
    pub to: String,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    /// Stable class name — assertions quote these, never the prose.
    pub code: &'static str,
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {} at {}", self.code, self.message, self.span)
    }
}

impl std::error::Error for SchemaError {}

/// Common-word abbreviations for typed-id keys — words whose 4-char
/// truncation is either ambiguous or has a stronger convention (Stripe's
/// id-prefix registry and classic data-model vocabulary). Curated, not
/// clever: an entry earns its slot only where truncation is wrong.
const KEY_ABBREVIATIONS: &[(&str, &str)] = &[
    ("transactions", "txns"),
    ("accounts", "acct"),
    ("spaces", "spc"),
    ("subscriptions", "subs"),
    ("invoices", "inv"),
    ("charges", "chg"),
    ("payments", "pay"),
    ("messages", "msgs"),
    ("addresses", "addr"),
    ("organizations", "orgs"),
    ("quantities", "qty"),
    ("amounts", "amt"),
    ("numbers", "num"),
    ("documents", "docs"),
    ("notifications", "notif"),
    ("configurations", "cfg"),
    ("departments", "dept"),
    ("employees", "emp"),
    ("statistics", "stats"),
    ("parameters", "params"),
    ("references", "refs"),
    ("permissions", "perms"),
    ("administrators", "admin"),
    ("environments", "envs"),
    ("repositories", "repos"),
    ("languages", "lang"),
    ("balances", "bal"),
    ("schedules", "sched"),
    // General-English shortenings (the classic lists: mgmt, dept, qty…),
    // filtered to words plausible as entity names. `revisions` and
    // `standards` are deliberately absent: `rev` collides with the
    // implicit field and `std` with the std library namespace.
    ("management", "mgmt"),
    ("miscellaneous", "misc"),
    ("apartments", "apt"),
    ("buildings", "bldg"),
    ("governments", "govt"),
    ("appointments", "appt"),
    ("corporations", "corp"),
    ("headquarters", "hq"),
    ("information", "info"),
    ("laboratories", "lab"),
    ("licenses", "lic"),
    ("locations", "loc"),
    ("manufacturers", "mfr"),
    ("packages", "pkg"),
    ("receipts", "rcpt"),
    ("records", "rec"),
    ("registrations", "reg"),
    ("requirements", "reqs"),
    ("sequences", "seq"),
    ("signatures", "sig"),
    ("temperatures", "temp"),
    ("universities", "univ"),
    ("versions", "ver"),
    ("volumes", "vol"),
    ("estimates", "est"),
    ("figures", "fig"),
    ("divisions", "div"),
    ("directors", "dir"),
    ("publications", "pub"),
    ("positions", "pos"),
];

/// The abbreviation table key derivation consults: the built-ins, with a
/// user's own words merged over them (user wins).
#[derive(Debug, Clone, Default)]
pub struct KeyTable {
    user: Vec<(String, String)>,
}

impl KeyTable {
    /// Parse a user table: one `word=abbreviation` per line, `#` comments
    /// and blank lines ignored. A malformed line is a loud error — a
    /// silently dropped abbreviation would surface years later as a
    /// surprising id prefix.
    pub fn parse(text: &str) -> Result<KeyTable, String> {
        let mut user = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((w, a)) = line.split_once('=') else {
                return Err(format!(
                    "line {}: expected `word=abbreviation`, got `{line}`",
                    i + 1
                ));
            };
            let (w, a) = (w.trim(), a.trim());
            if w.is_empty() || a.is_empty() || !a.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(format!("line {}: bad entry `{line}`", i + 1));
            }
            user.push((w.to_lowercase(), a.to_lowercase()));
        }
        Ok(KeyTable { user })
    }

    /// Load from the `ORDO_KEY_ABBREVIATIONS` env path, when set.
    pub fn from_env() -> Result<KeyTable, String> {
        match std::env::var("ORDO_KEY_ABBREVIATIONS") {
            Err(_) => Ok(KeyTable::default()),
            Ok(path) => {
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("ORDO_KEY_ABBREVIATIONS ({path}): {e}"))?;
                KeyTable::parse(&text)
            }
        }
    }

    fn lookup(&self, word: &str) -> Option<&str> {
        self.user
            .iter()
            .find(|(w, _)| w == word)
            .map(|(_, a)| a.as_str())
            .or_else(|| {
                KEY_ABBREVIATIONS
                    .iter()
                    .find(|(w, _)| *w == word)
                    .map(|(_, a)| *a)
            })
    }
}

/// Derive an entity's typed-id key: split camelCase, abbreviate each word
/// by the table or truncate to 4 chars, hyphen-join.
pub fn derive_key_with(name: &str, table: &KeyTable) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in name.chars() {
        if c.is_uppercase() && !cur.is_empty() {
            words.push(cur.clone());
            cur.clear();
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
        .iter()
        .map(|w| {
            table
                .lookup(w)
                .map(str::to_string)
                .unwrap_or_else(|| w.chars().take(4).collect())
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Derivation against the built-in table alone.
pub fn derive_key(name: &str) -> String {
    derive_key_with(name, &KeyTable::default())
}

/// Names the platform owns on every entity. An author who declares one gets a
/// compile error rather than a shadowed implicit column.
const RESERVED: &[&str] = &["id", "rev", "depth"];

/// Build the graph. Every ref target is resolved here or the build fails —
/// there is no such thing as a partially resolved schema downstream.
pub fn build(program: &Program) -> Result<Schema, SchemaError> {
    build_with_keys(program, &KeyTable::default())
}

/// The platform-provided principal entity (spec §3): per space, refable,
/// undeclarable. Domains `-> members`; the platform owns the shape.
fn platform_members() -> Entity {
    let f = |name: &str, ty: ScalarTy, optional: bool| Field {
        name: name.into(),
        ty: FieldTy::Scalar(ty),
        optional,
        unique: false,
        sealed: false,
        indexed: false,
        refinable: true,
        inline: false,
        private: false,
        normalize: None,
        enum_values: None,
        text_max: None,
        default: None,
        public: false,
        implicit: false,
        span: Span::new(0, 0, 1, 1),
    };
    let mut fields = vec![
        Field {
            name: "id".into(),
            ty: FieldTy::Scalar(ScalarTy::Id),
            optional: false,
            unique: true,
            sealed: false,
            indexed: false,
            refinable: true,
            inline: false,
            private: false,
            normalize: None,
            enum_values: None,
            text_max: None,
            default: None,
            public: false,
            implicit: true,
            span: Span::new(0, 0, 1, 1),
        },
        Field {
            name: "rev".into(),
            ty: FieldTy::Scalar(ScalarTy::Int),
            optional: false,
            unique: false,
            sealed: false,
            indexed: false,
            refinable: true,
            inline: false,
            private: false,
            normalize: None,
            enum_values: None,
            text_max: None,
            default: None,
            public: false,
            implicit: true,
            span: Span::new(0, 0, 1, 1),
        },
    ];
    fields.extend([
        f("principal", ScalarTy::Text, false),
        f("displayName", ScalarTy::Text, false),
        f("kind", ScalarTy::Text, false),
        f("memberCount", ScalarTy::Int, false),
        f("role", ScalarTy::Text, true),
        f("status", ScalarTy::Text, false),
        f("joined", ScalarTy::Date, false),
    ]);
    Entity {
        // The one entity this is true of today. DL §3 makes `members`
        // read-only to domain code; nothing enforced it until now.
        platform: true,
        name: "members".into(),
        key: "mem".into(),
        fields,
        is_tree: false,
        guards: None,
        acl_levels: None,
        std_sealed: false,
        public: None,
        unique_keys: Vec::new(),
        indexes: Vec::new(),
        checks: Vec::new(),
        span: Span::new(0, 0, 1, 1),
    }
}

/// The platform's group-membership link entity (DL §3, Groups `[settled]`).
///
/// `groupMembers { group -> members owned, member -> members owned }`, per
/// space, platform-maintained and undeclarable — the same status as `members`
/// itself — but **queryable and walkable like any entity**, *"which is what
/// makes group-aware guards and headcounts writable instead of magical"*.
///
/// That last clause is why it has to exist as a real entity rather than as a
/// rule inside the access engine. Group expansion in acl rows and profiles is
/// the access layer's, but a GUARD that wants group semantics writes the walk,
/// and there was nothing to walk: `== $caller` deliberately never expands a
/// group, so without this entity the escape hatch the language offers had no
/// floor under it.
///
/// Both refs point at `members`, so a reverse step into it must name the ref
/// it arrives by — `member.groupMembers.group(…)`.
fn platform_group_members() -> Entity {
    let implicit = |name: &str, ty: ScalarTy| Field {
        name: name.into(),
        ty: FieldTy::Scalar(ty),
        optional: false,
        unique: name == "id",
        sealed: false,
        indexed: false,
        refinable: true,
        inline: false,
        private: false,
        normalize: None,
        enum_values: None,
        text_max: None,
        default: None,
        public: false,
        implicit: true,
        span: Span::new(0, 0, 1, 1),
    };
    let group_ref = |name: &str| Field {
        name: name.into(),
        ty: FieldTy::Ref {
            target: "members".into(),
            owned: true,
            tree: false,
        },
        optional: false,
        unique: false,
        sealed: false,
        indexed: false,
        refinable: true,
        inline: false,
        private: false,
        normalize: None,
        enum_values: None,
        text_max: None,
        default: None,
        public: false,
        implicit: false,
        span: Span::new(0, 0, 1, 1),
    };
    Entity {
        platform: true,
        name: "groupMembers".into(),
        key: "gm".into(),
        fields: vec![
            implicit("id", ScalarTy::Id),
            implicit("rev", ScalarTy::Int),
            group_ref("group"),
            group_ref("member"),
        ],
        is_tree: false,
        guards: None,
        acl_levels: None,
        std_sealed: false,
        public: None,
        // One row per (group, member): a person is in a group once. Without
        // it, "remove Sam from Finance" would have to delete an unknown
        // number of rows and could leave one behind.
        unique_keys: vec![vec!["group".into(), "member".into()]],
        indexes: Vec::new(),
        checks: Vec::new(),
        span: Span::new(0, 0, 1, 1),
    }
}

/// Build with a user abbreviation table merged over the built-ins.
pub fn build_with_keys(program: &Program, keys: &KeyTable) -> Result<Schema, SchemaError> {
    let mut entities: Vec<Entity> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();

    // `members` is platform-provided and undeclarable (GC088).
    by_name.insert("members".into(), 0);
    entities.push(platform_members());
    // Same status as `members` — platform-provided, undeclarable, and
    // registered in the name map so a guard walk can find it. Without the
    // map entry the derived group disjunct compiles to "`groupMembers` is
    // not an entity", which is true of the lookup and false of the schema.
    by_name.insert("groupMembers".into(), 1);
    entities.push(platform_group_members());

    for decl in &program.entities {
        if decl.name.name == "members" {
            return Err(SchemaError {
                code: "parse_error",
                message: "`members` is platform-provided and cannot be declared".into(),
                span: decl.name.span,
            });
        }
        if by_name.contains_key(&decl.name.name) {
            return Err(SchemaError {
                code: "duplicate_entity",
                message: format!("entity `{}` is declared twice", decl.name.name),
                span: decl.name.span,
            });
        }
        by_name.insert(decl.name.name.clone(), entities.len());
        entities.push(lower_entity(decl, keys)?);
    }

    // Ref targets resolve against the completed name set, so forward
    // references are legal and a typo is caught with the name it meant.
    for decl in program.entities.iter() {
        let ei = by_name[&decl.name.name];
        for f in &decl.fields {
            let FieldKind::Ref { target, tree, .. } = &f.kind else {
                continue;
            };
            if !by_name.contains_key(&target.name) {
                return Err(SchemaError {
                    code: "unknown_ref_target",
                    message: format!(
                        "`{}.{}` points at `{}`, which is not a declared entity",
                        decl.name.name, f.name.name, target.name
                    ),
                    span: target.span,
                });
            }
            if *tree && target.name != decl.name.name {
                return Err(SchemaError {
                    code: "tree_not_self_ref",
                    message: format!(
                        "`{}.{}` is marked `tree` but points at `{}` — a tree edge is a self-edge",
                        decl.name.name, f.name.name, target.name
                    ),
                    span: target.span,
                });
            }
        }
        // The implicit `depth` only exists once the tree mark is confirmed.
        if entities[ei].is_tree {
            entities[ei].fields.push(Field {
                name: "depth".into(),
                ty: FieldTy::Scalar(ScalarTy::Int),
                optional: false,
                unique: false,
                sealed: false,
                indexed: false,
                refinable: true,
                inline: false,
                private: false,
                normalize: None,
                enum_values: None,
                text_max: None,
                default: None,
                public: false,
                implicit: true,
                span: decl.name.span,
            });
        }
    }

    resolve_guards(&mut entities, &mut by_name, keys)?;
    resolve_public(&mut entities)?;

    // An op name `acl` derives is reserved, for the same reason a derived
    // entity or field name is (DL §acl, `reserved_derived_name`): the
    // registry is a flat namespace, so a domain op called
    // `grantBoardsAccess` would either shadow the derived one or be
    // shadowed by it, and either way the name a caller grants would not be
    // the op they think they are granting. Refused with the name.
    let derived_ops: Vec<String> = entities
        .iter()
        .filter(|e| e.acl_levels.is_some())
        .flat_map(|e| derived_op_names(&format!("{}Access", e.name)))
        .collect();
    for op in &program.ops {
        if derived_ops.contains(&op.name.name) {
            return Err(SchemaError {
                code: "reserved_derived_name",
                message: format!(
                    "`{}` is an op name `acl` derives; it cannot also be declared",
                    op.name.name
                ),
                span: op.name.span,
            });
        }
    }

    // Typed-id keys must be unique — a collision is a compile failure
    // naming BOTH entities, because these prefixes go into every id ever
    // logged and can never quietly mean two things.
    for i in 0..entities.len() {
        for j in (i + 1)..entities.len() {
            if entities[i].key == entities[j].key {
                return Err(SchemaError {
                    code: "id_key_collision",
                    message: format!(
                        "`{}` and `{}` both derive id key `{}` — give one an explicit key: `entity {} (key: …)`",
                        entities[i].name, entities[j].name, entities[i].key, entities[j].name
                    ),
                    span: entities[j].span,
                });
            }
        }
    }

    if let Some(r) = &program.release {
        resolve_release(&entities, &by_name, r)?;
    }

    let mut secret_names = HashSet::new();
    for s in &program.secrets {
        if !secret_names.insert(s.name.name.as_str()) {
            return Err(SchemaError {
                code: "duplicate_secret",
                message: format!(
                    "secret `{}` is declared twice; list every host on one line: \
                     `secret {} for a.example.com, b.example.com`",
                    s.name.name, s.name.name
                ),
                span: s.span,
            });
        }
    }

    let schema = Schema {
        domain: program.domain.as_ref().map(|d| d.name.name.clone()),
        entities,
        by_name,
        release: program.release.clone(),
        routines: program.routines.clone(),
        secrets: program.secrets.clone(),
        functions: program.functions.clone(),
    };
    routine::check(&schema, program)?;
    function::check(&schema, program)?;
    Ok(schema)
}

/// Check a `release member` rule against the entities (27aq).
///
/// | Class | Refused |
/// |---|---|
/// | `release_unknown_entity` | an entity the program does not declare |
/// | `release_on_sealed` | a sealed field anywhere in a walk (G3 holds) |
/// | `invalid_guard_walk` | a walk the guard grammar cannot resolve |
/// | `invalid_release_effect` | an effect on a field that is not a ref to `members`, or on a platform entity |
/// | `duplicate_release_effect` | two effects on one ref |
fn resolve_release(
    entities: &[Entity],
    by_name: &HashMap<String, usize>,
    r: &ordo_syntax::ReleaseDecl,
) -> Result<(), SchemaError> {
    let known = |name: &ordo_syntax::Ident| -> Result<(), SchemaError> {
        if by_name.contains_key(&name.name) {
            Ok(())
        } else {
            Err(SchemaError {
                code: "release_unknown_entity",
                message: format!(
                    "the release rule names `{}`, which is not a declared entity",
                    name.name
                ),
                span: name.span,
            })
        }
    };
    let walk = |home: &str, w: &GuardExpr| -> Result<(), SchemaError> {
        validate_walk(entities, by_name, home, w).map_err(|e| {
            if e.code == "guard_on_sealed" {
                SchemaError {
                    code: "release_on_sealed",
                    message: e
                        .message
                        .replace("guards walk", "a release rule walks")
                        .replace("(G3)", "(G3 holds for release rules)"),
                    span: e.span,
                }
            } else {
                e
            }
        })
    };
    // The field an effect or a lookup names: a plaintext ref to `members`
    // on an entity the program declared.
    let member_ref = |entity: &ordo_syntax::Ident,
                      field: &ordo_syntax::Ident,
                      what: &str|
     -> Result<(), SchemaError> {
        known(entity)?;
        let e = &entities[by_name[&entity.name]];
        if e.platform {
            return Err(SchemaError {
                code: "invalid_release_effect",
                message: format!(
                    "`{}` is the platform's; leaving already settles its rows",
                    entity.name
                ),
                span: entity.span,
            });
        }
        let Some(f) = e.field(&field.name) else {
            return Err(SchemaError {
                code: "invalid_release_effect",
                message: format!("`{}` has no field `{}`", entity.name, field.name),
                span: field.span,
            });
        };
        if f.sealed {
            return Err(SchemaError {
                code: "release_on_sealed",
                message: format!(
                    "`{}.{}` is sealed; a release rule reads the plaintext skeleton only",
                    entity.name, field.name
                ),
                span: field.span,
            });
        }
        match &f.ty {
            FieldTy::Ref { target, .. } if target == "members" => Ok(()),
            _ => Err(SchemaError {
                code: "invalid_release_effect",
                message: format!(
                    "`{}.{}` is not a ref to `members`; {what}",
                    entity.name, field.name
                ),
                span: field.span,
            }),
        }
    };
    if let ordo_syntax::ReleaseMode::When(cond) = &r.mode {
        for atom in cond.disjuncts.iter().flatten() {
            known(&atom.entity)?;
            walk(&atom.entity.name, &atom.inner)?;
        }
    }
    let mut seen: Vec<(String, String)> = Vec::new();
    for effect in &r.effects {
        let (entity, field, span) = match effect {
            ordo_syntax::ReleaseEffect::Remove {
                entity,
                field,
                span,
            } => {
                member_ref(
                    entity,
                    field,
                    "`remove` takes the ref that points at the seat",
                )?;
                (entity, field, span)
            }
            ordo_syntax::ReleaseEffect::Reassign {
                entity,
                field,
                to,
                span,
            } => {
                member_ref(
                    entity,
                    field,
                    "`reassign` moves a ref that points at the seat",
                )?;
                for t in to {
                    if let ordo_syntax::ReassignTarget::Lookup {
                        entity: le,
                        inner,
                        field: lf,
                        ..
                    } = t
                    {
                        known(le)?;
                        walk(&le.name, inner)?;
                        member_ref(le, lf, "a reassignment goes to a member")?;
                    }
                }
                (entity, field, span)
            }
        };
        let key = (entity.name.clone(), field.name.clone());
        if seen.contains(&key) {
            return Err(SchemaError {
                code: "duplicate_release_effect",
                message: format!(
                    "`{}.{}` already has an effect in this release rule",
                    entity.name, field.name
                ),
                span: *span,
            });
        }
        seen.push(key);
    }
    Ok(())
}

fn lower_entity(decl: &EntityDecl, keys: &KeyTable) -> Result<Entity, SchemaError> {
    let mut fields = vec![
        Field {
            name: "id".into(),
            ty: FieldTy::Scalar(ScalarTy::Id),
            optional: false,
            unique: true,
            sealed: false,
            indexed: false,
            refinable: true,
            inline: false,
            private: false,
            normalize: None,
            enum_values: None,
            text_max: None,
            default: None,
            public: false,
            implicit: true,
            span: decl.name.span,
        },
        Field {
            name: "rev".into(),
            ty: FieldTy::Scalar(ScalarTy::Int),
            optional: false,
            unique: false,
            sealed: false,
            indexed: false,
            refinable: true,
            inline: false,
            private: false,
            normalize: None,
            enum_values: None,
            text_max: None,
            default: None,
            public: false,
            implicit: true,
            span: decl.name.span,
        },
    ];

    let mut owners = 0usize;
    let mut is_tree = false;
    // The single-column indexes declared as field modifiers (`index`),
    // lowered beside the entity's `index (a, b)` clauses: one list, one DDL
    // emitter, one diff (reported fix 25w).
    let mut field_indexes: Vec<Vec<String>> = Vec::new();

    for f in &decl.fields {
        if RESERVED.contains(&f.name.name.as_str()) {
            return Err(SchemaError {
                code: "reserved_field_name",
                message: format!(
                    "`{}` is provided by the platform on every entity and cannot be declared",
                    f.name.name
                ),
                span: f.name.span,
            });
        }
        if fields.iter().any(|x| x.name == f.name.name) {
            return Err(SchemaError {
                code: "duplicate_field",
                message: format!("`{}.{}` is declared twice", decl.name.name, f.name.name),
                span: f.name.span,
            });
        }
        if f.refinable && !f.sealed {
            return Err(SchemaError {
                code: "refinable_without_sealed",
                message: format!(
                    "`{}.{}` is `refinable` without `sealed` — every open field already accepts \
                     refinement conditions, so the marker would be decoration; it exists to open \
                     a SEALED field to them",
                    decl.name.name, f.name.name
                ),
                span: f.name.span,
            });
        }
        if f.indexed && !f.sealed {
            return Err(SchemaError {
                code: "indexed_without_sealed",
                message: format!(
                    "`{}.{}` is `indexed` without `sealed` — a blind index indexes a sealed \
                     value; a plain index on an open field is `{}: … index`",
                    decl.name.name, f.name.name, f.name.name
                ),
                span: f.name.span,
            });
        }
        // The other way round (reported fix 25w): `index` is the plain
        // single-column index, and a sealed payload is ciphertext, so an
        // index over it orders nothing. The index a sealed field can have
        // is the blind index, `indexed`.
        if f.index && f.sealed {
            return Err(SchemaError {
                code: "index_on_sealed",
                message: format!(
                    "`{}.{}` is `sealed` and marked `index` — a sealed payload is ciphertext, \
                     so a plain index over it answers no query; the index a sealed field \
                     takes is the blind index, `{}: … sealed indexed`",
                    decl.name.name, f.name.name, f.name.name
                ),
                span: f.name.span,
            });
        }
        if f.index {
            field_indexes.push(vec![f.name.name.clone()]);
        }
        check_content_modifiers(&decl.name.name, f)?;
        check_json_modifiers(&decl.name.name, f)?;

        let ty = match &f.kind {
            FieldKind::Scalar(s) => FieldTy::Scalar(*s),
            FieldKind::Ref {
                target,
                owned,
                tree,
            } => {
                if *owned {
                    owners += 1;
                }
                if *tree {
                    is_tree = true;
                }
                FieldTy::Ref {
                    target: target.name.clone(),
                    owned: *owned,
                    tree: *tree,
                }
            }
        };

        // The default expression set is closed (DL §2, R-G2 / G4), and
        // `$caller` is the member of it that is not merely unsupported but
        // *wrong* — see `check_default`.
        if let Some(d) = &f.default {
            check_default(&decl.name.name, &f.name.name, d)?;
        }

        // A money or decimal default is written in major units and must fit
        // the declared places (27ak): `= 10.50` on a `money(2)` stores 1050,
        // and `= 10.505` would have to round, which nothing does on its own.
        if let (FieldTy::Scalar(s), Some(d)) = (&ty, &f.default) {
            if let Err(message) = stored_numeric(*s, d) {
                return Err(SchemaError {
                    code: "implicit_rounding",
                    message: format!("`{}.{}`: {message}", decl.name.name, f.name.name),
                    span: d.span(),
                });
            }
        }

        // A duration default must be expressible in the field's declared unit.
        // Checked here rather than at DDL time so `ordo check` catches it
        // without anyone having to generate a schema first.
        if let (
            FieldTy::Scalar(ScalarTy::Duration(decl)),
            Some(Expr::DurationLit { n, unit, span }),
        ) = (&ty, &f.default)
        {
            if ddl::convert_duration(i128::from(*n), *unit, *decl).is_none() {
                return Err(SchemaError {
                    code: "duration_unit_mismatch",
                    message: format!(
                        "`{}.{}` is `duration({})` and its default `{n}{}` has no exact value in that unit",
                        decl.as_str(),
                        f.name.name,
                        decl.as_str(),
                        unit.as_str()
                    ),
                    span: *span,
                });
            }
        }

        fields.push(Field {
            name: f.name.name.clone(),
            ty,
            optional: f.optional,
            unique: f.unique,
            sealed: f.sealed,
            indexed: f.indexed,
            // Open fields are refinable by construction; the marker is only
            // ever the sealed opt-in (checked above), so the schema's flag
            // is "this field accepts a refinement condition" for both.
            //
            // A `json` field takes no condition at all (28b), so it is
            // never refinable, open or sealed.
            refinable: (f.refinable || !f.sealed)
                && !matches!(f.kind, FieldKind::Scalar(ScalarTy::Json)),
            inline: f.inline,
            private: f.private,
            normalize: f.normalize.as_ref().map(|i| i.name.clone()),
            enum_values: None,
            text_max: f.text_max,
            default: f.default.clone(),
            public: f.public,
            implicit: false,
            span: f.name.span,
        });
    }

    // One owner max — EXCEPT the double-owned link entity, which the spec
    // blesses explicitly (G7: two candidate inheritance sources, hence
    // none; whole: "through double-owned links"). Three is nothing.
    if owners > 2 {
        return Err(SchemaError {
            code: "multiple_owners",
            message: format!(
                "`{}` declares {owners} `owned` refs — one owner, or exactly two for a link entity",
                decl.name.name
            ),
            span: decl.name.span,
        });
    }

    // G9: one verb without the other is not spellable-by-omission.
    let guards = match (&decl.access_read, &decl.access_write) {
        (None, None) => None,
        (Some(r), Some(w)) => Some(Guards {
            read: clause_body(r),
            write: clause_body(w),
            source: GuardSource::Own,
            rebase: Vec::new(),
        }),
        (Some(_), None) | (None, Some(_)) => {
            return Err(SchemaError {
                code: "incomplete_access_clause",
                message: format!(
                    "`{}` declares one `access` verb without the other — both or neither; open access is spelled `access <verb> open`",
                    decl.name.name
                ),
                span: decl.name.span,
            })
        }
    };
    if decl.acl.is_some() && guards.is_some() {
        return Err(SchemaError {
            code: "invalid_guard_walk",
            message: format!(
                "`{}` declares both `acl` and `access` clauses",
                decl.name.name
            ),
            span: decl.name.span,
        });
    }

    // An entity's `unique (a, b)` and `index (a, b)` clauses refuse a
    // `json` column for the field modifiers' reason (28b).
    for (clause, keys) in [("unique", &decl.unique_keys), ("index", &decl.indexes)] {
        for col in keys.iter().flatten() {
            if fields
                .iter()
                .any(|f| f.name == col.name && f.ty == FieldTy::Scalar(ScalarTy::Json))
            {
                return Err(SchemaError {
                    code: "json_opaque",
                    message: format!(
                        "rule: a `json` field is read and written whole and no condition may \
                         name it, so `{}.{}` cannot be in a `{clause}` clause.\n\
                         fix: keep the value you need to find rows by in its own typed field",
                        decl.name.name, col.name
                    ),
                    span: col.span,
                });
            }
        }
    }

    let mut checks = check::build_checks(&decl.name.name, &fields, &decl.checks)?;
    checks.extend(check::text_bound_checks(&decl.name.name, &fields));

    Ok(Entity {
        platform: false,
        name: decl.name.name.clone(),
        key: decl
            .key
            .as_ref()
            .map(|k| k.name.clone())
            .unwrap_or_else(|| derive_key_with(&decl.name.name, keys)),
        fields,
        is_tree,
        guards,
        acl_levels: decl
            .acl
            .as_ref()
            .map(|ls| ls.iter().map(|l| l.name.clone()).collect()),
        unique_keys: decl
            .unique_keys
            .iter()
            .map(|k| k.iter().map(|c| c.name.clone()).collect())
            .collect(),
        indexes: field_indexes
            .into_iter()
            .chain(
                decl.indexes
                    .iter()
                    .map(|k| k.iter().map(|c| c.name.clone()).collect()),
            )
            .collect(),
        std_sealed: decl.std_sealed,
        public: decl.public.map(|m| EntityPublic {
            cascade: m.cascade,
            from: None,
            span: m.span,
        }),
        checks,
        span: decl.span,
    })
}

/// Shape defaults may not mention `$caller` — class `caller_in_default`
/// (`definition-language.md` G4 / R-G2, `guards-conformance.md` GC069).
///
/// # Why this is its own refusal and not just "unsupported"
///
/// The declaration-site refusals a **content** type carries (DL §2).
///
/// Every one of these is a modifier that means something real on an
/// ordinary field and means nothing — or something false — on a blob, so
/// each is refused by name rather than accepted and ignored. Silently
/// ignoring a modifier is how an author comes to believe their file column
/// is unique.
///
/// Each error takes the teaching shape `adoption-dx-plan.md` §3.3 commits
/// to: what the rule is, then what to write instead.
///
/// `uri` appears in none of them. It is an address, and an address is a
/// value: comparable, sortable, uniquable, sealable, blind-indexable. Only
/// `blob` — the type that holds *content* — is opaque.
/// The modifiers a `json` field refuses, and the defaults it takes (28b).
///
/// A document is read and written whole: no condition may name it, so an
/// index, a uniqueness or a blind index over one would answer no question
/// the language can ask (`json_opaque`). `normalize` shapes text before a
/// digest, and a document has neither. `refinable` opens a field to
/// conditions, and none applies.
///
/// The default is `{}`, `[]` or `null` (the parser spells the first two as
/// their JSON text); `null` only where the field admits null
/// (`json_default`).
fn check_json_modifiers(entity: &str, f: &ordo_syntax::FieldDecl) -> Result<(), SchemaError> {
    if !matches!(f.kind, FieldKind::Scalar(ScalarTy::Json)) {
        return Ok(());
    }
    let name = &f.name.name;
    let modifier = if f.unique {
        Some("unique")
    } else if f.index {
        Some("index")
    } else if f.indexed {
        Some("indexed")
    } else if f.refinable {
        Some("refinable")
    } else if f.normalize.is_some() {
        Some("normalize")
    } else {
        None
    };
    if let Some(m) = modifier {
        return Err(SchemaError {
            code: "json_opaque",
            message: format!(
                "rule: a `json` field is read and written whole and no condition may name \
                 it, so `{entity}.{name}` cannot be `{m}`.\n\
                 fix: keep the value you need to find rows by in its own typed field, and \
                 index that"
            ),
            span: f.name.span,
        });
    }
    if let Some(d) = &f.default {
        let ok = match d {
            Expr::StrLit { value, .. } => value == "{}" || value == "[]",
            Expr::NullLit { .. } => f.optional,
            _ => false,
        };
        if !ok {
            return Err(SchemaError {
                code: "json_default",
                message: format!(
                    "rule: a `json` field's default is `{{}}`, `[]`, or `null` when the field \
                     admits null; `{entity}.{name}` declares another.\n\
                     fix: write `= {{}}` or `= []`, or make the field `json?` for `= null`"
                ),
                span: d.span(),
            });
        }
    }
    Ok(())
}

fn check_content_modifiers(entity: &str, f: &ordo_syntax::FieldDecl) -> Result<(), SchemaError> {
    let is_blob = matches!(f.kind, FieldKind::Scalar(ScalarTy::Blob));
    let name = &f.name.name;

    // `private` is a CUSTODY tier for bytes; `inline` is a STORAGE tier
    // for bytes. Neither means anything on a type that has none.
    if f.private && !is_blob {
        return Err(SchemaError {
            code: "private_without_blob",
            message: format!(
                "rule: `private` is client-sealed custody over a blob's BYTES — the client \
                 encrypts before upload and the platform holds no key — so it is only \
                 meaningful on a `blob` field, and `{entity}.{name}` is not one.\n\
                 fix: drop `private`. To seal an ordinary field under the SPACE's key, \
                 write `sealed`; the two are different custody and mean different things \
                 on a consent page"
            ),
            span: f.name.span,
        });
    }
    // `sealed` is the space DEK; `private` is a key the platform never
    // holds. A field cannot be both, and the pair is not a stronger
    // version of either — it is an author who has not decided who can read
    // the file, which is the one question these modifiers exist to answer.
    if f.private && f.sealed {
        return Err(SchemaError {
            code: "conflicting_custody",
            message: format!(
                "rule: `sealed` seals under the SPACE's key (the platform can open it for \
                 a caller who passes the door) and `private` seals under a key the \
                 platform never holds — `{entity}.{name}` declares both, which does not \
                 name a stronger position, it names two.\n\
                 fix: pick one. `sealed` if the platform must be able to serve, process or \
                 export the bytes; `private` if it must not, and the consent page should \
                 say so"
            ),
            span: f.name.span,
        });
    }
    // `inline` is a storage tier for *bytes*. Nothing else has bytes.
    if f.inline && !is_blob {
        return Err(SchemaError {
            code: "inline_without_blob",
            message: format!(
                "rule: `inline` chooses where a blob's BYTES are stored, so it is only \
                 meaningful on a `blob` field — `{entity}.{name}` is not one.\n\
                 fix: drop `inline`, or declare the field `blob inline` if it holds a file"
            ),
            span: f.name.span,
        });
    }
    if !is_blob {
        return Ok(());
    }

    // **A blob field must admit null**, and the reason is a consequence
    // rather than a convention: the only producer of a blob value is an
    // upload, and an upload addresses a row — `PUT /blob/<entity>/<field>/
    // <id>` — so the row exists first and the field is empty until the
    // bytes land. A required blob field would be a field no `create` could
    // ever fill, which is why the derived `std.E.create` excludes blob
    // fields from its parameters and stays insert-complete: that exclusion
    // is only sound because of this refusal.
    if !f.optional {
        return Err(SchemaError {
            code: "blob_required",
            message: format!(
                "rule: a blob's value comes from an upload against an EXISTING row, so a \
                 `blob` field is empty between the row's creation and the upload — \
                 `{entity}.{name}` must admit null.\n\
                 fix: write `{name}: blob?`; if the file is genuinely mandatory, enforce \
                 it where the workflow completes rather than where the row begins"
            ),
            span: f.name.span,
        });
    }

    // A blind index over a keyed-random handle indexes randomness. It
    // would answer no question, cost a digest per write, and — worst —
    // look from the outside exactly like a working index.
    if f.unique || f.indexed || f.index {
        let modifier = if f.unique {
            "unique"
        } else if f.indexed {
            "indexed"
        } else {
            "index"
        };
        return Err(SchemaError {
            code: "blob_not_indexable",
            message: format!(
                "rule: a `blob` is addressed by a keyed-random handle minted per upload, so \
                 two uploads of one file compare unequal and an index over the column \
                 indexes randomness — `{entity}.{name}` cannot be `{modifier}`.\n\
                 fix: index `{name}.mime` or `{name}.size`, which are ordinary sealed \
                 values, or hold a separate declared field for whatever the uniqueness is \
                 really about"
            ),
            span: f.name.span,
        });
    }

    // The only producer of a blob value is an upload, so there is no
    // expression that could be a default. A `= "…"` here would be an
    // author writing a handle by hand, which names an object they did not
    // put there.
    if f.default.is_some() {
        return Err(SchemaError {
            code: "blob_default",
            message: format!(
                "rule: the only thing that produces a `blob` value is an upload, so \
                 `{entity}.{name}` cannot carry a default — a literal here would be a \
                 handle naming an object nobody uploaded.\n\
                 fix: declare it optional (`{name}: blob?`) and upload through the blob \
                 door after the row exists"
            ),
            span: f.name.span,
        });
    }

    // `normalize` runs over text before a digest. There is no text and no
    // digest.
    if f.normalize.is_some() {
        return Err(SchemaError {
            code: "blob_not_normalizable",
            message: format!(
                "rule: `normalize` canonicalizes TEXT before equality and the blind index, \
                 and a `blob` has neither — `{entity}.{name}` cannot declare one.\n\
                 fix: drop `normalize`; if the canonical form matters, it is a property of \
                 the file's contents and belongs to whatever wrote it"
            ),
            span: f.name.span,
        });
    }

    Ok(())
}

/// A default is a property of the **shape**, not of a call: it is evaluated
/// with no session in scope, at every site that omits the field — including
/// the ones no principal is present for at all (a seed, a migration
/// backfill, a scheduled job). `$caller` there does not mean "whoever is
/// creating this row"; it means *nothing*, and the only ways to make it
/// mean something are worse than the error. Resolve it at declaration and
/// the creating principal of one deployment is baked into the schema for
/// every row after. Resolve it lazily and a field's default silently
/// changes value depending on who happens to be writing — a static
/// declaration that is not static, which is exactly the property defaults
/// exist to have.
///
/// `$caller` is legal on a write's right-hand side (`createdBy: $caller`),
/// which is where "whoever is creating this row" is sayable and where a
/// session always exists. That is the fix this error names.
///
/// Checked recursively: `= coalesce($caller, "")` is the same mistake with
/// a function around it.
fn check_default(entity: &str, field: &str, e: &Expr) -> Result<(), SchemaError> {
    fn find_caller(e: &Expr) -> Option<Span> {
        match e {
            Expr::Param { name, span } if name == "caller" => Some(*span),
            Expr::Param { .. }
            | Expr::IntLit { .. }
            | Expr::DecimalLit { .. }
            | Expr::StrLit { .. }
            | Expr::DateLit { .. }
            | Expr::DatetimeLit { .. }
            | Expr::DurationLit { .. }
            | Expr::BoolLit { .. }
            | Expr::NullLit { .. }
            | Expr::Path { .. } => None,
            Expr::Neg { expr, .. } => find_caller(expr),
            Expr::Binary { lhs, rhs, .. } => find_caller(lhs).or_else(|| find_caller(rhs)),
            Expr::FnCall { args, .. } => args.iter().find_map(find_caller),
            Expr::Cond { arms, else_, .. } => arms
                .iter()
                .find_map(|a| find_caller(&a.value))
                .or_else(|| find_caller(else_)),
        }
    }
    match find_caller(e) {
        None => Ok(()),
        Some(span) => Err(SchemaError {
            code: "caller_in_default",
            message: format!(
                "`{entity}.{field}` defaults to `$caller` — a default is evaluated without a \
                 session, so there is no caller to bind; write it on the statement that creates \
                 the row instead (`set {entity} {{ {field}: $caller, … }}`)"
            ),
            span,
        }),
    }
}

// ── the derived admin ops (DL §acl) ──────────────────────────────────────

/// The op names `acl` derives on one root: grant, revoke, level change.
///
/// A rule, not an example, exactly as the link entity's name is: the
/// derived names are `grant<Link>`, `revoke<Link>` and `set<Link>Level`
/// with the link entity's own name upper-cased at the front — `boards`
/// derives `boardsAccess`, hence `grantBoardsAccess`, `revokeBoardsAccess`
/// and `setBoardsAccessLevel` (GC036).
fn derived_op_names(link_name: &str) -> [String; 3] {
    let mut upper = link_name.to_string();
    if let Some(c) = upper.get(0..1) {
        upper.replace_range(0..1, &c.to_uppercase());
    }
    [
        format!("grant{upper}"),
        format!("revoke{upper}"),
        format!("set{upper}Level"),
    ]
}

/// The **derived admin ops**, as ordinary registered ops (DL §acl:
/// "ordinary registered ops with manifests").
///
/// They are derived rather than hand-written for the same reason the link
/// entity is: an author who has to write them can write them wrong, and
/// the one thing that must not be wrong is the op that changes who may
/// change access. Their guards are not stated here — the link entity's own
/// derived write guard (top level, R-G14) is what gates all three, which
/// is exactly the pin `std-library.md` §2 records for the std surface over
/// the same rows: no privilege gap between `grantBoardsAccess` and any
/// other write to `boardsAccess`.
///
/// The shapes, per root, with `<root>` the link's back-ref name:
///
/// ```text
/// op grantBoardsAccess($board: id, $member: id, $level: text) =
///   set boardsAccess { board: $board, member: $member, level: $level }
///
/// op revokeBoardsAccess($board: id, $member: id) =
///   del boardsAccess(board == $board and member == $member)
///
/// op setBoardsAccessLevel($board: id, $member: id, $level: text) =
///   set boardsAccess(board == $board and member == $member) { level: $level }
/// ```
///
/// `grant` is **converge-or-create on `unique (board, member)`** (R-G17,
/// mode label `creates-or-converges`): regranting an existing holder
/// converges their level rather than minting a second row. That is the
/// composite key doing the work — without it the same statement is a
/// plain create and "grant Dana read, then grant Dana admin" leaves two
/// rows and a revoke that only half works.
///
/// `revoke` and the level change address by that same key. The language
/// has no *addressed* spelling for a composite key (`del E { id: … }` is
/// id-only), so both are written as the condition the key is: over a
/// unique key it matches at most one row, which is what "addressed by the
/// same key" means operationally, and the caller's write view still bounds
/// it (R-G9).
///
/// **Not derived here, and named rather than left to discovery:** R-G17's
/// *surviving-grant-path report* on revoke. It needs the group expansion
/// DL §3 describes (a level can arrive through a group row).
///
/// The reason it gave for not deriving — *"nothing in the tree expands
/// groups yet, so a report would be a list that is empty for the wrong
/// reason"* — has expired. Groups expand (the derived guards below carry the
/// second disjunct) and `groupMembers` has a writer, so the list can be
/// right; `ordo_conformance::membership::grant_paths` is that query,
/// executed and asserted. What is still owed is the derivation: turning it
/// into an **op** on every acl root, with the result shape and the manifest
/// entries that follow. Recorded so the stale reason does not keep standing
/// in for the real one.
/// Is this entity an `acl`-derived link entity, and if so what does it hang
/// off?
///
/// Derived rather than flagged, from the same two facts `acl_admin_ops`
/// derives its ops from: a root carries `acl_levels`, and its link is named
/// `<Root>Access`. A second flag would be a second source of truth for one
/// thing, and the two would eventually disagree about an entity somebody
/// renamed.
///
/// The space owner's administration capability (DL §3 bootstrap **rule 6**)
/// is scoped to exactly these entities, so this is what the door asks.
pub fn acl_link_root(schema: &Schema, entity: &str) -> Option<String> {
    let root = entity.strip_suffix("Access")?;
    let e = schema.entity(root)?;
    e.acl_levels.as_ref()?;
    Some(root.to_string())
}

pub fn acl_admin_ops(schema: &Schema) -> Program {
    let sp = Span::new(0, 0, 1, 1);
    let ident = |n: &str| ordo_syntax::Ident {
        name: n.to_string(),
        span: sp,
    };
    let param = |n: &str, ty: ScalarTy| ordo_syntax::decl::ParamDecl {
        name: ident(n),
        ty,
        default: None,
        // A derived admin op declares no parameter bound. `<root>` and
        // `member` are ids and `level` is checked against the ladder's
        // enum, which is a stronger statement than any `<=` could make.
        max: None,
        text_max: None,
        filled: false,
        span: sp,
    };
    let param_ref = |n: &str| Expr::Param {
        name: n.to_string(),
        span: sp,
    };
    // `field == $field` — the addressing half of every one of the three.
    let addresses = |fields: &[&str]| ordo_syntax::Condition::And {
        items: fields
            .iter()
            .map(|f| ordo_syntax::Condition::Compare {
                op: ordo_syntax::CmpOp::Eq,
                lhs: Expr::Path {
                    segments: vec![ident(f)],
                    span: sp,
                },
                rhs: param_ref(f),
                span: sp,
            })
            .collect(),
        span: sp,
    };

    let mut ops = Vec::new();
    for e in &schema.entities {
        let Some(_) = e.acl_levels.as_ref() else {
            continue;
        };
        let link_name = format!("{}Access", e.name);
        if schema.entity(&link_name).is_none() {
            continue;
        }
        let back = e.name.strip_suffix('s').unwrap_or(&e.name).to_string();
        let key = [back.as_str(), "member"];
        let [grant, revoke, set_level] = derived_op_names(&link_name);

        let op = |name: String,
                  params: Vec<ordo_syntax::decl::ParamDecl>,
                  body: ordo_syntax::decl::OpBody| {
            ordo_syntax::decl::OpDecl {
                name: ident(&name),
                params,
                flags: ordo_syntax::decl::OpFlags::default(),
                body,
                span: sp,
            }
        };

        ops.push(op(
            grant,
            vec![
                param(&back, ScalarTy::Id),
                param("member", ScalarTy::Id),
                param("level", ScalarTy::Text),
            ],
            ordo_syntax::decl::OpBody::Write(vec![ordo_syntax::decl::WriteStmt::Set {
                // A derived op names nothing: these three are the platform's
                // own grant/revoke/set-level, and nothing follows them in
                // their group to use a name.
                binding: None,
                entity: ident(&link_name),
                cond: None,
                fields: vec![
                    (ident(&back), param_ref(&back)),
                    (ident("member"), param_ref("member")),
                    (ident("level"), param_ref("level")),
                ],
                span: sp,
            }]),
        ));
        ops.push(op(
            revoke,
            vec![param(&back, ScalarTy::Id), param("member", ScalarTy::Id)],
            ordo_syntax::decl::OpBody::Write(vec![ordo_syntax::decl::WriteStmt::Del {
                entity: ident(&link_name),
                cond: addresses(&key),
                subtree: false,
                span: sp,
            }]),
        ));
        ops.push(op(
            set_level,
            vec![
                param(&back, ScalarTy::Id),
                param("member", ScalarTy::Id),
                param("level", ScalarTy::Text),
            ],
            ordo_syntax::decl::OpBody::Write(vec![ordo_syntax::decl::WriteStmt::Set {
                // A derived op names nothing: these three are the platform's
                // own grant/revoke/set-level, and nothing follows them in
                // their group to use a name.
                binding: None,
                entity: ident(&link_name),
                cond: Some(addresses(&key)),
                fields: vec![(ident("level"), param_ref("level"))],
                span: sp,
            }]),
        ));
    }
    Program {
        domain: None,
        entities: Vec::new(),
        ops,
        release: None,
        routines: Vec::new(),
        secrets: Vec::new(),
        functions: Vec::new(),
    }
}

/// The **std library** (`std-library.md`): the baseline operation surface
/// `ordo apply` derives for every entity, under the reserved `std.`
/// namespace, as a pure function of the schema.
///
/// Derived exactly the way [`acl_admin_ops`] is — `OpDecl` nodes built from
/// the declaration and handed to `Registry::extend_from`, then through
/// `ordo_plan::cost::check_registration` with every authored op. There is no
/// second mechanism and no exemption: a std op binds through the same binder,
/// so its guards inline (§5.1), and it faces the same cost, venue, row-bound
/// and write gates. A std op that could not be registered is **not derived**,
/// which is the whole of §5.7's promise — *std derives nothing it cannot
/// bound* — expressed as code rather than as a hope.
///
/// # What is derived, and what is not
///
/// Of the twelve rows in §2's derivation table, eight are derived here:
/// `get`, `list`, `count`, `create`, `put`, `update`, `delete`,
/// `deleteSubtree`. The other four are **deliberately absent**, each for a
/// stated reason, and the reasons are different in kind:
///
/// - **`whole` — no body can hold it.** `whole` is not an [`OpBody`] variant;
///   it is a standalone read (`ordo_syntax::parse_whole`) returning an anchor,
///   with no way to name it inside an op. Its row bound also rests on a
///   per-collection cap whose value §10 marks **[open]** and explicitly
///   declines to guess.
/// - **`under` and `rollup` are [lean]**, and a [lean] construct is not owed.
///   `rollup` additionally needs the declared relationship and depth caps its
///   bounded-closure test reads, which the language does not spell. (`under`
///   no longer meets `list`'s old blocker — the 2026-09-05 paging ruling
///   would carry it too — but [lean] is still not owed.)
/// - **`history` is platform-derived, not language-compiled** — §2.3 says so
///   — and the language has no log source to compile. Its field list is
///   [lean].
///
/// `std.space.*` (§3) is absent for a different reason again, and it is a
/// permanent one: those ops read the access engine's **runtime state**, not
/// a table, so there is nothing here to derive them *from*. They live on the
/// platform-op path instead (`ordo_host::platform_ops`, checklist 8r), which
/// is a second **source** of `OpSpec`s — hand-written, because nothing
/// compiles them — rather than the second derivation mechanism this function
/// exists to avoid. They reach the door through the same registry and the
/// same `OpExecutor`, so nothing downstream distinguishes them.
///
/// # The two entities that get nothing, and why
///
/// **`members` is excluded entirely.** It is platform-provided,
/// undeclarable, and **unguarded**, so a derived read over it is a
/// whole-space principal directory and a derived write is an unattributed
/// path to minting, re-roling and deleting principals. Membership is
/// administered by `std.space.*` commands precisely so that every change to
/// it is attributed, chained and revertible; a generic `std.members.update`
/// would be a second door onto the same rows with none of that. §2's
/// "derived entities are entities" licenses std over the `acl`-derived link
/// entities — which carry the link's inherited write guard, and so cannot
/// undercut `grantBoardsAccess` — and says nothing about the platform's own.
///
/// # Rulings taken here
///
/// - **`list` rides the paging ruling, and rides it as an ordinary op.**
///   §5.7 always asserted `list` needs no acknowledgment because it is
///   keyset-paged; as of 2026-09-05 the registration gate agrees
///   (`ordo_plan::cost::keyset_paged`, and the ruling in
///   `corpus-rulings.md`). What is worth noticing is what did NOT happen:
///   no `unbounded` flag, no `take`, and no std-shaped exemption. The
///   evidence the gate accepts for `std.E.list` is the evidence it accepts
///   for any authored listing with a total order, which is the only kind of
///   relaxation this derivation is allowed to depend on.
/// - **Reads derive over the OPEN SKELETON** (§2.4): sealed fields are absent
///   from `get`'s projection and from `update`'s braces. The sealed-inclusive
///   marker §2.4 defers to the language has no spelling in
///   `definition-language.md`, so only the default half exists — which is the
///   safe half, and the one that fails closed.
/// - **`update` drops sealed fields from its PARAMS as well as its braces.**
///   §2.4's table says write params are "unchanged" and, one row later, that
///   `update`'s braces are the open skeleton only. Both cannot hold: a param
///   no brace consumes is a parameter the manifest advertises and the op
///   ignores. Braces win, because that row is the specific one; the params
///   follow them. Nothing is lost — writing a sealed field is `create`/`put`,
///   where §2.4's insert-completeness reason actually bites.
/// - **`create` carries `rev: null` only when the entity has a unique key**
///   — the create-once assertion — so its derived mode is `creates` as §2
///   says, rather than silently becoming `creates-or-converges` on every
///   entity that happens to declare a unique field. `put` is the same
///   braces *without* it, which is what makes the two different ops rather
///   than one op named twice. A **keyless** entity's `create` carries no
///   `rev` at all: DL §6 rule 9 makes `rev` a belief about the row the
///   braces address, and keyless braces address none, so the belief has
///   nothing to be about and the write protocol refuses it by name
///   (`invalid_binding_expr`) on every call — which is what a keyless
///   entity's derived `create` did, unconditionally, before this line
///   distinguished the two cases (checklist 11-10a).
/// - **`create` and `put` need at most one unique key** (`put`: exactly one).
///   §2 states the rule for `put` and says `create` derives "always", but DL
///   §6 rule 8 refuses any statement covering two unique keys — identity is
///   ambiguous and the platform must not pick — so on a two-key entity
///   `create` is unregistrable for the same reason `put` is undecidable.
///   Skipping it applies the language's rule rather than overriding it.
pub fn std_ops(schema: &Schema) -> Program {
    let sp = Span::new(0, 0, 1, 1);
    let ident = |n: &str| ordo_syntax::Ident {
        name: n.to_string(),
        span: sp,
    };
    let param = |n: &str, ty: ScalarTy, text_max: Option<u32>, default: Option<Expr>| {
        ordo_syntax::decl::ParamDecl {
            name: ident(n),
            ty,
            default,
            // Like the acl admin ops: a derived op declares no `<=` bound.
            // Nothing here is a `take` count, which is the only construct
            // that needs one, and every field param is already pinned by
            // its declared type (and, for an enum field, by the ladder).
            max: None,
            // A `text(64)` field's parameter carries the field's bound, so
            // the door refuses an over-long value before the write path
            // would. The same number, stated once on the field.
            text_max,
            // A derived op fills nothing: every value it takes is data the
            // caller holds, never the caller's clock.
            filled: false,
            span: sp,
        }
    };
    let param_ref = |n: &str| Expr::Param {
        name: n.to_string(),
        span: sp,
    };
    let path = |n: &str| Expr::Path {
        segments: vec![ident(n)],
        span: sp,
    };
    // `id == $id` — the addressing half of every addressed std op.
    let addressed = || ordo_syntax::Condition::Compare {
        op: ordo_syntax::CmpOp::Eq,
        lhs: path("id"),
        rhs: param_ref("id"),
        span: sp,
    };

    let mut ops = Vec::new();
    for e in &schema.entities {
        // Platform entities are excluded, by the FLAG rather than by name.
        // The std derivation excluded `members` by name and was right to;
        // `groupMembers` then arrived and inherited none of that reasoning,
        // so the derivation produced write ops the write path refuses at
        // registration — a build failure rather than a hole, but only
        // because the platform-entity refusal landed first.
        //
        // The reason is `members`' reason: deriving `std.groupMembers.delete`
        // hands every grantee an unattributed path to editing who is in which
        // group, which is the door's own input.
        if e.platform {
            continue;
        }
        let name = |verb: &str| format!("std.{}.{verb}", e.name);
        let op = |name: String,
                  params: Vec<ordo_syntax::decl::ParamDecl>,
                  body: ordo_syntax::decl::OpBody| {
            ordo_syntax::decl::OpDecl {
                name: ident(&name),
                params,
                // No flag, ever. `unguarded` is the bypass std never takes
                // (§5.1) and `unbounded` the acknowledgment no derivation may
                // make on a human's behalf (§5.7); leaving both false is what
                // makes the registration gate a real gate for these ops.
                flags: ordo_syntax::decl::OpFlags::default(),
                body,
                span: sp,
            }
        };

        // The open skeleton: what the author declared, minus what is sealed.
        // `id` and `rev` ride explicitly — they are implicit fields, so `[*]`
        // would not carry them, and §2 puts them in every derived row.
        //
        // **`std sealed` widens this to every declared field** (DL §3,
        // "Sealed fields and derived std ops"): the marker is the authored
        // act that puts the sealed values into `get`/`list`'s projections
        // and `update`'s braces, and so into their manifests and consent
        // pages. The derived ops still take no flag: `get` is bounded by
        // its id pin and `list` by its page, so the sealed projection is
        // bounded host work in both, and the registration gate judges them
        // like any authored op.
        let open: Vec<&Field> = e.declared().filter(|f| e.std_sealed || !f.sealed).collect();
        // The open skeleton a derived UPDATE writes: the open fields minus
        // the content ones, for `writable`'s reason one op over. An
        // unsealed blob field is "open" — its handle is a plain `text`
        // column — but its VALUE still has no parameter form, so a derived
        // update that named it would refuse registration.
        //
        // Leaving a blob field out of `update` is also the correct
        // semantics rather than a concession: a full-row CAS converge sets
        // every field in the braces, so including one would mean every
        // update either re-sends the handle or nulls the file. Re-sending
        // is the caller naming an object it did not upload; nulling is
        // silent data loss on an unrelated edit.
        let open_writable: Vec<&Field> = open.iter().copied().filter(|f| !f.is_content()).collect();
        let declared: Vec<&Field> = e.declared().collect();
        // **What a derived write may set**, which is not the same list.
        //
        // A `blob` is not a parameter (`blob_parameter`) and no expression
        // produces one, so a derived `create` that named a blob field
        // would declare a parameter the compiler refuses and would fail
        // registration for every entity holding a file. Excluding it is
        // sound rather than a workaround: a blob field is required to be
        // optional (`blob_required`), precisely because the only producer
        // of its value is an upload that happens after the row exists — so
        // the braces stay insert-complete without it.
        let writable: Vec<&Field> = declared
            .iter()
            .copied()
            .filter(|f| !f.is_content())
            .collect();
        // `id` is unique and implicit; the count that matters for DL §6
        // rule 8 is over keys an author declared.
        let unique_keys = e.declared().filter(|f| f.unique).count() + e.unique_keys.len();

        // ── std.E.get($id) ────────────────────────────────────────────────
        // Row-bounded by the unique pin on `id`, which is what makes it
        // registrable with no acknowledgment.
        let mut proj: Vec<ordo_syntax::decl::ProjItem> = Vec::new();
        for f in ["id", "rev"] {
            proj.push(ordo_syntax::decl::ProjItem::Field {
                path: vec![ident(f)],
                span: sp,
            });
        }
        if e.is_tree {
            proj.push(ordo_syntax::decl::ProjItem::Field {
                path: vec![ident("depth")],
                span: sp,
            });
        }
        for f in &open {
            proj.push(ordo_syntax::decl::ProjItem::Field {
                path: vec![ident(&f.name)],
                span: sp,
            });
        }
        ops.push(op(
            name("get"),
            vec![param("id", ScalarTy::Id, None, None)],
            ordo_syntax::decl::OpBody::Read(ordo_syntax::decl::Pipeline {
                lets: Vec::new(),
                sources: vec![ordo_syntax::decl::Source {
                    name: ident(&e.name),
                    args: vec![ordo_syntax::decl::SourceArg::Cond(addressed())],
                    projection: Some(proj.clone()),
                    span: sp,
                }],
                steps: Vec::new(),
                span: sp,
            }),
        ));

        // ── std.E.list ────────────────────────────────────────────────────
        // `get`'s row shape with the addressing dropped and §10's default
        // sort put on: `sort -id`. Newest-first, because ids are time-
        // ordered, and the keyset stays stable under writes — new rows land
        // before the cursor, never inside a page already visited.
        //
        // Its row bound is the **page**, per the 2026-09-05 ruling
        // (`corpus-rulings.md`): the door serves every read one keyset page
        // at a time, and this plan's order is total (the declared `-id`,
        // which is also the anchor tiebreak, over one source with no
        // many-side join). `ordo_plan::cost::keyset_paged` is where the
        // compiler checks the half it can see, and it checks it here like
        // it does for an authored op — this derivation gets no private door.
        //
        // Notably NOT a `take`: `plan_paged` puts a take INSIDE the paged
        // subquery, so `take n` would cap the whole listing at n and make
        // row n+1 unreachable by any cursor — a list that stops silently
        // and looks complete, which is worse than one that refuses.
        ops.push(op(
            name("list"),
            Vec::new(),
            ordo_syntax::decl::OpBody::Read(ordo_syntax::decl::Pipeline {
                lets: Vec::new(),
                sources: vec![ordo_syntax::decl::Source {
                    name: ident(&e.name),
                    args: Vec::new(),
                    projection: Some(proj),
                    span: sp,
                }],
                steps: vec![ordo_syntax::decl::Step::Sort {
                    keys: vec![ordo_syntax::decl::SortKey {
                        expr: path("id"),
                        desc: true,
                        span: sp,
                    }],
                    span: sp,
                }],
                span: sp,
            }),
        ));

        // ── std.E.count ───────────────────────────────────────────────────
        // `reduce { n: count }` — a keyless reduce is provably one row, and
        // it projects `id` alone because counting needs no column and a
        // manifest should not claim fields the op never reads.
        ops.push(op(
            name("count"),
            Vec::new(),
            ordo_syntax::decl::OpBody::Read(ordo_syntax::decl::Pipeline {
                lets: Vec::new(),
                sources: vec![ordo_syntax::decl::Source {
                    name: ident(&e.name),
                    args: Vec::new(),
                    projection: Some(vec![ordo_syntax::decl::ProjItem::Field {
                        path: vec![ident("id")],
                        span: sp,
                    }]),
                    span: sp,
                }],
                steps: vec![ordo_syntax::decl::Step::Reduce {
                    aggs: vec![ordo_syntax::decl::AggDecl {
                        out: ident("n"),
                        func: ident("count"),
                        arg: None,
                        arg2: None,
                        cond: None,
                        span: sp,
                    }],
                    span: sp,
                }],
                span: sp,
            }),
        ));

        // ── std.E.create(…) / std.E.put(…) ────────────────────────────────
        // Insert-complete by construction: the braces list every declared
        // field, so no required field can be missing. Sealed fields stay —
        // writing a value is not disclosing it (§2.4).
        let write_fields = |extra: Option<(&str, Expr)>| {
            let mut fields: Vec<(ordo_syntax::Ident, Expr)> = writable
                .iter()
                .map(|f| (ident(&f.name), param_ref(&f.name)))
                .collect();
            if let Some((n, v)) = extra {
                fields.push((ident(n), v));
            }
            fields
        };
        // Required fields become required params; defaulted and nullable
        // fields become defaulted params mirroring the field (§2).
        let create_params: Vec<ordo_syntax::decl::ParamDecl> = writable
            .iter()
            .map(|f| {
                let default = match (&f.default, f.optional) {
                    (Some(d), _) => Some(d.clone()),
                    (None, true) => Some(Expr::NullLit { span: sp }),
                    (None, false) => None,
                };
                param(&f.name, f.ty.value_ty(), f.text_max, default)
            })
            .collect();
        if unique_keys <= 1 {
            // The create-once belief (`rev: null`) states something about
            // the ONE row the unique key addresses (DL §6 rule 9: "`rev`
            // states a belief about the addressed row … legal alongside
            // `id` *or* a unique key"). A keyless entity's create braces
            // address no row at all — there is no key for the belief to be
            // about — so `ordo_plan::writes::set_mode` reads them as
            // `SetMode::Create`, and the write protocol refuses `rev` there
            // by name (`invalid_binding_expr`, `ordo_conformance::write`'s
            // `plan_write_stmt`): "`rev` states a belief about ONE addressed
            // row, and `set E { … }` addresses none". Before this, the
            // derivation carried the belief unconditionally, so
            // `std.<entity>.create` refused every row of every keyless
            // entity — nothing had executed a derived `create` before
            // (11-10a), so the interface-shape tests that name it never
            // called it.
            //
            // A keyed entity keeps the belief: that is `create`'s whole
            // distinction from `put` (see the module doc above) — without
            // it, calling `create` twice with the same key would silently
            // converge onto the first row instead of refusing the second.
            let extra = (unique_keys == 1).then_some(("rev", Expr::NullLit { span: sp }));
            ops.push(op(
                name("create"),
                create_params.clone(),
                ordo_syntax::decl::OpBody::Write(vec![ordo_syntax::decl::WriteStmt::Set {
                    // A derived std write binds nothing: `let x = set …` is an
                    // authored construct, and there is no later statement here
                    // to name the new row from.
                    binding: None,
                    entity: ident(&e.name),
                    cond: None,
                    fields: write_fields(extra),
                    span: sp,
                }]),
            ));
        }
        if unique_keys == 1 {
            ops.push(op(
                name("put"),
                create_params,
                ordo_syntax::decl::OpBody::Write(vec![ordo_syntax::decl::WriteStmt::Set {
                    // A derived std write binds nothing: `let x = set …` is an
                    // authored construct, and there is no later statement here
                    // to name the new row from.
                    binding: None,
                    entity: ident(&e.name),
                    cond: None,
                    fields: write_fields(None),
                    span: sp,
                }]),
            ));
        }

        // ── std.E.update($id, $rev, …) ────────────────────────────────────
        // Full-row CAS converge (§2.2). `$rev` is mandatory — there is no
        // unversioned form — so a stale writer gets `cas_miss` rather than
        // silently winning. The braces are the open skeleton: a sealed field
        // is left untouched, never nulled.
        let mut update_params = vec![
            param("id", ScalarTy::Id, None, None),
            param("rev", ScalarTy::Int, None, None),
        ];
        update_params.extend(
            open_writable
                .iter()
                .map(|f| param(&f.name, f.ty.value_ty(), f.text_max, None)),
        );
        let mut update_fields: Vec<(ordo_syntax::Ident, Expr)> = open_writable
            .iter()
            .map(|f| (ident(&f.name), param_ref(&f.name)))
            .collect();
        update_fields.push((ident("rev"), param_ref("rev")));
        ops.push(op(
            name("update"),
            update_params,
            ordo_syntax::decl::OpBody::Write(vec![ordo_syntax::decl::WriteStmt::Set {
                // A derived std write binds nothing: `let x = set …` is an
                // authored construct, and there is no later statement here
                // to name the new row from.
                binding: None,
                entity: ident(&e.name),
                cond: Some(addressed()),
                fields: update_fields,
                span: sp,
            }]),
        ));

        // ── std.E.delete($id) / std.E.deleteSubtree($id) ──────────────────
        // The cascade is its own op, never a flag on delete (§2): separately
        // named, separately grantable, separately consented.
        ops.push(op(
            name("delete"),
            vec![param("id", ScalarTy::Id, None, None)],
            ordo_syntax::decl::OpBody::Write(vec![ordo_syntax::decl::WriteStmt::Del {
                entity: ident(&e.name),
                cond: addressed(),
                subtree: false,
                span: sp,
            }]),
        ));
        if e.is_tree {
            ops.push(op(
                name("deleteSubtree"),
                vec![param("id", ScalarTy::Id, None, None)],
                ordo_syntax::decl::OpBody::Write(vec![ordo_syntax::decl::WriteStmt::Del {
                    entity: ident(&e.name),
                    cond: addressed(),
                    subtree: true,
                    span: sp,
                }]),
            ));
        }
    }
    Program {
        domain: None,
        entities: Vec::new(),
        ops,
        release: None,
        routines: Vec::new(),
        secrets: Vec::new(),
        functions: Vec::new(),
    }
}

/// Guard resolution: acl derivation, inheritance down owned subtrees, and
/// walk validation. Runs after every ref resolves.
/// Resolve `public` and `public*` onto the fields they reach, and refuse the
/// combinations that cannot be served (`plan-syntax-additions.md` §1).
///
/// - `public*` cascades along `owned` refs only, never plain refs, exactly
///   as guard inheritance does (G6). A child's own `public` or `public*`
///   replaces the inherited one (G8).
/// - `public_sealed`: a sealed value never leaves its tenant in the clear,
///   so neither a field mark nor an entity line may reach a sealed field.
/// - `public_private`: a `blob private` field is sealed on the client, and
///   the platform holds no key to serve it.
/// - `public_guarded`: an entity with a read guard or an `acl` says who
///   reads its rows; a public field says everyone. The two lines disagree.
///
/// Runs after [`resolve_guards`], so an inherited guard counts.
fn resolve_public(entities: &mut [Entity]) -> Result<(), SchemaError> {
    // The cascade, to a fixpoint: an inherited `public*` cascades on.
    loop {
        let mut changed = false;
        for ci in 0..entities.len() {
            if entities[ci].public.is_some() || entities[ci].platform {
                continue;
            }
            let owner = entities[ci].fields.iter().find_map(|f| match &f.ty {
                FieldTy::Ref {
                    target,
                    owned: true,
                    tree: false,
                } if *target != entities[ci].name => Some(target.clone()),
                _ => None,
            });
            let Some(owner) = owner else { continue };
            let Some(mark) = entities
                .iter()
                .find(|e| e.name == owner)
                .and_then(|e| e.public.clone())
            else {
                continue;
            };
            if !mark.cascade {
                continue;
            }
            entities[ci].public = Some(EntityPublic {
                cascade: true,
                from: Some(mark.from.clone().unwrap_or(owner)),
                span: mark.span,
            });
            changed = true;
        }
        if !changed {
            break;
        }
    }
    for e in entities.iter_mut() {
        let line = e.public.clone();
        let entity = e.name.clone();
        for f in e.fields.iter_mut().filter(|f| !f.implicit) {
            let by_line = line.is_some();
            if !(f.public || by_line) {
                continue;
            }
            let span = if f.public {
                f.span
            } else {
                line.as_ref().map_or(f.span, |l| l.span)
            };
            let through = match line.as_ref().and_then(|l| l.from.as_ref()) {
                Some(owner) if !f.public => format!(" (through `public*` on `{owner}`)"),
                _ if !f.public => format!(" (through `public` on `{entity}`)"),
                _ => String::new(),
            };
            if f.sealed {
                return Err(SchemaError {
                    code: "public_sealed",
                    message: format!(
                        "`{entity}.{}` is sealed, so it cannot be public{through}: a sealed \
                         value never leaves its tenant in the clear. Mark the fields you mean \
                         `public` one by one, or remove `sealed`.",
                        f.name
                    ),
                    span,
                });
            }
            if f.private {
                return Err(SchemaError {
                    code: "public_private",
                    message: format!(
                        "`{entity}.{}` is `blob private`, sealed on the client{through}: the \
                         platform holds no key and cannot serve it to anyone.",
                        f.name
                    ),
                    span,
                });
            }
            f.public = true;
        }
        if !e.has_public() {
            continue;
        }
        let guarded = e
            .guards
            .as_ref()
            .is_some_and(|g| matches!(g.read, GuardBody::Walk(_)))
            || e.acl_levels.is_some();
        if guarded {
            let span = line.as_ref().map_or_else(
                || {
                    e.fields
                        .iter()
                        .find(|f| f.public)
                        .map_or(e.span, |f| f.span)
                },
                |l| l.span,
            );
            return Err(SchemaError {
                code: "public_guarded",
                message: format!(
                    "`{entity}` is guarded, so its fields cannot be public: the guard and \
                     `public` disagree about who reads the row. Put public rows in an \
                     unguarded entity, or select them in the public op's body."
                ),
                span,
            });
        }
    }
    Ok(())
}

fn resolve_guards(
    entities: &mut Vec<Entity>,
    by_name: &mut HashMap<String, usize>,
    keys: &KeyTable,
) -> Result<(), SchemaError> {
    // ── acl derivation (DL §acl): mint `<entity>Access`, derive guards ──
    let acl_roots: Vec<usize> = entities
        .iter()
        .enumerate()
        .filter(|(_, e)| e.acl_levels.is_some())
        .map(|(i, _)| i)
        .collect();
    for ri in acl_roots {
        let root = entities[ri].name.clone();
        let levels = entities[ri].acl_levels.clone().expect("checked");
        let link_name = format!("{root}Access");
        if by_name.contains_key(&link_name) {
            return Err(SchemaError {
                code: "reserved_derived_name",
                message: format!("`{link_name}` collides with the name `acl` derives on `{root}`"),
                span: entities[ri].span,
            });
        }
        // Mechanical back-ref name: trailing `s` dropped when present.
        let back = root.strip_suffix('s').unwrap_or(&root).to_string();
        let sp = entities[ri].span;
        let mk_ref = |name: &str, target: &str| Field {
            name: name.into(),
            ty: FieldTy::Ref {
                target: target.into(),
                owned: true,
                tree: false,
            },
            optional: false,
            unique: false,
            sealed: false,
            indexed: false,
            refinable: true,
            inline: false,
            private: false,
            normalize: None,
            enum_values: None,
            text_max: None,
            default: None,
            public: false,
            implicit: false,
            span: sp,
        };
        let mut fields = vec![
            Field {
                name: "id".into(),
                ty: FieldTy::Scalar(ScalarTy::Id),
                optional: false,
                unique: true,
                sealed: false,
                indexed: false,
                refinable: true,
                inline: false,
                private: false,
                normalize: None,
                enum_values: None,
                text_max: None,
                default: None,
                public: false,
                implicit: true,
                span: sp,
            },
            Field {
                name: "rev".into(),
                ty: FieldTy::Scalar(ScalarTy::Int),
                optional: false,
                unique: false,
                sealed: false,
                indexed: false,
                refinable: true,
                inline: false,
                private: false,
                normalize: None,
                enum_values: None,
                text_max: None,
                default: None,
                public: false,
                implicit: true,
                span: sp,
            },
            mk_ref(&back, &root),
            mk_ref("member", "members"),
            Field {
                name: "level".into(),
                ty: FieldTy::Scalar(ScalarTy::Text),
                optional: false,
                unique: false,
                sealed: false,
                indexed: false,
                refinable: true,
                inline: false,
                private: false,
                normalize: None,
                // `level: enum(<the declared ladder>)`, exactly as DL
                // §acl spells the derived shape. It is what makes the
                // ordering below total: a row holding a level outside the
                // ladder has no position in it, so "at or above X" would
                // have no answer for it. The column stays `text`; the
                // ladder is the constraint.
                enum_values: Some(levels.clone()),
                text_max: None,
                default: None,
                public: false,
                implicit: false,
                span: sp,
            },
        ];
        fields.shrink_to_fit();

        // ── derived guards (R-G14), and the level ORDERING ──────────────
        //
        // The declared levels are a **full ordering, left to right** (DL
        // §acl: "each level includes every lower level's rights, for reads
        // *and* writes"), so a derived guard tests "the caller holds a
        // level **at or above** X" — never "the caller holds exactly the
        // top level", which is what made the middle rung of every
        // three-level `acl` decoration. Three thresholds, **positional**
        // in the declared ladder, because the level NAMES are the app's
        // vocabulary and only their ORDER is pinned:
        //
        // - **read** — the bottom rung: any level reads (GC037, GC046).
        // - **write** — the second rung: `acl (read, write, admin)` makes
        //   `write` the threshold for writing the root and its owned
        //   subtree (§3.3's matrix, `cards write (level >= write)`;
        //   GC039). On a two-level ladder it clamps to the top, so
        //   `acl (read, write)` still means "read reads, write writes".
        // - **admin** — the top rung: the derived link entity's own write
        //   guard, direct or through the derived admin ops (R-G14, GC043).
        //
        // Ladders longer than three are the one thing the spec leaves
        // open: under `acl (read, comment, write, admin)` the second rung
        // is `comment`, so `comment` confers write. That is the stated
        // rule applied rather than a guess at what the names mean; an
        // author who wants a rung that reads and does not write puts it
        // first.
        let read_at = 0usize;
        let admin_at = levels.len().saturating_sub(1);
        let write_at = admin_at.min(1);

        // One reverse step whose inner condition is `member == $caller`
        // conjoined with a **closed disjunction of `==` over the
        // qualifying levels** — DL §acl's own words, and the reason the
        // ordering stays inside G1's closed grammar: `level >= "write"` is
        // not spellable there and is not becoming spellable, while the
        // enumeration is, stays one indexed semi-join, and is rendered in
        // full in the shape summary.
        let ident = |n: &str| ordo_syntax::Ident {
            name: n.to_string(),
            span: sp,
        };
        let member_is_caller = GuardAtom::Cmp {
            path: vec![ident("member")],
            caller: true,
            lit: None,
            span: sp,
        };
        let level_is = |l: &str| GuardAtom::Cmp {
            path: vec![ident("level")],
            caller: false,
            lit: Some(ordo_syntax::Expr::StrLit {
                value: l.to_string(),
                span: sp,
            }),
            span: sp,
        };
        // The group half, DL §3 Groups `[settled]`: *"The acl-derived guards
        // carry exactly this second disjunct, and the shape summary renders it
        // in full — the derivation is the access layer, so it expands groups,
        // and it discloses that it does by showing the walk it generated."*
        //
        // It was never emitted, so an acl row naming a GROUP granted nothing.
        // The row landed, the UI showed it, and every guard asked whether the
        // caller's own members row was the one named — which for a group it
        // never is. Silent, and in the widening direction only by omission:
        // people saw less than they were granted, never more.
        //
        // Why a walk rather than a rule: `== $caller` deliberately never
        // expands a group ANYWHERE in the language, because one spelling with
        // two truth values is what principle 1 forbids — and the old rule that
        // did expand it in guards was silently PERMISSIVE. So the derivation,
        // which *is* the access layer, writes the expansion out where a reader
        // can see it. `member.groupMembers.group(…)` names the arriving ref
        // because both of `groupMembers`' refs point at `members`.
        let member_in_group = GuardAtom::Step {
            path: vec![ident("member"), ident("groupMembers"), ident("group")],
            inner: Box::new(GuardExpr {
                disjuncts: vec![vec![member_is_caller.clone()]],
                span: sp,
            }),
            span: sp,
        };
        let walk = |path: &[&str], at: usize| -> GuardExpr {
            // At the bottom rung every declared level qualifies, and
            // `level` is pinned to the ladder, so the disjunction would be
            // a tautology: holding a row IS holding a qualifying level.
            // Emitting it would be three semi-join branches asserting what
            // the enum already guarantees.
            let qualifying = |who: &GuardAtom| -> Vec<Vec<GuardAtom>> {
                if at == read_at || levels.is_empty() {
                    vec![vec![who.clone()]]
                } else {
                    // The same level conjunct either way: a group granted
                    // `write` confers `write`. The level is a property of the
                    // grant, not of how the grantee was named.
                    levels
                        .iter()
                        .skip(at)
                        .map(|l| vec![who.clone(), level_is(l)])
                        .collect()
                }
            };
            let step = |who: &GuardAtom| GuardAtom::Step {
                path: path.iter().map(|p| ident(p)).collect(),
                inner: Box::new(GuardExpr {
                    disjuncts: qualifying(who),
                    span: sp,
                }),
                span: sp,
            };
            // TWO outer disjuncts, which is the shape DL prints:
            //
            //   project.projectMembers(member == $caller)
            //   or project.projectMembers(member.groupMembers.group(member == $caller))
            //
            // One step with an internal `or` is logically the same thing —
            // `EXISTS(a) OR EXISTS(b)` and `EXISTS(a OR b)` agree — but the
            // document says the derived guards "carry exactly this second
            // disjunct", and the shape summary is read by people comparing it
            // against that text. Matching the printed form costs one more
            // semi-join branch the planner will union anyway, and buys a
            // rendering somebody can diff against the spec.
            GuardExpr {
                disjuncts: vec![vec![step(&member_is_caller)], vec![step(&member_in_group)]],
                span: sp,
            }
        };

        // The root, and by inheritance its whole owned subtree: read at
        // any level, write at or above the write rung.
        entities[ri].guards = Some(Guards {
            read: GuardBody::Walk(walk(&[&link_name], read_at)),
            write: GuardBody::Walk(walk(&[&link_name], write_at)),
            source: GuardSource::AclDerived,
            rebase: Vec::new(),
        });

        // The link's own guards: read at any level ON THE PARENT row (who
        // holds access is one honest query), write at the top level —
        // changing access is admin's, and only admin's.
        let link_path = [back.as_str(), link_name.as_str()];
        let link_guards = Guards {
            read: GuardBody::Walk(walk(&link_path, read_at)),
            write: GuardBody::Walk(walk(&link_path, admin_at)),
            source: GuardSource::AclDerived,
            rebase: Vec::new(),
        };
        let link = Entity {
            platform: false,
            name: link_name.clone(),
            key: derive_key_with(&link_name, keys),
            fields,
            is_tree: false,
            guards: Some(link_guards),
            acl_levels: None,
            // `unique (<root>, member)` — DL §acl declares it as part of
            // the derived shape, and R-G17 builds the grant op on it:
            // converge-or-create needs a key to converge on, and without
            // one a second grant to the same member is a second row, so
            // "revoke Dana" leaves Dana with the level the earlier row
            // still confers. A principal's level on a row is ONE fact.
            unique_keys: vec![vec![back.clone(), "member".to_string()]],
            indexes: Vec::new(),
            checks: Vec::new(),
            // The link holds no sealed field, so the marker has nothing to
            // opt in; and it is derived, so nobody could have written one.
            std_sealed: false,
            public: None,
            span: sp,
        };
        by_name.insert(link_name, entities.len());
        entities.push(link);
    }

    // ── inheritance down owned subtrees (G6, G7) ─────────────────────────
    // Iterate to a fixed point: a child's guard may come from a grandparent
    // through a guardless parent.
    loop {
        let mut changed = false;
        for i in 0..entities.len() {
            if entities[i].guards.is_some() {
                continue;
            }
            let owned_refs: Vec<(String, String)> = entities[i]
                .fields
                .iter()
                .filter_map(|f| match &f.ty {
                    FieldTy::Ref {
                        target,
                        owned: true,
                        ..
                    } => Some((f.name.clone(), target.clone())),
                    _ => None,
                })
                .collect();
            // G7: a link entity (two owners) never inherits.
            if owned_refs.len() != 1 {
                continue;
            }
            let (via, parent) = owned_refs.into_iter().next().expect("one");
            let pi = by_name[&parent];
            if let Some(pg) = entities[pi].guards.clone() {
                let from = match &pg.source {
                    GuardSource::Inherited { from } => from.clone(),
                    _ => parent.clone(),
                };
                let mut rebase = vec![via];
                rebase.extend(pg.rebase.clone());
                entities[i].guards = Some(Guards {
                    read: pg.read,
                    write: pg.write,
                    source: GuardSource::Inherited { from },
                    rebase,
                });
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // ── walk validation ──────────────────────────────────────────────────
    for i in 0..entities.len() {
        let Some(g) = entities[i].guards.clone() else {
            continue;
        };
        // An inherited walk's text starts at its ORIGIN entity; the rebase
        // prefix is what carries it down to the child. Validate from there.
        let home = match &g.source {
            GuardSource::Inherited { from } => from.clone(),
            _ => entities[i].name.clone(),
        };
        for body in [&g.read, &g.write] {
            if let GuardBody::Walk(w) = body {
                validate_walk(entities, by_name, &home, w)?;
                if !expr_references_caller(w) {
                    return Err(SchemaError {
                        code: "invalid_guard_walk",
                        message: format!(
                            "`{home}`'s guard never consults `$caller` — a hidden filter is not authorization (G2)"
                        ),
                        span: w.span,
                    });
                }
            }
        }
    }
    Ok(())
}

fn expr_references_caller(e: &GuardExpr) -> bool {
    e.disjuncts.iter().flatten().any(|a| match a {
        GuardAtom::Cmp { caller, .. } => *caller,
        GuardAtom::Step { inner, .. } => expr_references_caller(inner),
        GuardAtom::Bare { .. } => false,
    })
}

/// Resolve every atom of a walk against the ref graph from `home`.
fn validate_walk(
    entities: &[Entity],
    by_name: &HashMap<String, usize>,
    home: &str,
    w: &GuardExpr,
) -> Result<(), SchemaError> {
    for conj in &w.disjuncts {
        for atom in conj {
            match atom {
                GuardAtom::Cmp { path, span, .. } | GuardAtom::Bare { path, span } => {
                    walk_forward(entities, by_name, home, path, *span)?;
                }
                GuardAtom::Step { path, inner, span } => {
                    // Forward hops while the segment is a ref field; the
                    // first non-field segment is the reverse-step entity,
                    // optionally followed by the arriving ref's name.
                    let mut cur = home.to_string();
                    let mut i = 0;
                    while i < path.len() {
                        let seg = &path[i].name;
                        let e = &entities[by_name[&cur]];
                        match e.field(seg).map(|f| &f.ty) {
                            Some(FieldTy::Ref { target, .. }) => {
                                cur = target.clone();
                                i += 1;
                            }
                            Some(_) => {
                                return Err(SchemaError {
                                    code: "invalid_guard_walk",
                                    message: format!(
                                        "`{seg}` is not a ref — a guard walks refs and collections only"
                                    ),
                                    span: *span,
                                })
                            }
                            None => break,
                        }
                    }
                    if i >= path.len() {
                        return Err(SchemaError {
                            code: "invalid_guard_walk",
                            message: "a reverse step names the collection entity".into(),
                            span: *span,
                        });
                    }
                    let step_entity = &path[i].name;
                    let Some(&si) = by_name.get(step_entity) else {
                        return Err(SchemaError {
                            code: "invalid_guard_walk",
                            message: format!(
                                "`{step_entity}` is neither a field of `{cur}` nor an entity"
                            ),
                            span: *span,
                        });
                    };
                    let arriving: Vec<&Field> = entities[si]
                        .fields
                        .iter()
                        .filter(|f| matches!(&f.ty, FieldTy::Ref { target, .. } if target == &cur))
                        .collect();
                    let named = path.get(i + 1).map(|id| id.name.as_str());
                    match (arriving.len(), named) {
                        (0, _) => {
                            return Err(SchemaError {
                                code: "invalid_guard_walk",
                                message: format!(
                                    "`{step_entity}` declares no ref to `{cur}` — the walk cannot arrive"
                                ),
                                span: *span,
                            })
                        }
                        (1, None) => {}
                        (_, Some(r)) if arriving.iter().any(|f| f.name == r) => {}
                        (n, None) if n >= 2 => {
                            return Err(SchemaError {
                                code: "ambiguous_reverse_step",
                                message: format!(
                                    "`{step_entity}` refs `{cur}` more than once — name the arriving ref (`{step_entity}.<ref>`)"
                                ),
                                span: *span,
                            })
                        }
                        (_, Some(r)) => {
                            return Err(SchemaError {
                                code: "invalid_guard_walk",
                                message: format!("`{step_entity}.{r}` is not a ref to `{cur}`"),
                                span: *span,
                            })
                        }
                        _ => {}
                    }
                    validate_walk(entities, by_name, step_entity, inner)?;
                }
            }
        }
    }
    Ok(())
}

/// Forward hops ending at a field; sealed anywhere is guard_on_sealed (G3).
fn walk_forward(
    entities: &[Entity],
    by_name: &HashMap<String, usize>,
    home: &str,
    path: &[ordo_syntax::Ident],
    span: Span,
) -> Result<(), SchemaError> {
    let mut cur = home.to_string();
    for (i, seg) in path.iter().enumerate() {
        let e = &entities[by_name[&cur]];
        let Some(f) = e.field(&seg.name) else {
            return Err(SchemaError {
                code: "invalid_guard_walk",
                message: format!("`{cur}` has no field `{}`", seg.name),
                span,
            });
        };
        if f.sealed {
            return Err(SchemaError {
                code: "guard_on_sealed",
                message: format!(
                    "`{cur}.{}` is sealed — guards walk the plaintext skeleton only (G3)",
                    seg.name
                ),
                span,
            });
        }
        match &f.ty {
            FieldTy::Ref { target, .. } => cur = target.clone(),
            _ if i + 1 == path.len() => {}
            _ => {
                return Err(SchemaError {
                    code: "invalid_guard_walk",
                    message: format!("`{}` is not a ref and cannot be walked through", seg.name),
                    span,
                })
            }
        }
    }
    Ok(())
}

impl Guards {
    /// Render one verb's effective guard as source-shaped text, rebase
    /// applied — what `ordo check` prints and what a reviewer greps.
    pub fn render(&self, read: bool) -> String {
        let body = if read { &self.read } else { &self.write };
        match body {
            GuardBody::Open => "open".into(),
            GuardBody::Walk(w) => {
                let prefix = self.rebase.join(".");
                render_expr(w, &prefix, "caller")
            }
        }
    }
}

fn render_expr(w: &GuardExpr, prefix: &str, subject: &str) -> String {
    w.disjuncts
        .iter()
        .map(|conj| {
            conj.iter()
                .map(|a| render_atom(a, prefix, subject))
                .collect::<Vec<_>>()
                .join(" and ")
        })
        .collect::<Vec<_>>()
        .join(" or ")
}

fn render_atom(a: &GuardAtom, prefix: &str, subject: &str) -> String {
    let path = |p: &[ordo_syntax::Ident]| -> String {
        let segs: Vec<&str> = p.iter().map(|i| i.name.as_str()).collect();
        if prefix.is_empty() {
            segs.join(".")
        } else {
            format!("{prefix}.{}", segs.join("."))
        }
    };
    match a {
        GuardAtom::Cmp {
            path: p,
            caller: true,
            ..
        } => format!("{} == ${subject}", path(p)),
        GuardAtom::Cmp {
            path: p,
            lit: Some(e),
            ..
        } => format!("{} == {}", path(p), render_lit(e)),
        GuardAtom::Cmp { path: p, .. } => path(p),
        GuardAtom::Bare { path: p, .. } => path(p),
        GuardAtom::Step { path: p, inner, .. } => {
            format!("{}({})", path(p), render_expr(inner, "", subject))
        }
    }
}

/// One release condition as source text: `not invoices(owner == $member)`.
/// What a refusal names when the condition holds a member back, and what
/// `ordo check` prints. It names the rule as written, never a row.
pub fn render_release_atom(a: &ordo_syntax::ReleaseAtom) -> String {
    format!(
        "{}{}({})",
        if a.negated { "not " } else { "" },
        a.entity.name,
        render_expr(&a.inner, "", "member")
    )
}

/// A whole `release member` rule as source text, one effect per line.
pub fn render_release(r: &ordo_syntax::ReleaseDecl) -> String {
    use ordo_syntax::{ReassignTarget, ReleaseEffect, ReleaseMode};
    let mut out = String::from("release member ");
    match &r.mode {
        ReleaseMode::Open { .. } => out.push_str("open"),
        ReleaseMode::Locked { .. } => out.push_str("locked"),
        ReleaseMode::When(c) => {
            out.push_str("when ");
            let parts: Vec<String> = c
                .disjuncts
                .iter()
                .map(|conj| {
                    conj.iter()
                        .map(render_release_atom)
                        .collect::<Vec<_>>()
                        .join(" and ")
                })
                .collect();
            if parts.len() == 1 {
                out.push_str(&parts[0]);
            } else {
                out.push_str(
                    &parts
                        .iter()
                        .map(|p| format!("({p})"))
                        .collect::<Vec<_>>()
                        .join(" or "),
                );
            }
        }
    }
    if !r.effects.is_empty() {
        out.push_str(" {");
        for e in &r.effects {
            out.push_str("\n  ");
            match e {
                ReleaseEffect::Remove { entity, field, .. } => {
                    out.push_str(&format!("remove {}.{}", entity.name, field.name))
                }
                ReleaseEffect::Reassign {
                    entity, field, to, ..
                } => {
                    let targets: Vec<String> = to
                        .iter()
                        .map(|t| match t {
                            ReassignTarget::Owner { .. } => "owner".to_string(),
                            ReassignTarget::Lookup {
                                entity,
                                inner,
                                field,
                                ..
                            } => format!(
                                "{}({}).{}",
                                entity.name,
                                render_expr(inner, "", "member"),
                                field.name
                            ),
                        })
                        .collect();
                    out.push_str(&format!(
                        "reassign {}.{} to {}",
                        entity.name,
                        field.name,
                        targets.join(" or ")
                    ))
                }
            }
        }
        out.push_str("\n}");
    }
    out
}

/// The right-hand side of a guard comparison, as source-shaped text.
///
/// **Every literal form the guard grammar admits renders distinguishably**,
/// and that is now load-bearing rather than cosmetic: [`Guards::render`] is
/// the *identity of the walk* a capability grant pins (`definition-language.md`
/// §8, row-scope shape), so two different guards that render alike are two
/// different row bounds one pin cannot tell apart. Before this, `…` stood for
/// four of the eight literal forms — a guard could be re-pointed from
/// `2020-01-01` to `2030-01-01` and every pin over it would read "same walk".
///
/// The remaining `…` is for a non-literal right-hand side, which the parser
/// accepts as an expression and `check_guard_lit_ty` then refuses, so no
/// schema reaching this function carries one.
fn render_lit(e: &Expr) -> String {
    match e {
        Expr::StrLit { value, .. } => format!("\"{value}\""),
        Expr::IntLit { value, .. } => value.to_string(),
        Expr::BoolLit { value, .. } => value.to_string(),
        Expr::DecimalLit { digits, .. } => digits.clone(),
        Expr::DateLit { y, m, d, .. } => format!("{y:04}-{m:02}-{d:02}"),
        Expr::DatetimeLit {
            y,
            m,
            d,
            hh,
            mm,
            ss,
            ..
        } => format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"),
        Expr::DurationLit { n, unit, .. } => format!("{n}{}", unit.as_str()),
        Expr::NullLit { .. } => "null".into(),
        Expr::Param { name, .. } => format!("${name}"),
        _ => "…".into(),
    }
}

fn clause_body(c: &GuardClause) -> GuardBody {
    match c {
        GuardClause::Walk(w) => GuardBody::Walk(w.clone()),
        GuardClause::Open { .. } => GuardBody::Open,
    }
}

/// A numeric literal written in major units, as a `money(p)` or
/// `decimal(p)` column stores it (27ak).
///
/// - `money(p)`: whole minor units, `10.50` at places 2 is `1050`.
/// - `decimal(p)`: the literal as written, once it is known to carry no
///   more than `p` significant places.
///
/// `Ok(None)` when the type is neither or the expression is not a numeric
/// literal (a negative one included). `Err` says why the literal does not
/// fit: storing it would round, or it overflows a `bigint`.
pub fn stored_numeric(ty: ScalarTy, e: &Expr) -> Result<Option<Expr>, String> {
    use ordo_syntax::numeric::{literal_minor_units, significant_places, Fit};
    let (neg, inner) = match e {
        Expr::Neg { expr, .. } => (true, expr.as_ref()),
        other => (false, other),
    };
    let digits = match inner {
        Expr::IntLit { value, .. } => value.to_string(),
        Expr::DecimalLit { digits, .. } => digits.clone(),
        _ => return Ok(None),
    };
    let digits = if neg { format!("-{digits}") } else { digits };
    let span = e.span();
    match ty {
        ScalarTy::Money(p) => {
            let lit = Expr::DecimalLit {
                digits: digits.clone(),
                span,
            };
            match literal_minor_units(&lit, p) {
                Ok(m) => Ok(Some(Expr::IntLit {
                    value: i128::from(m),
                    span,
                })),
                Err(Fit::TooFine { written }) => Err(format!(
                    "`{digits}` has {written} places and `money({p})` holds {p}; storing it \
                     would round, and nothing rounds implicitly"
                )),
                Err(Fit::Overflow) => Err(format!(
                    "`{digits}` is too large for `money({p})`, which holds whole minor units \
                     in a 64-bit integer"
                )),
                Err(Fit::NotNumeric) => Ok(None),
            }
        }
        ScalarTy::Decimal(p) => match significant_places(&digits) {
            Some(s) if s <= u32::from(p) => Ok(Some(Expr::DecimalLit { digits, span })),
            Some(s) => Err(format!(
                "`{digits}` has {s} places and `decimal({p})` holds {p}; storing it would \
                 round, and nothing rounds implicitly"
            )),
            None => Ok(None),
        },
        _ => Ok(None),
    }
}
