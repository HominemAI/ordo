//! Token inventory. Closed, like everything else: if it isn't here, it
//! doesn't lex, and the error says so.

use crate::span::Span;

/// One word list, four readers.
///
/// A closed vocabulary used to be written three times here — the enum, the
/// `parse` match, the `as_str` match — and anything outside the crate that
/// needed to *enumerate* it (the editor grammar's parity test, most recently)
/// had to write it a fourth. Three of those could drift silently and the
/// fourth could drift loudly, which is the same defect the rest of the
/// compiler refuses: a second walk instead of a projection. So the table is
/// declared once and the enum, the parser, the printer and `ALL` are all
/// projections of it. Adding a word is one line, and no reader can miss it.
///
/// `ALL` is therefore complete **by construction**, not by memory — which is
/// what makes a two-directional inventory test against it worth writing.
macro_rules! word_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $( $variant:ident => $word:literal ),* $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $( $variant ),* }

        impl $name {
            /// Every word in the vocabulary, in declaration order. Iterating
            /// this is the supported way to enumerate the set; a second list
            /// would be a second definition that could drift from the first.
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),* ];

            pub fn parse(s: &str) -> Option<$name> {
                Some(match s {
                    $( $word => $name::$variant, )*
                    _ => return None,
                })
            }

            pub fn as_str(self) -> &'static str {
                match self {
                    $( $name::$variant => $word ),*
                }
            }
        }
    };
}

word_enum! {
    /// Duration units, spelled exactly as the spec's closed set (§2):
    /// `ms s m h d w mo y` — `m` is minutes, `mo` is months.
    pub enum DurationUnit {
        Ms => "ms",
        S => "s",
        M => "m",
        H => "h",
        D => "d",
        W => "w",
        Mo => "mo",
        Y => "y",
    }
}

word_enum! {
    /// Reserved words (engine-plan G3), fully-reserved tier only: a word belongs
    /// here iff it can *begin a declaration or a clause*. The contextual tier —
    /// `read write admin` (after `access`, inside `acl (…)`), `run method methods
    /// share rollup live table metric detail route` (positional inside `routine`,
    /// `auth`, `policy`, `app`/`page` blocks), and every aggregate/function name
    /// (`count sum avg min max …`) — is deliberately NOT here: those stay `Ident`
    /// so they remain usable as field names, and position disambiguates them.
    pub enum Keyword {
        // logic and pipeline
        And => "and",
        Or => "or",
        Not => "not",
        In => "in",
        Is => "is",
        Null => "null",
        True => "true",
        False => "false",
        Match => "match",
        Any => "any",
        None => "none",
        On => "on",
        Group => "group",
        Reduce => "reduce",
        Sort => "sort",
        Take => "take",
        Append => "append",
        Let => "let",
        Distinct => "distinct",
        Under => "under",
        Above => "above",
        Since => "since",
        Contains => "contains",
        Starts => "starts",
        Matches => "matches",
        Set => "set",
        Del => "del",
        Subtree => "subtree",
        Each => "each",
        // declarations, clauses and field modifiers
        Op => "op",
        Entity => "entity",
        Routine => "routine",
        Domain => "domain",
        When => "when",
        Every => "every",
        Trigger => "trigger",
        Check => "check",
        Index => "index",
        Unique => "unique",
        Sealed => "sealed",
        Indexed => "indexed",
        Owned => "owned",
        Tree => "tree",
        Normalize => "normalize",
        Whole => "whole",
        // Authorization and platform blocks — each one can begin a clause, so each
        // one is fully reserved (G3). Their absence is what made `read: int` look
        // legal and left authorization outside the parser entirely.
        Acl => "acl",
        Access => "access",
        App => "app",
        Page => "page",
        Principal => "principal",
        Client => "client",
        Auth => "auth",
        Policy => "policy",
        Compliance => "compliance",
        Channel => "channel",
        Space => "space",
        // Registration flags (G9's wrapper): reserved, or an op named `unbounded`
        // parses as a flagless op with a stray word after it.
        Unguarded => "unguarded",
        Unbounded => "unbounded",
        Reproducible => "reproducible",
        // Keeps the op answering while its tenant is paused (27ar, owner
        // 2026-10-05: `op billing nopause = ...`).
        Nopause => "nopause",
        // `public` (ruled 2026-10-03, `syntax-additions` ask 2): a field
        // modifier, an entity line (`public`, `public*`) and an op flag.
        // Reserved because it heads an entity clause (the G3 rule).
        Public => "public",
        Filled => "filled",
        Cond => "cond",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    // ── literals ──
    /// Integer literal (exact; overflow of i128 at lex time is an error).
    Int(i128),
    /// Decimal literal, kept as its exact source digits (`10.50`) — the
    /// engine's arithmetic is exact decimal, so no float ever exists.
    Decimal(String),
    /// String literal, escapes already resolved.
    Str(String),
    /// Bare ISO date `2026-07-01`.
    Date {
        y: i32,
        m: u8,
        d: u8,
    },
    /// Bare ISO datetime `2026-07-01T14:30:00Z` (UTC only; the Z is mandatory).
    Datetime {
        y: i32,
        m: u8,
        d: u8,
        hh: u8,
        mm: u8,
        ss: u8,
    },
    /// A schedule time `09:00Z` (DL §7): hour and minute, UTC, the `Z`
    /// mandatory. Only a `trigger every day` clause takes one.
    UtcTime {
        hour: u8,
        minute: u8,
    },
    /// Duration literal `90d`, `5m`, `1mo`, `500ms`.
    Duration {
        n: u64,
        unit: DurationUnit,
    },

    // ── names ──
    Ident(String),
    /// `$name` parameter reference.
    Param(String),
    Keyword(Keyword),

    // ── punctuation & operators ──
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,
    Bar,
    /// `->` — a ref in shapes, "yields" in `cond` arms and `each ->`;
    /// position decides, the token is one.
    Arrow,
    Eq,    // =
    EqEq,  // ==
    NotEq, // !=
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Question,
    At,

    /// Statement separator: end-of-line at delimiter depth zero (G6).
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
