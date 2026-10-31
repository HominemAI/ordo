//! Recursive-descent parser for declarations and pipelines — the half of P1
//! that `parser.rs` left to "the future pipeline/entity parsers".
//!
//! Two structural facts drive every awkward-looking decision here:
//!
//! 1. **The lexer emits `Newline` only at delimiter depth zero** (G6). Inside
//!    an entity's `{ … }` there are no separators at all, so field boundaries
//!    are found by shape (`ident :` / `ident ->`) rather than by a token.
//! 2. **A step ends at end of line.** So an op body is: one source-list line,
//!    then zero or more step lines, ending at the first line that opens a new
//!    declaration. Since `op`/`entity`/`domain` are reserved (G3) and no step
//!    may begin with them, that boundary needs no indentation tracking.

use crate::ast::{Condition, Expr, Ident};
use crate::decl::*;
use crate::lexer::lex;
use crate::parser::{self, describe, ParseError, Parser};
use crate::span::Span;
use crate::token::{DurationUnit, Keyword, TokenKind};

/// Parse a whole definition file.
pub fn parse_program(src: &str) -> Result<Program, ParseError> {
    parse_program_noting(src).map(|(prog, _)| prog)
}

/// Every field or parameter type written as bare `money`, in source order.
///
/// Bare `money` is `money(0)`, so a program written before places existed
/// keeps its meaning. New code should say its places, and `ordo check`
/// warns at each of these (ruled 2026-10-05, `money-types` ask 1). Empty
/// when the program does not parse: the parse error is the news then.
pub fn bare_money_sites(src: &str) -> Vec<crate::Span> {
    let mut sites = parse_program_noting(src)
        .map(|(_, sites)| sites)
        .unwrap_or_default();
    // The parser may read a stretch twice after a backtrack.
    sites.sort_by_key(|s| s.start);
    sites.dedup();
    sites
}

fn parse_program_noting(src: &str) -> Result<(Program, Vec<crate::Span>), ParseError> {
    let toks = lex(src)?;
    let mut p = Parser::new(&toks);
    let mut prog = Program::default();

    loop {
        skip_newlines(&mut p);
        match p.peek() {
            TokenKind::Eof => break,
            TokenKind::Keyword(Keyword::Domain) => {
                let d = domain_decl(&mut p)?;
                if prog.domain.is_some() {
                    return Err(p.err_at(d.span, "duplicate `domain` header"));
                }
                prog.domain = Some(d);
            }
            TokenKind::Keyword(Keyword::Entity) => prog.entities.push(entity_decl(&mut p)?),
            TokenKind::Keyword(Keyword::Op) => prog.ops.push(op_decl(&mut p)?),
            TokenKind::Keyword(Keyword::Routine) => prog.routines.push(routine_decl(&mut p, src)?),
            // §7 spells a trigger as a top-level line; §8 writes the same
            // fact as a routine's `trigger` clause. The clause is the one
            // that is built (checklist 12a), so the top-level line is
            // refused with the spelling that works.
            TokenKind::Keyword(k @ (Keyword::When | Keyword::Every | Keyword::Trigger)) => {
                return Err(p.err_here(format!(
                    "`{}` is written inside a routine, as its trigger: \
                     `routine name {{ trigger every 5m  run: set … }}` or \
                     `routine name {{ trigger when <domain>.<entity>.created  run: set … }}`",
                    k.as_str()
                )));
            }
            // `release` is contextual: it heads a declaration only here, at
            // the start of a line, so a field or an op named `release` keeps
            // working and no example has to change (27aq).
            // `secret` is contextual too: it heads a declaration only at the
            // start of a top-level line (`plan-backend-functions.md` §5).
            TokenKind::Ident(w) if w == "secret" => prog.secrets.push(secret_decl(&mut p, src)?),
            TokenKind::Ident(w) if w == "release" => {
                let r = release_decl(&mut p)?;
                if prog.release.is_some() {
                    return Err(p.err_class_at(
                        parser::DUPLICATE_RELEASE,
                        r.span,
                        "a program has one `release member` rule; this is a second",
                    ));
                }
                prog.release = Some(r);
            }
            TokenKind::Ident(w) => {
                let (w, span) = (w.clone(), p.tok().span);
                reject_foreign(&p, &w, span)?;
                // Keyword, not Ident, is where the reserved-and-unbuilt
                // declarations land — see the `other` arm. Checked here too
                // because the table is keyed by spelling, and a word that
                // stops being a keyword must not silently stop being refused.
                reject_unbuilt(&p, &w, span)?;
                return Err(p.err_here(format!(
                    "expected a declaration (`domain`, `entity`, `op`, `release`, `secret`), \
                     found `{w}`"
                )));
            }
            // **Every reserved-and-unbuilt declaration arrives here**, not in
            // the `Ident` arm above: `routine`, `app`, `policy` and the rest
            // are all in `Keyword`, so the lexer classifies them and only the
            // parser has nowhere to send them. `describe` renders a keyword
            // with backticks exactly as the `Ident` arm formats a word, which
            // is why the two answers were indistinguishable and why this
            // finding survived four rounds of review.
            TokenKind::Keyword(k) => {
                let (k, span) = (*k, p.tok().span);
                reject_unbuilt(&p, k.as_str(), span)?;
                return Err(p.err_here(format!(
                    "expected a declaration (`domain`, `entity`, `op`), found `{}`",
                    k.as_str()
                )));
            }
            other => {
                let d = describe(other);
                return Err(p.err_here(format!(
                    "expected a declaration (`domain`, `entity`, `op`), found {d}"
                )));
            }
        }
    }
    Ok((prog, p.bare_money))
}

/// Parse a standalone `whole` read: `whole orders("ord_1")` — the anchored
/// composition form (DL §`whole`), which is not a pipeline: its result is
/// one member plus its owned closure and signposts, not a row stream.
pub fn parse_whole(src: &str) -> Result<(Ident, Expr), ParseError> {
    let toks = lex(src)?;
    let mut p = Parser::new(&toks);
    skip_newlines(&mut p);
    if !p.eat_kw(Keyword::Whole) {
        return Err(p.err_here("expected `whole`"));
    }
    let entity = ident(&mut p, "an entity name")?;
    p.expect(&TokenKind::LParen, "`(` around the anchor id")?;
    let anchor = p.expr()?;
    p.expect(&TokenKind::RParen, "`)` closing the anchor")?;
    p.finish()?;
    Ok((entity, anchor))
}

/// Parse a bare pipeline — a corpus case body, with no `op` wrapper.
pub fn parse_pipeline(src: &str) -> Result<Pipeline, ParseError> {
    let toks = lex(src)?;
    let mut p = Parser::new(&toks);
    skip_newlines(&mut p);
    let pl = pipeline(&mut p)?;
    skip_newlines(&mut p);
    if let TokenKind::Ident(w) = p.peek().clone() {
        reject_foreign(&p, &w, p.tok().span)?;
    }
    if !matches!(p.peek(), TokenKind::Eof) {
        let d = describe(p.peek());
        return Err(p.err_here(format!("unexpected trailing {d}")));
    }
    Ok(pl)
}

/// Declarations the language **reserves and does not implement**.
///
/// The same argument as [`FOREIGN_SPELLINGS`] one tier up. That table exists
/// because a borrowed spelling lexed as an entity name and failed later with
/// an error naming the wrong problem. These fail *earlier* and name an even
/// wronger one: every word here lexes as a plain identifier, so
/// `parse_program` reaches its fallback and answers **"expected a declaration
/// (`domain`, `entity`, `op`)"**, which reads as a typo or a stale binary.
///
/// It is not a typo. `tutorial.md` §13 teaches `routine` across a full page,
/// `syntax-examples.md`, `modeling-guidance.md` and `traps.md` all use it, and
/// `definition-language.md` §8 specifies it. A reader who has followed twelve
/// sections correctly types exactly what the document showed them and is told
/// the word does not belong — so they go looking for the mistake they did not
/// make. Found by re-running the consolidated critique over the documentation
/// (`critique-consolidated-5.md` W-001, checklist 17af).
///
/// **Reserved and unbuilt is a third state**, and the refusal has to say which
/// of the three it is: a word the grammar does not know, a word it knows and
/// implements, and a word it knows and has deliberately not implemented
/// because the spelling is not settled (9C's lean pin; 12a for `routine`,
/// where §7 and §8 spell one fact two ways and building either would pick by
/// accident). The second element is that sentence.
const UNBUILT_DECLARATIONS: &[(&str, &str)] = &[
    (
        "principal",
        "the `principal` declaration is specified in `definition-language.md` §8 \
         under the lean pin and is unbuilt (checklist 19d)",
    ),
    (
        "client",
        "the `client` declaration (an OIDC relying party) is specified in \
         `definition-language.md` §8 under the lean pin and is unbuilt \
         (checklist 19e)",
    ),
    (
        "policy",
        "the `policy` compliance pack is specified in `definition-language.md` §8 \
         under the lean pin and is unbuilt (checklist 19f)",
    ),
    (
        "channel",
        "`channel` and `route` are specified in `definition-language.md` §8 under \
         the lean pin and their runtime is unbuilt (checklist 19g, 17r)",
    ),
    (
        "route",
        "`channel` and `route` are specified in `definition-language.md` §8 under \
         the lean pin and their runtime is unbuilt (checklist 19g, 17r)",
    ),
    (
        "app",
        "the `app` manifest is specified in `ui-gen.md` §5, whose binding grammar \
         is normative and whose declaration is not built here (checklist 19h)",
    ),
    (
        "page",
        "a `page` is declared inside an `app` manifest (`ui-gen.md` §5), which is \
         not built here (checklist 19h)",
    ),
];

/// Refuse a reserved-and-unbuilt declaration by name, saying which state it is
/// in rather than listing the three declarations that do parse.
fn reject_unbuilt(p: &Parser, name: &str, span: Span) -> Result<(), ParseError> {
    match UNBUILT_DECLARATIONS.iter().find(|(k, _)| *k == name) {
        Some((_, why)) => Err(p.err_at(
            span,
            format!("`{name}` is reserved and not implemented: {why}"),
        )),
        None => Ok(()),
    }
}

/// Spellings borrowed from SQL and from method-chain query builders.
///
/// None of these are Ordo keywords, so without this table they lex as plain
/// identifiers and **read as entity names** — `orders join customers` parsed
/// cleanly as a three-source list, and `select total from orders` as a
/// four-source one. Both are `composition-conformance.md` C100/C109, which
/// require a parse-time rejection, and both got one only much later at bind
/// time as `unknown_source` — an error naming the wrong problem.
///
/// "One spelling per concept" is only true if the other spellings fail, and
/// fail *saying so*. The second element is what to write instead.
const FOREIGN_SPELLINGS: &[(&str, &str)] = &[
    ("select", "name the source, then project: `orders[total]`"),
    ("from", "the source comes first: `orders[total]`"),
    (
        "where",
        "a filter is `orders(total > 5)` or a `| condition` step",
    ),
    ("join", "matches are inferred — list sources side by side"),
    ("inner", "matches are inferred; there is no join spelling"),
    (
        "outer",
        "matches are inferred; use `match any/none` to choose preservation",
    ),
    ("cross", "matches are inferred; there is no join spelling"),
    ("natural", "matches are inferred; there is no join spelling"),
    (
        "having",
        "filter a grouped pipeline with a `| condition` step",
    ),
    ("limit", "`take n`"),
    ("offset", "pagination is by cursor, not offset"),
    ("order", "`sort key` / `sort -key`"),
    ("by", "`sort key` / `group key`"),
    ("asc", "ascending is the default; `sort key`"),
    ("desc", "`sort -key`"),
    ("union", "`append`"),
    ("intersect", "there is no set-intersection spelling"),
    ("except", "there is no set-difference spelling"),
    ("insert", "`set entity { … }`"),
    ("into", "`set entity { … }`"),
    ("values", "`set entity { field: value }`"),
    ("update", "`set` — there is no raw UPDATE path"),
    ("as", "rename in the projection: `[label: name]`"),
    ("with", "`let`"),
    (
        "filter",
        "a filter is `orders(total > 5)` or a `| condition` step",
    ),
    ("map", "project instead: `[field, computed: expr]`"),
    ("pluck", "project instead: `[field]`"),
    ("top", "`take n`"),
    ("fetch", "`take n`"),
    ("like", "`contains` / `starts` / `matches`"),
    ("ilike", "`contains` / `starts` / `matches`"),
    ("between", "chain comparisons with `and`"),
    ("exists", "`is not null`, or a match over the collection"),
];

fn foreign_spelling(name: &str) -> Option<&'static str> {
    FOREIGN_SPELLINGS
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| *v)
}

/// Reject a borrowed spelling wherever a source or a step may begin.
fn reject_foreign(p: &Parser, name: &str, span: Span) -> Result<(), ParseError> {
    match foreign_spelling(name) {
        Some(instead) => Err(p.err_at(span, format!("`{name}` is not Ordo — {instead}"))),
        None => Ok(()),
    }
}

/// Extend a source name with its **qualifier segments** — `std.orders.get`,
/// `finance.spendingByPayee` — folding the dots into the single registered
/// name (`std-library.md` §7: *"Composition uses the dotted name verbatim …
/// the dots are part of the name, never a traversal"*).
///
/// # Why this is in `source` and nowhere else
///
/// A registered name is a **flat key** — `Registry` is a map from one string
/// to one `OpDecl` — so a qualified name is not a path to walk but a longer
/// spelling of one key. Joining here, at the only position where a *name* is
/// read, keeps that true: the binder still does one lookup, guard atoms and
/// projections keep their own dot loops (where a dot IS a traversal), and no
/// other construct gains a meaning for `.`.
///
/// The lookahead is what makes it unambiguous. A source is followed by `(`,
/// `[`, a step keyword, or a newline; a dot in that position parses today
/// only as an error, so consuming `. ident` here takes nothing away from any
/// program that currently parses. `.` followed by anything else is left for
/// the caller to fail on, with its own message.
///
/// Nothing is validated about the segments beyond their being identifiers:
/// whether `std.orders.get` names something is the binder's question, and it
/// already answers `unknown_source` for names that do not resolve. The
/// reserved half of the namespace rule (`std-library.md` §6) needs no check
/// here — an *authored* name is a single `Ident` from the lexer, which admits
/// no dot, so no `op` declaration can spell one. The namespace is reserved by
/// the grammar rather than by a guard, which is the cheapest form of reserved.
fn qualify(p: &mut Parser, name: &mut Ident) {
    while matches!(p.peek(), TokenKind::Dot) && matches!(p.peek_at(1), TokenKind::Ident(_)) {
        p.bump();
        let seg = p.bump();
        let TokenKind::Ident(seg_name) = seg.kind else {
            unreachable!("peeked an identifier")
        };
        name.name.push('.');
        name.name.push_str(&seg_name);
    }
}

fn skip_newlines(p: &mut Parser) {
    while matches!(p.peek(), TokenKind::Newline) {
        p.bump();
    }
}

fn ident(p: &mut Parser, what: &str) -> Result<Ident, ParseError> {
    match p.peek().clone() {
        TokenKind::Ident(name) => {
            let t = p.bump();
            Ok(Ident { name, span: t.span })
        }
        other => Err(p.err_here(format!("expected {what}, found {}", describe(&other)))),
    }
}

/// A path segment **after a dot**: an identifier, or a reserved word read
/// as the field name it is in that position.
///
/// After `.` nothing but a segment can follow, so the position
/// disambiguates — the rule DL §2 states for the contextual tier
/// ("the grammar always disambiguates by position"). An author cannot
/// declare a keyword-named field, so the only fields this reaches are the
/// platform's: `groupMembers.group` (DL §3 Groups, "`member.groupMembers
/// .group(member == $caller)`"), whose ref is named `group` because that
/// is what it is, and which a guard must name because both of
/// `groupMembers`' refs point at `members`. Refusing the word here made
/// DL's own settled spelling unwritable. Whether the segment names a real
/// field stays the binder's question, as for any identifier.
fn segment(p: &mut Parser, what: &str) -> Result<Ident, ParseError> {
    match p.peek().clone() {
        TokenKind::Ident(name) => {
            let t = p.bump();
            Ok(Ident { name, span: t.span })
        }
        TokenKind::Keyword(k) => {
            let t = p.bump();
            Ok(Ident {
                name: k.as_str().to_string(),
                span: t.span,
            })
        }
        other => Err(p.err_here(format!("expected {what}, found {}", describe(&other)))),
    }
}

// ── routines ─────────────────────────────────────────────────────────────

/// `routine name [in domain] { trigger <clause>  run[(params)] [flags]: <body> }`
/// (DL §8's block, with §7's operands; checklist 12a). The body is write
/// statements, or `<pipeline> each -> <writes>`.
///
/// Inside the braces the lexer emits no line breaks, so every part is found
/// by shape: the trigger clause runs from `trigger` to the word `run`, and
/// the body is write statements up to the closing brace. The clause is read
/// back from the source text and handed to `ordo_syntax::trigger`'s operand
/// grammars, so a schedule or a listener pattern means exactly what it
/// meant before the declaration existed.
fn routine_decl(p: &mut Parser, src: &str) -> Result<RoutineDecl, ParseError> {
    let start = p.bump().span; // `routine`
    let name = ident(p, "a routine name")?;
    let domain = if p.eat_kw(Keyword::In) {
        Some(ident(p, "the domain the routine runs in")?)
    } else {
        None
    };
    p.expect(&TokenKind::LBrace, "`{` opening the routine")?;
    if !p.eat_kw(Keyword::Trigger) {
        return Err(p.err_here(
            "a routine begins with its trigger: `trigger every 5m`, \
             `trigger every day 09:00Z` or `trigger when <domain>.<entity>.<kind>`",
        ));
    }
    let (trigger, trigger_span) = trigger_clause(p, src)?;
    let notify = notify_clause(p)?;

    match p.peek() {
        TokenKind::Ident(w) if w == "run" => {
            p.bump();
        }
        other => {
            let d = describe(other);
            return Err(p.err_here(format!(
                "expected `run` after the trigger clause, found {d}"
            )));
        }
    }
    let mut params = Vec::new();
    if matches!(p.peek(), TokenKind::LParen) {
        p.bump();
        if !matches!(p.peek(), TokenKind::RParen) {
            loop {
                params.push(param_decl(p)?);
                if matches!(p.peek(), TokenKind::Comma) {
                    p.bump();
                    continue;
                }
                break;
            }
        }
        p.expect(&TokenKind::RParen, "`)` to close the parameter list")?;
    }
    let mut flags = OpFlags::default();
    loop {
        match p.peek() {
            TokenKind::Keyword(Keyword::Unbounded) => flags.unbounded = true,
            TokenKind::Keyword(Keyword::Unguarded) => flags.unguarded = true,
            TokenKind::Keyword(Keyword::Reproducible) => flags.reproducible = true,
            TokenKind::Keyword(Keyword::Nopause) => flags.nopause = true,
            _ => break,
        }
        p.bump();
    }
    p.expect(&TokenKind::Colon, "`:` after the `run` header")?;
    // A body that does not open with a write reads first: a pipeline, then
    // `each ->` and the writes to run for each row it produces.
    let each = if starts_write_stmt(p) || matches!(p.peek(), TokenKind::RBrace) {
        None
    } else {
        let pl = pipeline(p)?;
        if !p.eat_kw(Keyword::Each) {
            return Err(p.err_here(format!(
                "a routine's body is write statements (`set …`, `del …`), or a read \
                 pipeline followed by `each ->` and the writes to run for each row. \
                 Found {} after the pipeline",
                describe(p.peek())
            )));
        }
        p.expect(&TokenKind::Arrow, "`->` after `each`")?;
        Some(pl)
    };
    let mut body = Vec::new();
    while starts_write_stmt(p) {
        body.push(write_stmt(p)?);
    }
    if body.is_empty() {
        return Err(p.err_here(match each {
            None => format!(
                "a routine's body is write statements (`set …`, `del …`), or a read \
                 pipeline followed by `each ->` and its writes. Found {}",
                describe(p.peek())
            ),
            Some(_) => format!(
                "`each ->` is followed by the writes to run for each row (`set …`, \
                 `del …`), found {}",
                describe(p.peek())
            ),
        }));
    }
    let end = p.expect(&TokenKind::RBrace, "`}` closing the routine")?;
    Ok(RoutineDecl {
        name,
        domain,
        trigger,
        trigger_span,
        params,
        flags,
        each,
        body,
        notify,
        span: start.to(end.span),
    })
}

/// `secret <name> for <host>[, <host>]...`, one line.
///
/// A host is read back from the source text between the separators, so a
/// name like `api-v2.example.com` means what it says whatever the lexer
/// made of its pieces. A host the lexer cannot read bare, such as one whose
/// label starts with a digit, is written in quotes: `"1password.example"`.
fn secret_decl(p: &mut Parser, src: &str) -> Result<SecretDecl, ParseError> {
    let start = p.bump().span; // `secret`
    let name = ident(p, "a secret name")?;
    match p.peek() {
        TokenKind::Ident(w) if w == "for" => {
            p.bump();
        }
        other => {
            return Err(p.err_here(format!(
                "expected `for` and the hosts the secret may be sent to after `secret {}`, \
                 found {}: `secret {} for api.example.com`",
                name.name,
                describe(other),
                name.name
            )))
        }
    }
    let mut hosts = Vec::new();
    let end = loop {
        let first = p.tok().span;
        let mut last = None;
        while !matches!(
            p.peek(),
            TokenKind::Comma | TokenKind::Newline | TokenKind::Eof
        ) {
            last = Some(p.bump().span);
        }
        let Some(last) = last else {
            return Err(p.err_here(format!(
                "expected a host name for secret `{}`, such as `api.example.com`",
                name.name
            )));
        };
        let span = first.to(last);
        let raw = src.get(span.start..span.end).unwrap_or_default();
        let text = raw
            .strip_prefix('"')
            .and_then(|t| t.strip_suffix('"'))
            .unwrap_or(raw);
        let host = secret_host(text).map_err(|why| {
            p.err_class_at(
                parser::INVALID_SECRET_HOST,
                span,
                format!("secret `{}`: `{text}` {why}", name.name),
            )
        })?;
        if !hosts.contains(&host) {
            hosts.push(host);
        }
        if matches!(p.peek(), TokenKind::Comma) {
            p.bump();
            continue;
        }
        break last;
    };
    Ok(SecretDecl {
        name,
        hosts,
        span: start.to(end),
    })
}

/// One host a secret is bound to: an exact DNS name, lowercased.
pub(crate) fn secret_host(text: &str) -> Result<String, &'static str> {
    if text.contains("://") || text.contains('/') {
        return Err("is a URL; a secret is bound to a host name such as `api.example.com`");
    }
    if text.contains('*') {
        return Err("is a wildcard; a secret is bound to exact host names only");
    }
    if text.contains(':') {
        return Err("names a port; a secret is bound to a host name, on any port");
    }
    let host = text.to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if host.len() > 253 || labels.len() < 2 || !labels.iter().all(label_ok) {
        return Err("is not a host name such as `api.example.com`");
    }
    Ok(host)
}

/// `notify <level> "<text>"`, between the trigger and `run`. Absent is
/// `None`.
fn notify_clause(p: &mut Parser) -> Result<Option<crate::decl::RoutineNotify>, ParseError> {
    let start = match p.peek() {
        TokenKind::Ident(w) if w == "notify" => p.bump().span,
        _ => return Ok(None),
    };
    let level = ident(
        p,
        "the acl level whose holders are told, as in `notify admin \"A reminder is due\"`",
    )?;
    let (text, end) = match p.peek().clone() {
        TokenKind::Str(t) => (t, p.bump().span),
        other => {
            return Err(p.err_here(format!(
                "`notify {}` takes the message as a quoted text, as in `notify {} \"A \
                 reminder is due\"`; found {}",
                level.name,
                level.name,
                describe(&other)
            )))
        }
    };
    Ok(Some(crate::decl::RoutineNotify {
        level,
        text,
        span: start.to(end),
    }))
}

/// The tokens from after `trigger` up to the word `run` (or `notify`), read
/// as the text they were written as.
fn trigger_clause(p: &mut Parser, src: &str) -> Result<(RoutineTrigger, Span), ParseError> {
    let first = p.tok().span;
    let mut last = first;
    let mut n = 0;
    loop {
        match p.peek() {
            TokenKind::Ident(w) if w == "run" || w == "notify" => break,
            TokenKind::Eof | TokenKind::RBrace | TokenKind::LBrace => break,
            _ => {
                last = p.bump().span;
                n += 1;
            }
        }
    }
    if n == 0 {
        return Err(p.err_at(first, "`trigger` takes `every …` or `when …`"));
    }
    let span = first.to(last);
    let text = src.get(span.start..span.end).unwrap_or_default();
    // The operand grammars report from a standalone string, so their errors
    // carry no place. They are re-spanned to the clause the author wrote.
    let at = |e: ParseError| ParseError { span, ..e };
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.first().copied() {
        Some("every") => {
            let schedule = crate::trigger::Schedule::parse(text).map_err(at)?;
            Ok((RoutineTrigger::Clock(schedule), span))
        }
        Some("when") => {
            let rest = words[1..].to_vec();
            let cut = rest
                .iter()
                .position(|w| *w == "debounce" || *w == "coalesce")
                .unwrap_or(rest.len());
            // `notes . tasks . created` is one pattern however it was spaced.
            let pattern_text: String = rest[..cut].concat();
            if pattern_text.is_empty() {
                return Err(p.err_at(
                    span,
                    "`trigger when` takes a pattern: `<domain>.<entity>.<kind>`",
                ));
            }
            let pattern = crate::trigger::ListenerPattern::parse(&pattern_text).map_err(at)?;
            let modifiers = crate::trigger::Modifiers::parse(&rest[cut..].join(" ")).map_err(at)?;
            Ok((RoutineTrigger::Change { pattern, modifiers }, span))
        }
        _ => Err(p.err_at(
            span,
            format!("`trigger {text}`: a trigger is `every …` or `when …`"),
        )),
    }
}

// ── domain ───────────────────────────────────────────────────────────────

fn domain_decl(p: &mut Parser) -> Result<DomainDecl, ParseError> {
    let start = p.bump().span; // `domain`
    let name = ident(p, "a domain name")?;
    let mut version = None;
    if matches!(p.peek(), TokenKind::At) {
        p.bump();
        version = Some(version_text(p)?);
    }
    let span = start.to(name.span);
    Ok(DomainDecl {
        name,
        version,
        span,
    })
}

/// `1.0.0` reaches the lexer as a decimal followed by `.0`, or as an int when
/// two-part. Reassemble the source spelling rather than inventing a numeric
/// type for something that is not a number.
fn version_text(p: &mut Parser) -> Result<String, ParseError> {
    let mut out = String::new();
    loop {
        match p.peek().clone() {
            TokenKind::Int(n) => {
                p.bump();
                out.push_str(&n.to_string());
            }
            TokenKind::Decimal(d) => {
                p.bump();
                out.push_str(&d);
            }
            TokenKind::Dot => {
                p.bump();
                out.push('.');
            }
            _ => break,
        }
        if !matches!(
            p.peek(),
            TokenKind::Dot | TokenKind::Int(_) | TokenKind::Decimal(_)
        ) {
            break;
        }
    }
    if out.is_empty() {
        return Err(p.err_here("expected a version after `@`"));
    }
    Ok(out)
}

// ── entities ─────────────────────────────────────────────────────────────

fn entity_decl(p: &mut Parser) -> Result<EntityDecl, ParseError> {
    let start = p.bump().span; // `entity`
    let name = ident(p, "an entity name")?;
    // Optional typed-id key override: `(key: proj)`.
    let mut key = None;
    if matches!(p.peek(), TokenKind::LParen) {
        p.bump();
        let kw = ident(p, "`key`")?;
        if kw.name != "key" {
            return Err(p.err_at(
                kw.span,
                "an entity's parenthesis takes only `key: <prefix>`",
            ));
        }
        p.expect(&TokenKind::Colon, "`:` after `key`")?;
        key = Some(ident(p, "the id prefix")?);
        p.expect(&TokenKind::RParen, "`)` closing the key")?;
    }
    p.expect(&TokenKind::LBrace, "`{` to open the shape")?;

    let mut fields = Vec::new();
    let mut access_read = None;
    let mut access_write = None;
    let mut acl = None;
    let mut unique_keys: Vec<Vec<Ident>> = Vec::new();
    let mut indexes: Vec<Vec<Ident>> = Vec::new();
    let mut checks: Vec<CheckDecl> = Vec::new();
    let mut std_sealed = false;
    let mut public: Option<PublicMark> = None;
    loop {
        skip_newlines(p);
        if matches!(p.peek(), TokenKind::RBrace) {
            break;
        }
        if matches!(p.peek(), TokenKind::Eof) {
            return Err(p.err_here("unterminated shape: expected `}`"));
        }
        if p.eat_kw(Keyword::Access) {
            let verb = ident(p, "`read` or `write`")?;
            let clause = guard_clause(p)?;
            match verb.name.as_str() {
                "read" => {
                    if access_read.replace(clause).is_some() {
                        return Err(p.err_at(verb.span, "duplicate `access read`"));
                    }
                }
                "write" => {
                    if access_write.replace(clause).is_some() {
                        return Err(p.err_at(verb.span, "duplicate `access write`"));
                    }
                }
                other => {
                    // GC079: the verb set is closed.
                    return Err(p.err_at(
                        verb.span,
                        format!(
                            "`access {other}` is not a clause — the verbs are `read` and `write`"
                        ),
                    ));
                }
            }
            continue;
        }
        if p.eat_kw(Keyword::Acl) {
            p.expect(&TokenKind::LParen, "`(` opening the acl levels")?;
            let mut levels = vec![ident(p, "a level name")?];
            while matches!(p.peek(), TokenKind::Comma) {
                p.bump();
                levels.push(ident(p, "a level name")?);
            }
            p.expect(&TokenKind::RParen, "`)` closing the acl levels")?;
            if acl.replace(levels).is_some() {
                return Err(p.err_here("duplicate `acl`"));
            }
            continue;
        }
        // `unique (a, b)` and `index (a, b)` — entity-level column lists.
        //
        // `unique` is ALSO a field modifier, so the two spellings share a
        // keyword and the parenthesis is what tells them apart: a field's
        // `unique` is never followed by `(`. One token of lookahead decides
        // it, with no line-awareness needed — which matters because the
        // lexer emits no `Newline` inside a brace (G6), so the modifier
        // loop in `field_decl` would otherwise swallow the `unique` that
        // opens the NEXT line and then choke on its `(`.
        if matches!(p.peek(), TokenKind::Keyword(Keyword::Unique))
            && matches!(p.peek_at(1), TokenKind::LParen)
        {
            let kw = p.bump().span;
            unique_keys.push(column_list(p, "unique", kw)?);
            continue;
        }
        if matches!(p.peek(), TokenKind::Keyword(Keyword::Index)) {
            let kw = p.bump().span;
            indexes.push(column_list(p, "index", kw)?);
            continue;
        }
        // `check <condition>` — DL §3, "Row invariants: `check` [settled]".
        //
        // **No terminator, and none is needed.** The lexer emits no
        // `Newline` inside a brace (G6), so a clause cannot end at one; it
        // ends where the condition grammar stops consuming. That is exact
        // rather than lucky: a condition ends at a token that can neither
        // continue an expression nor open a chain, and the next line of an
        // entity body always begins with a field name (`ident :` /
        // `ident ->`) or a clause keyword — never with `and`, `or`, an
        // operator, or a literal. The same property is what lets
        // `field_decl` find its own boundaries.
        if matches!(p.peek(), TokenKind::Keyword(Keyword::Check)) {
            let kw = p.bump().span;
            let cond = p.condition()?;
            checks.push(CheckDecl {
                span: kw.to(cond.span()),
                cond,
            });
            continue;
        }
        // `std sealed` — DL §3, "Sealed fields and derived std ops
        // [settled]". `std` is contextual: a field named `std` is followed
        // by `:` or `->`, this line by the reserved word `sealed`, so one
        // token of lookahead tells them apart and no author loses a name.
        if matches!(p.peek(), TokenKind::Ident(w) if w == "std")
            && matches!(p.peek_at(1), TokenKind::Keyword(Keyword::Sealed))
        {
            let t = p.bump();
            p.bump();
            if std_sealed {
                return Err(p.err_at(t.span, "duplicate `std sealed`"));
            }
            std_sealed = true;
            continue;
        }
        // `public` / `public*` on a line of its own. A field's `public`
        // sits on the field's line and is taken by `field_decl`, so one that
        // reaches here opened a line.
        if matches!(p.peek(), TokenKind::Keyword(Keyword::Public)) {
            let t = p.bump();
            let cascade = matches!(p.peek(), TokenKind::Star) && !p.at_line_break();
            let span = if cascade {
                t.span.to(p.bump().span)
            } else {
                t.span
            };
            if public.is_some() {
                return Err(p.err_at(span, "duplicate `public` line"));
            }
            public = Some(PublicMark { cascade, span });
            continue;
        }
        fields.push(field_decl(p)?);
    }
    let end = p.bump().span; // `}`
    Ok(EntityDecl {
        name,
        key,
        fields,
        access_read,
        access_write,
        acl,
        unique_keys,
        indexes,
        checks,
        std_sealed,
        public,
        span: start.to(end),
    })
}

/// The parenthesised column list shared by `unique (a, b)` and
/// `index (a, b)`.
///
/// **Two or more columns, always.** The one-column case already has a
/// spelling — the field modifier `email: text unique`, and the automatic
/// index on every ref — and one construct per concept is the rule the whole
/// declaration grammar is built on (DL §1, principle 2). A `unique (a)`
/// that quietly meant the modifier would be a second way to say it that a
/// reader has to know is the same; a `unique (a)` that meant something
/// *else* would be worse. So it is refused at the spelling.
///
/// Whether the named columns exist, and whether they may be indexed at all,
/// is the schema's question and is answered there against the graph — the
/// parser owns the shape (`corpus-rulings.md`, "shape is the parser's").
fn column_list(p: &mut Parser, what: &str, kw: Span) -> Result<Vec<Ident>, ParseError> {
    p.expect(&TokenKind::LParen, "`(` opening the column list")?;
    let mut cols = vec![ident(p, "a field name")?];
    while matches!(p.peek(), TokenKind::Comma) {
        p.bump();
        cols.push(ident(p, "a field name")?);
    }
    let close = p.expect(&TokenKind::RParen, "`)` closing the column list")?;
    if cols.len() < 2 {
        return Err(p.err_at(
            kw.to(close.span),
            format!(
                "`{what} ({})` names one column — the single-column form is the field modifier \
                 `{}: … {what}`",
                cols[0].name, cols[0].name
            ),
        ));
    }
    let mut seen: Vec<&str> = Vec::new();
    for c in &cols {
        if seen.contains(&c.name.as_str()) {
            return Err(p.err_at(c.span, format!("`{what}` names `{}` twice", c.name)));
        }
        seen.push(&c.name);
    }
    Ok(cols)
}

/// One `access` clause body: the reserved-shaped `open`, or a guard walk.
fn guard_clause(p: &mut Parser) -> Result<GuardClause, ParseError> {
    if let TokenKind::Ident(w) = p.peek() {
        if w == "open" {
            let t = p.bump();
            return Ok(GuardClause::Open { span: t.span });
        }
    }
    // The withdrawn `access write true` spelling (GC100): a boolean literal
    // was never a legal walk, and saying so beats "expected a walk".
    if matches!(
        p.peek(),
        TokenKind::Keyword(Keyword::True) | TokenKind::Keyword(Keyword::False)
    ) {
        return Err(p.err_here(
            "a boolean literal is not a guard — open access is spelled `open`, guarded access is a `$caller` walk",
        ));
    }
    Ok(GuardClause::Walk(guard_expr(p)?))
}

/// A guard's normalized form after distribution: `or` of `and`s of atoms.
type Dnf = Vec<Vec<GuardAtom>>;

/// The most disjuncts a normalized guard may carry.
///
/// Distribution is a product: `(a or b) and (c or d) and (e or f)` is eight
/// independent authorization paths, and nesting `n` binary groups is `2^n`.
/// Real guards use nought to two groups; a schema that reaches sixty-four
/// paths has stopped being a rule a reviewer can read, and the parser must
/// not try to allocate its way there.
///
/// The cap is **engine-pinned**: R-G3 admits parens and says nothing about
/// how many paths a guard may have, so no inventory names a class for
/// "too many". It is `parse_error` by the same ruling `corpus-rulings.md`
/// took for `take count out of range` — a limit on what this compiler can
/// represent, not a statement about what the guard means.
const MAX_GUARD_DISJUNCTS: usize = 64;

/// Same-connective chains of primaries; parens to mix (G1, principle 2).
///
/// The result is always **disjunctive normal form**, and that is a ruling
/// rather than a convenience — see the note on [`ordo_syntax::GuardExpr`].
/// `and` is the cross product of its operands' disjunct sets; `or` is their
/// concatenation. Guards have no `not`, so distribution is sound: there is
/// no negation for De Morgan to flip, and every consumer's notion of "a
/// disjunct" (an independent authorization path) stays exactly what it was.
fn guard_expr(p: &mut Parser) -> Result<GuardExpr, ParseError> {
    let start = p.tok().span;
    let first = guard_prim(p)?;
    let conn = match p.peek() {
        TokenKind::Keyword(k @ (Keyword::And | Keyword::Or)) => *k,
        _ => {
            return Ok(GuardExpr {
                disjuncts: first,
                span: start.to(p.tok().span),
            })
        }
    };
    let mut parts = vec![first];
    while p.eat_kw(conn) {
        parts.push(guard_prim(p)?);
    }
    // Principle 2 verbatim, and the same message the condition parser
    // gives: no precedence table, here or anywhere.
    if let TokenKind::Keyword(other @ (Keyword::And | Keyword::Or)) = p.peek() {
        let other = other.as_str();
        return Err(p.err_here(format!(
            "cannot mix `{conn}` and `{other}` without parentheses — parenthesize one \
             side, e.g. `(a {conn} b) {other} c` (no precedence table, spec §1)",
            conn = conn.as_str(),
        )));
    }
    let disjuncts = match conn {
        Keyword::Or => parts.concat(),
        _ => {
            let mut acc: Dnf = vec![Vec::new()];
            for part in parts {
                if acc.len().saturating_mul(part.len()) > MAX_GUARD_DISJUNCTS {
                    return Err(p.err_here(format!(
                        "this guard normalizes to more than {MAX_GUARD_DISJUNCTS} independent \
                         authorization paths — parenthesized groups multiply, and a rule that \
                         wide is not one a reviewer can check"
                    )));
                }
                let mut next: Dnf = Vec::with_capacity(acc.len() * part.len());
                for a in &acc {
                    for d in &part {
                        let mut conj = a.clone();
                        conj.extend(d.iter().cloned());
                        next.push(conj);
                    }
                }
                acc = next;
            }
            acc
        }
    };
    Ok(GuardExpr {
        disjuncts,
        span: start.to(p.tok().span),
    })
}

/// `'(' guard_expr ')'` or a single atom. A group is normalized by the
/// recursive call, so a primary is already DNF when it comes back.
fn guard_prim(p: &mut Parser) -> Result<Dnf, ParseError> {
    if matches!(p.peek(), TokenKind::LParen) {
        p.bump();
        let inner = guard_expr(p)?;
        p.expect(&TokenKind::RParen, "`)` closing the guard group")?;
        // R-G3 has no operator that takes a GROUP as an operand: `==` and
        // the bare-bool form are both atom shapes. Anything else after `)`
        // read fine and says what the guard grammar does not admit, so it
        // is the walk's class rather than the parser's (GC077's boundary).
        if let Some(op) = foreign_guard_op(p) {
            return Err(p.err_class_here(
                parser::INVALID_GUARD_WALK,
                format!(
                    "`{op}` is not in the guard grammar — a parenthesized group joins to the \
                     rest of the walk with `and` or `or`, and nothing else"
                ),
            ));
        }
        return Ok(inner.disjuncts);
    }
    Ok(vec![vec![guard_atom(p)?]])
}

/// `path` rendered the way the author wrote it, so a refusal can name the
/// walk it is about rather than a token offset.
fn render_walk(path: &[Ident]) -> String {
    path.iter()
        .map(|i| i.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

/// R-G3 admits exactly two atom shapes that carry an operator: `==` against
/// `$caller` or a literal, and a bare `bool` field. An atom followed by any
/// OTHER operator is perfectly readable — a path, then a comparison — it
/// just says something the closed guard grammar does not admit. That makes
/// it the walk's own class, `invalid_guard_walk` with the walk named, and
/// not `parse_error`: nothing failed to parse.
///
/// This is the boundary the guards corpus disagreed with the tree about
/// (GC077). The parser owns the SHAPE of a clause; the guard grammar owns
/// what a well-formed shape is allowed to MEAN, and R-G3's exclusions —
/// "operator other than `==`/bare-bool, function call" — are meaning.
fn foreign_guard_op(p: &Parser) -> Option<&'static str> {
    Some(match p.peek() {
        TokenKind::NotEq => "!=",
        TokenKind::Lt => "<",
        TokenKind::LtEq => "<=",
        TokenKind::Gt => ">",
        TokenKind::GtEq => ">=",
        TokenKind::Plus => "+",
        TokenKind::Minus => "-",
        TokenKind::Star => "*",
        TokenKind::Slash => "/",
        TokenKind::Eq => "=",
        _ => return None,
    })
}

fn reject_foreign_guard_op(p: &Parser, path: &[Ident]) -> Result<(), ParseError> {
    let Some(op) = foreign_guard_op(p) else {
        return Ok(());
    };
    Err(p.err_class_here(
        parser::INVALID_GUARD_WALK,
        format!(
            "`{op}` is not in the guard grammar — a guard atom is `{walk} == $caller`, \
             `{walk} == <literal>`, a bare `bool` field, or a collection step",
            walk = render_walk(path)
        ),
    ))
}

fn guard_atom(p: &mut Parser) -> Result<GuardAtom, ParseError> {
    debug_assert!(
        !matches!(p.peek(), TokenKind::LParen),
        "a leading `(` is a group and is handled by guard_prim"
    );
    // `open` is a keyword *inside an access clause*: it is the entire
    // clause or it is nothing (G9, R-G21, GC100). Read as an ordinary path
    // atom it became "`X` has no field `open`" from the schema builder —
    // an `invalid_guard_walk` about a field the author never meant, and a
    // message that would go on being right if somebody later declared a
    // `bool` field called `open`. Refusing it here keeps partial-open
    // unspellable by construction rather than by accident.
    if matches!(p.peek(), TokenKind::Ident(w) if w == "open") {
        return Err(p.err_here(
            "`open` is the whole access clause, never an operand inside a walk — \
             an entity is open on a verb or guarded on it, not partly both",
        ));
    }
    let start = p.tok().span;
    let mut path = vec![ident(p, "a guard walk")?];
    while matches!(p.peek(), TokenKind::Dot) {
        p.bump();
        path.push(segment(p, "a walk segment")?);
    }
    match p.peek() {
        // Reverse collection step: `…name(inner)`.
        TokenKind::LParen => {
            p.bump();
            let inner = guard_expr(p)?;
            let end = p.expect(&TokenKind::RParen, "`)` closing the step")?;
            // `length(title) > 3` gets here as a step over `length`: only
            // the schema knows `length` is not an entity, but ANY operator
            // after the step is already outside R-G3.
            reject_foreign_guard_op(p, &path)?;
            Ok(GuardAtom::Step {
                path,
                inner: Box::new(inner),
                span: start.to(end.span),
            })
        }
        TokenKind::EqEq => {
            p.bump();
            match p.peek().clone() {
                TokenKind::Param(name) if name == p.guard_subject => {
                    let t = p.bump();
                    Ok(GuardAtom::Cmp {
                        path,
                        caller: true,
                        lit: None,
                        span: start.to(t.span),
                    })
                }
                // Inside a release rule the subject is the seat being
                // released, whoever asks: `$caller` there would make the
                // rule mean something different for an owner and a member.
                TokenKind::Param(name) if name == "caller" && p.guard_subject == "member" => Err(p
                    .err_class_here(
                        parser::RELEASE_SUBJECT,
                        "a release rule is about the seat being released, `$member`; \
                         `$caller` has no meaning there",
                    )),
                // R-G3: "`==` tests against `$caller` or literals". A
                // different parameter is a well-formed comparison the
                // guard grammar does not admit — the walk's class, not
                // the parser's.
                TokenKind::Param(other) => Err(p.err_class_here(
                    parser::INVALID_GUARD_WALK,
                    format!(
                        "guards compare against `$caller` or literals, not `${other}` (`{}`)",
                        render_walk(&path)
                    ),
                )),
                _ => {
                    let e = p.expr()?;
                    let span = start.to(e.span());
                    Ok(GuardAtom::Cmp {
                        path,
                        caller: false,
                        lit: Some(e),
                        span,
                    })
                }
            }
        }
        // The closed grammar: anything else is a bare bool atom, and the
        // binder decides whether that is true (G1's walk resolution lands
        // there as invalid_guard_walk with the walk named). An operator
        // is decided here, under the same class.
        _ => {
            reject_foreign_guard_op(p, &path)?;
            Ok(GuardAtom::Bare {
                path,
                span: start.to(p.tok().span),
            })
        }
    }
}

// ── release ──────────────────────────────────────────────────────────────

/// `release member open | locked | when <rule> [{ effects }]` (27aq).
///
/// The block, when present, opens on the same line. Inside braces the lexer
/// emits no line breaks, so each effect is found by its leading word. A rule
/// longer than one line is written inside parentheses for the same reason.
fn release_decl(p: &mut Parser) -> Result<ReleaseDecl, ParseError> {
    let start = p.bump().span; // `release`
    match p.peek() {
        TokenKind::Ident(w) if w == "member" => {
            p.bump();
        }
        other => {
            return Err(p.err_here(format!(
                "expected `member` after `release`, found {}; a release rule is \
                 `release member open`, `release member locked` or `release member when …`",
                describe(other)
            )))
        }
    }
    let mode = match p.peek().clone() {
        TokenKind::Ident(w) if w == "open" => ReleaseMode::Open {
            span: p.bump().span,
        },
        TokenKind::Ident(w) if w == "locked" => ReleaseMode::Locked {
            span: p.bump().span,
        },
        TokenKind::Keyword(Keyword::When) => {
            p.bump();
            let prev = p.guard_subject;
            p.guard_subject = "member";
            let cond = release_cond(p);
            p.guard_subject = prev;
            ReleaseMode::When(cond?)
        }
        other => {
            return Err(p.err_here(format!(
                "expected `open`, `locked` or `when` after `release member`, found {}",
                describe(&other)
            )))
        }
    };
    let mut effects = Vec::new();
    let mut end = p.tok().span;
    if matches!(p.peek(), TokenKind::LBrace) {
        p.bump();
        let prev = p.guard_subject;
        p.guard_subject = "member";
        let body = release_effects(p);
        p.guard_subject = prev;
        effects = body?;
        end = p
            .expect(&TokenKind::RBrace, "`}` closing the release block")?
            .span;
    }
    if !matches!(p.peek(), TokenKind::Newline | TokenKind::Eof) {
        return Err(p.err_here(format!(
            "expected the end of the release rule, found {}",
            describe(p.peek())
        )));
    }
    Ok(ReleaseDecl {
        mode,
        effects,
        span: start.to(end),
    })
}

type ReleaseDnf = Vec<Vec<ReleaseAtom>>;

/// The guard algebra over release atoms: same-connective chains, parens to
/// mix, distributed into DNF.
fn release_cond(p: &mut Parser) -> Result<ReleaseCond, ParseError> {
    let start = p.tok().span;
    let first = release_prim(p)?;
    let conn = match p.peek() {
        TokenKind::Keyword(k @ (Keyword::And | Keyword::Or)) => *k,
        _ => {
            return Ok(ReleaseCond {
                disjuncts: first,
                span: start.to(p.tok().span),
            })
        }
    };
    let mut parts = vec![first];
    while p.eat_kw(conn) {
        parts.push(release_prim(p)?);
    }
    if let TokenKind::Keyword(other @ (Keyword::And | Keyword::Or)) = p.peek() {
        let other = other.as_str();
        return Err(p.err_here(format!(
            "cannot mix `{conn}` and `{other}` without parentheses — parenthesize one \
             side, e.g. `(a {conn} b) {other} c` (no precedence table, spec §1)",
            conn = conn.as_str(),
        )));
    }
    let disjuncts = match conn {
        Keyword::Or => parts.concat(),
        _ => {
            let mut acc: ReleaseDnf = vec![Vec::new()];
            for part in parts {
                if acc.len().saturating_mul(part.len()) > MAX_GUARD_DISJUNCTS {
                    return Err(p.err_here(format!(
                        "this release rule normalizes to more than {MAX_GUARD_DISJUNCTS} \
                         alternatives; a rule that wide is not one a reviewer can check"
                    )));
                }
                let mut next: ReleaseDnf = Vec::with_capacity(acc.len() * part.len());
                for a in &acc {
                    for d in &part {
                        let mut conj = a.clone();
                        conj.extend(d.iter().cloned());
                        next.push(conj);
                    }
                }
                acc = next;
            }
            acc
        }
    };
    Ok(ReleaseCond {
        disjuncts,
        span: start.to(p.tok().span),
    })
}

fn release_prim(p: &mut Parser) -> Result<ReleaseDnf, ParseError> {
    if matches!(p.peek(), TokenKind::LParen) {
        p.bump();
        let inner = release_cond(p)?;
        p.expect(&TokenKind::RParen, "`)` closing the group")?;
        return Ok(inner.disjuncts);
    }
    let start = p.tok().span;
    let negated = p.eat_kw(Keyword::Not);
    if negated && matches!(p.peek(), TokenKind::LParen) {
        return Err(p.err_here(
            "`not` applies to one condition: write `not a and not b` rather than \
             `not (a or b)`",
        ));
    }
    let entity = ident(p, "an entity in the release rule")?;
    if !matches!(p.peek(), TokenKind::LParen) {
        return Err(p.err_here(format!(
            "a release condition is an entity with a filter, `{name}(… == $member)`",
            name = entity.name
        )));
    }
    p.bump();
    let inner = guard_expr(p)?;
    let end = p.expect(&TokenKind::RParen, "`)` closing the condition")?;
    Ok(vec![vec![ReleaseAtom {
        negated,
        entity,
        inner,
        span: start.to(end.span),
    }]])
}

/// `remove e.f` and `reassign e.f to <target> [or <target>]…`, until `}`.
fn release_effects(p: &mut Parser) -> Result<Vec<ReleaseEffect>, ParseError> {
    let mut out = Vec::new();
    loop {
        match p.peek().clone() {
            TokenKind::RBrace => return Ok(out),
            TokenKind::Ident(w) if w == "remove" => {
                let start = p.bump().span;
                let (entity, field) = entity_field(p)?;
                let span = start.to(field.span);
                out.push(ReleaseEffect::Remove {
                    entity,
                    field,
                    span,
                });
            }
            TokenKind::Ident(w) if w == "reassign" => {
                let start = p.bump().span;
                let (entity, field) = entity_field(p)?;
                match p.peek() {
                    TokenKind::Ident(w) if w == "to" => {
                        p.bump();
                    }
                    other => {
                        return Err(p.err_here(format!(
                            "expected `to` after `reassign {}.{}`, found {}",
                            entity.name,
                            field.name,
                            describe(other)
                        )))
                    }
                }
                let mut to = vec![reassign_target(p)?];
                while p.eat_kw(Keyword::Or) {
                    to.push(reassign_target(p)?);
                }
                let span = start.to(p.tok().span);
                out.push(ReleaseEffect::Reassign {
                    entity,
                    field,
                    to,
                    span,
                });
            }
            other => {
                return Err(p.err_here(format!(
                    "expected `remove`, `reassign` or `}}` in the release block, found {}",
                    describe(&other)
                )))
            }
        }
    }
}

fn entity_field(p: &mut Parser) -> Result<(Ident, Ident), ParseError> {
    let entity = ident(p, "an entity")?;
    p.expect(&TokenKind::Dot, "`.` and the ref field")?;
    let field = segment(p, "a ref field")?;
    Ok((entity, field))
}

fn reassign_target(p: &mut Parser) -> Result<ReassignTarget, ParseError> {
    let start = p.tok().span;
    let entity = ident(
        p,
        "`owner` or a lookup such as `people(member == $member).manager`",
    )?;
    if entity.name == "owner" && !matches!(p.peek(), TokenKind::LParen) {
        return Ok(ReassignTarget::Owner { span: start });
    }
    p.expect(&TokenKind::LParen, "`(` and the row to look up")?;
    let inner = guard_expr(p)?;
    p.expect(&TokenKind::RParen, "`)` closing the lookup")?;
    p.expect(&TokenKind::Dot, "`.` and the field that names the member")?;
    let field = segment(p, "the field that names the member")?;
    Ok(ReassignTarget::Lookup {
        entity,
        inner,
        span: start.to(field.span),
        field,
    })
}

fn field_decl(p: &mut Parser) -> Result<FieldDecl, ParseError> {
    let name = ident(p, "a field name")?;

    let mut text_max = None;
    let (kind, mut optional) = if matches!(p.peek(), TokenKind::Arrow) {
        p.bump();
        let target = ident(p, "the referenced entity")?;
        let opt = eat_question(p);
        (
            FieldKind::Ref {
                target,
                owned: false,
                tree: false,
            },
            opt,
        )
    } else {
        p.expect(&TokenKind::Colon, "`:` after the field name")?;
        let (ty, max) = scalar_ty(p)?;
        text_max = max;
        let opt = eat_question(p);
        (FieldKind::Scalar(ty), opt)
    };

    let mut f = FieldDecl {
        name,
        kind,
        optional,
        text_max,
        default: None,
        unique: false,
        index: false,
        sealed: false,
        indexed: false,
        refinable: false,
        inline: false,
        private: false,
        normalize: None,
        public: false,
        span: Span::new(0, 0, 0, 0),
    };
    f.span = f.name.span;

    // Modifiers, in any order, until the next field or `}`.
    loop {
        match p.peek().clone() {
            // `unique` is a field modifier AND the head of an entity-level
            // composite key, and the lexer emits no `Newline` inside a
            // brace (G6) — so this loop would otherwise eat the `unique`
            // that opens the NEXT line. The parenthesis separates them: a
            // field's `unique` never has one, a composite key always does,
            // and one token of lookahead sees which.
            TokenKind::Keyword(Keyword::Unique) if !matches!(p.peek_at(1), TokenKind::LParen) => {
                p.bump();
                f.unique = true;
            }
            // `index` has the same two spellings as `unique`, told apart the
            // same way: a field's `index` is never followed by `(`, and the
            // entity clause `index (a, b)` always is (reported fix 25w).
            TokenKind::Keyword(Keyword::Index) if !matches!(p.peek_at(1), TokenKind::LParen) => {
                p.bump();
                f.index = true;
            }
            TokenKind::Keyword(Keyword::Sealed) => {
                p.bump();
                f.sealed = true;
            }
            TokenKind::Keyword(Keyword::Indexed) => {
                p.bump();
                f.indexed = true;
            }
            // `public` on the field's own line marks the field. On a line of
            // its own it is the entity line `public` (or `public*`): the lexer
            // emits no `Newline` inside a brace (G6), so the line break is
            // read off the spans, as `group` does.
            TokenKind::Keyword(Keyword::Public) if !p.at_line_break() => {
                p.bump();
                f.public = true;
            }
            TokenKind::Keyword(Keyword::Owned) => {
                let t = p.bump();
                match &mut f.kind {
                    FieldKind::Ref { owned, .. } => *owned = true,
                    _ => return Err(p.err_at(t.span, "`owned` marks a ref, not a scalar field")),
                }
            }
            TokenKind::Keyword(Keyword::Tree) => {
                let t = p.bump();
                match &mut f.kind {
                    FieldKind::Ref { tree, .. } => *tree = true,
                    _ => return Err(p.err_at(t.span, "`tree` marks a ref, not a scalar field")),
                }
            }
            TokenKind::Keyword(Keyword::Normalize) => {
                p.bump();
                p.expect(&TokenKind::LParen, "`(` after `normalize`")?;
                f.normalize = Some(ident(p, "a normalizer name")?);
                p.expect(&TokenKind::RParen, "`)` to close `normalize`")?;
            }
            // `refinable` is not reserved — it may only appear here.
            TokenKind::Ident(w) if w == "refinable" => {
                p.bump();
                f.refinable = true;
            }
            // `inline` is contextual for the same reason: reserving it
            // would break every schema with a field called `inline`, and
            // the modifier list is the only position it can occupy.
            // Whether it is MEANINGFUL on this field's type is the
            // binder's question — the parser's job is to record what was
            // written, so the refusal can point at the word.
            TokenKind::Ident(w) if w == "inline" => {
                p.bump();
                f.inline = true;
            }
            // `private` — the third custody tier (DL §2, Content types).
            // Contextual for `inline`'s reason, and **admitted by the
            // parser even though no runtime path can serve it yet**: a
            // language that cannot spell the tier cannot refuse it by
            // name, and "unknown modifier" is a worse answer than "this is
            // client-sealed custody and F7 Phase 6 has not landed".
            TokenKind::Ident(w) if w == "private" => {
                p.bump();
                f.private = true;
            }
            TokenKind::Eq => {
                p.bump();
                f.default = Some(if matches!(f.kind, FieldKind::Scalar(ScalarTy::Json)) {
                    json_default(p)?
                } else {
                    p.expr()?
                });
            }
            TokenKind::Question => {
                p.bump();
                optional = true;
            }
            _ => break,
        }
    }
    f.optional = optional;
    Ok(f)
}

fn eat_question(p: &mut Parser) -> bool {
    if matches!(p.peek(), TokenKind::Question) {
        p.bump();
        true
    } else {
        false
    }
}

/// A `json` field's default: `{}`, `[]` or `null` (28b).
///
/// The language has no document literal, so the two empty containers are
/// spelled here and nowhere else, and they reach the schema as the JSON
/// text they stand for. `null` is the ordinary null literal; the schema
/// refuses it on a field that does not admit null. Anything else is
/// refused by name, so `= { a: 1 }` says what is allowed instead of
/// failing somewhere inside the expression grammar.
fn json_default(p: &mut Parser) -> Result<Expr, ParseError> {
    let (close, text) = match p.peek() {
        TokenKind::LBrace => (TokenKind::RBrace, "{}"),
        TokenKind::LBracket => (TokenKind::RBracket, "[]"),
        _ => {
            let e = p.expr()?;
            // `"{}"` and `"[]"` are the same defaults spelled as their JSON
            // text, which is how a builder that prints expressions (the
            // CDK) writes them back; the schema refuses any other text
            // (`json_default`).
            if matches!(e, Expr::NullLit { .. } | Expr::StrLit { .. }) {
                return Ok(e);
            }
            return Err(p.err_at(
                e.span(),
                "a `json` field's default is `{}`, `[]` or `null`; a document with contents \
                 is written by an op, not by the shape",
            ));
        }
    };
    let start = p.bump().span;
    if *p.peek() != close {
        return Err(p.err_here(
            "a `json` field's default is `{}`, `[]` or `null`; a document with contents is \
             written by an op, not by the shape",
        ));
    }
    let end = p.bump().span;
    Ok(Expr::StrLit {
        value: text.to_string(),
        span: start.to(end),
    })
}

/// A declared type and, for `text(64)`, its length bound.
///
/// The bound comes back beside the type rather than inside it: `text(64)`
/// is a `text` with a constraint, not a tenth type, so every consumer that
/// matches `ScalarTy::Text` keeps matching and the bound lands on the
/// declaration (`FieldDecl::text_max`, `ParamDecl::text_max`) where the
/// schema and the door read it. Only `text` takes a length; `duration`
/// takes its unit in the same parentheses, `money` its places (27ak), and
/// every other type refuses one by name so `int(64)` says what is wrong
/// instead of "expected `)`".
fn scalar_ty(p: &mut Parser) -> Result<(ScalarTy, Option<u32>), ParseError> {
    let t = p.peek().clone();
    let TokenKind::Ident(word) = t else {
        return Err(p.err_here(format!("expected a type, found {}", describe(&t))));
    };
    let word_span = p.bump().span;
    if word == "money" && !matches!(p.peek(), TokenKind::LParen) {
        p.bare_money.push(word_span);
    }
    if matches!(p.peek(), TokenKind::LParen)
        && !matches!(word.as_str(), "text" | "duration" | "money" | "decimal")
    {
        return Err(p.err_here(format!(
            "`{word}` takes no parenthesis — only `text(<n>)` declares a length bound, \
             `money(<places>)` and `decimal(<places>)` their places and \
             `duration(<unit>)` its unit"
        )));
    }
    Ok(match word.as_str() {
        // DL §2 Types: "`text` (boundable `text(64)`)". The bound is a
        // count of characters, enforced at the door for a parameter and
        // by the schema for a field.
        "text" => {
            if !matches!(p.peek(), TokenKind::LParen) {
                return Ok((ScalarTy::Text, None));
            }
            p.bump();
            let n = match p.peek().clone() {
                TokenKind::Int(n) if n > 0 && n <= i128::from(u32::MAX) => {
                    p.bump();
                    n as u32
                }
                other => {
                    return Err(p.err_here(format!(
                        "`text(<n>)` bounds the length in characters, so `<n>` is a positive \
                         integer literal, found {}",
                        describe(&other)
                    )))
                }
            };
            p.expect(&TokenKind::RParen, "`)` to close the text bound")?;
            (ScalarTy::Text, Some(n))
        }
        "int" => (ScalarTy::Int, None),
        // `money(2)`: whole minor units, with the currency's places (ruled
        // 2026-10-04, 27ak). Bare `money` is `money(0)` — whole units,
        // which is what `money` meant before places existed, so a program
        // written then keeps its meaning (see `ScalarTy`).
        "money" => (ScalarTy::Money(places(p, "money")?), None),
        // `rate: decimal(4)`: an exact decimal stored at a declared scale,
        // for rates and unit prices (27ak). The scale is required: a bare
        // `decimal` would leave the column's places to a default nobody
        // chose, and a rate's precision is the author's decision.
        "decimal" => {
            if !matches!(p.peek(), TokenKind::LParen) {
                return Err(p.err_here(
                    "`decimal` declares its places — `decimal(4)` holds four digits after \
                     the point",
                ));
            }
            (ScalarTy::Decimal(places(p, "decimal")?), None)
        }
        "bool" => (ScalarTy::Bool, None),
        "date" => (ScalarTy::Date, None),
        "datetime" => (ScalarTy::Datetime, None),
        "id" => (ScalarTy::Id, None),
        // DL §2, Content types. `blob` holds a reference to managed
        // binary content; `uri` holds an address with no custody. Neither
        // takes a parenthesised argument: `blob private` and `blob inline`
        // are *modifiers* on the field, not parameters on the type, which
        // is what lets them compose orthogonally the way DL says they do.
        "blob" => (ScalarTy::Blob, None),
        "uri" => (ScalarTy::Uri, None),
        // A JSON document (28b), stored as `jsonb`. It takes no argument:
        // the size and depth limits are the deployment's settings, not
        // part of the type.
        "json" => (ScalarTy::Json, None),
        "duration" => {
            p.expect(&TokenKind::LParen, "`(` — `duration` declares its unit")?;
            let u = ident(p, "a duration unit")?;
            let unit = DurationUnit::parse(&u.name)
                .ok_or_else(|| p.err_at(u.span, format!("unknown duration unit `{}`", u.name)))?;
            p.expect(&TokenKind::RParen, "`)` to close the duration unit")?;
            (ScalarTy::Duration(unit), None)
        }
        other => {
            // The rejection names the set, and the set is **derived** from
            // `ScalarTy::DECLARABLE` rather than written out beside it. A
            // hand-written list here is a second inventory that drifts
            // silently, and this one already had: it named eight words
            // while the language admitted nine. `the_closed_set_message_\
            // names_every_declarable_type` now holds the two together.
            return Err(p.err_here(format!(
                "unknown type `{other}` — the type set is closed ({})",
                declarable_type_words()
            )));
        }
    })
}

/// `(<places>)` after `money`, or nothing (places 0).
///
/// The places are a digit count from 0 to [`DIVISION_SCALE`]: no stored
/// value may be finer than the twelve places `/` computes at, so a stored
/// value widened into an expression is never rounded.
fn places(p: &mut Parser, word: &str) -> Result<u8, ParseError> {
    if !matches!(p.peek(), TokenKind::LParen) {
        return Ok(0);
    }
    p.bump();
    let n = match p.peek().clone() {
        TokenKind::Int(n) if (0..=i128::from(DIVISION_SCALE)).contains(&n) => {
            p.bump();
            n as u8
        }
        other => {
            return Err(p.err_here(format!(
                "`{word}(<places>)` counts the digits after the point, so `<places>` is an \
                 integer from 0 to {DIVISION_SCALE}, found {}",
                describe(&other)
            )))
        }
    };
    p.expect(&TokenKind::RParen, "`)` to close the places")?;
    Ok(n)
}

/// The declarable type words, comma-separated, in inventory order.
///
/// One producer for the parser's closed-set rejection and for the test
/// that holds the editor grammar's `support.type.ordo` to the same list.
pub fn declarable_type_words() -> String {
    ScalarTy::DECLARABLE
        .iter()
        .map(|t| t.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

// ── ops ──────────────────────────────────────────────────────────────────

fn op_decl(p: &mut Parser) -> Result<OpDecl, ParseError> {
    let start = p.bump().span; // `op`
    let name = ident(p, "an op name")?;

    let mut params = Vec::new();
    if matches!(p.peek(), TokenKind::LParen) {
        p.bump();
        if !matches!(p.peek(), TokenKind::RParen) {
            loop {
                params.push(param_decl(p)?);
                if matches!(p.peek(), TokenKind::Comma) {
                    p.bump();
                    continue;
                }
                break;
            }
        }
        p.expect(&TokenKind::RParen, "`)` to close the parameter list")?;
    }

    let mut flags = OpFlags::default();
    loop {
        match p.peek() {
            TokenKind::Keyword(Keyword::Unbounded) => {
                p.bump();
                flags.unbounded = true;
            }
            TokenKind::Keyword(Keyword::Unguarded) => {
                p.bump();
                flags.unguarded = true;
            }
            TokenKind::Keyword(Keyword::Reproducible) => {
                p.bump();
                flags.reproducible = true;
            }
            TokenKind::Keyword(Keyword::Nopause) => {
                p.bump();
                flags.nopause = true;
            }
            TokenKind::Keyword(Keyword::Public) => {
                p.bump();
                flags.public = true;
            }
            // `download` is contextual: it is a flag only here, between the
            // parameters and the `=`, so a field or an op named `download`
            // keeps working.
            TokenKind::Ident(w) if w == "download" => {
                p.bump();
                flags.download = true;
            }
            _ => break,
        }
    }

    p.expect(&TokenKind::Eq, "`=` before the op body")?;
    skip_newlines(p);
    // A body opening with `set`/`del` — or with a `let` bound to one — is a
    // write group; statements chain until the next declaration.
    if starts_write_stmt(p) {
        let mut writes = Vec::new();
        loop {
            writes.push(write_stmt(p)?);
            skip_newlines(p);
            if !starts_write_stmt(p) {
                break;
            }
        }
        let span = start.to(p.tok().span);
        return Ok(OpDecl {
            name,
            params,
            flags,
            body: OpBody::Write(writes),
            span,
        });
    }
    let pipeline = pipeline(p)?;
    let span = start.to(pipeline.span);
    Ok(OpDecl {
        name,
        params,
        flags,
        body: OpBody::Read(pipeline),
        span,
    })
}

/// Does a write statement start here?
///
/// `set` and `del` say so themselves. The third opener is `let name = set …`
/// (DL §3, "Naming a freshly created row in the same op"), and it is the one
/// place the op grammar needs more than one token of lookahead: `let` also
/// opens a **read** pipeline's CTE binding, and the two are told apart only
/// by what follows the `=`. Four tokens decide it, exactly and with no
/// backtracking — `let`, a name, `=`, then a write verb.
///
/// `del` is admitted here deliberately even though a delete can never be
/// bound. Sending `let x = del …` into [`write_stmt`] gets the reader the
/// real answer ("a delete names no row") instead of the read parser's
/// complaint about a keyword where a source should be, which is what the
/// same input produced before this function existed.
fn starts_write_stmt(p: &Parser) -> bool {
    match p.peek() {
        TokenKind::Keyword(Keyword::Set) | TokenKind::Keyword(Keyword::Del) => true,
        TokenKind::Keyword(Keyword::Let) => {
            matches!(p.peek_at(1), TokenKind::Ident(_))
                && matches!(p.peek_at(2), TokenKind::Eq)
                && matches!(
                    p.peek_at(3),
                    TokenKind::Keyword(Keyword::Set) | TokenKind::Keyword(Keyword::Del)
                )
        }
        _ => false,
    }
}

fn write_stmt(p: &mut Parser) -> Result<WriteStmt, ParseError> {
    // `let name = <write>`. The binding is parsed before the verb because
    // that is the order it is written in; whether the verb may *carry* one
    // is decided below (`del`, never) and at registration (`set`, only in
    // CREATE mode — `unbindable_write`).
    let mut binding = None;
    let mut binding_start = None;
    if matches!(p.peek(), TokenKind::Keyword(Keyword::Let)) {
        binding_start = Some(p.bump().span);
        binding = Some(ident(p, "a name for the created row")?);
        p.expect(&TokenKind::Eq, "`=` after the let name")?;
    }
    if matches!(p.peek(), TokenKind::Keyword(Keyword::Del)) {
        if let Some(name) = &binding {
            return Err(p.err_at(
                name.span,
                format!(
                    "`let {} = del …` names nothing — a delete produces no row, in any \
                     mode; only a create can be bound (`let {} = set <entity> {{ … }}`)",
                    name.name, name.name
                ),
            ));
        }
    }
    if p.eat_kw(Keyword::Del) {
        let start = p.tok().span;
        let entity = ident(p, "an entity to delete from")?;
        // Rule 5 [settled]: **bare `del entity` does not parse.** A delete
        // states its target — a condition, an addressed row, or the
        // explicit `(true)` that means every row. The reason is
        // greppability: an unconditional delete must be a deliberate act a
        // reviewer can find in the source, never what a forgotten
        // condition quietly turns into.
        let (cond, mut end, addressed) = match p.peek() {
            TokenKind::LParen => {
                p.bump();
                let cond = p.condition()?;
                let end = p.expect(&TokenKind::RParen, "`)` closing the condition")?;
                (cond, end.span, false)
            }
            TokenKind::LBrace => {
                let (cond, end) = del_address(p)?;
                (cond, end, true)
            }
            _ => {
                return Err(p.err_here(
                    "a delete must state its target: `del <entity>(<condition>)`, \
                     `del <entity> { id: $id }`, or `del <entity>(true)` to delete every \
                     row — bare `del <entity>` does not parse",
                ))
            }
        };
        // Rule 6 / §tree [settled]: subtree deletion is its own greppable
        // spelling, and it hangs off ONE anchor. `subtree` after a ranged
        // delete is refused for the same reason a ranged reparent is: many
        // anchors mean overlapping subtrees, and no coherent order over
        // them.
        let mut subtree = false;
        if matches!(p.peek(), TokenKind::Keyword(Keyword::Subtree)) {
            let t = p.bump();
            if !addressed {
                return Err(p.err_at(
                    t.span,
                    "`subtree` cascades from one anchor — address it: \
                     `del <entity> { id: $id } subtree`",
                ));
            }
            subtree = true;
            end = t.span;
        }
        return Ok(WriteStmt::Del {
            entity,
            cond,
            subtree,
            span: start.to(end),
        });
    }
    let start = binding_start.unwrap_or(p.tok().span);
    if !p.eat_kw(Keyword::Set) {
        return Err(p.err_here("expected `set` or `del`"));
    }
    let entity = ident(p, "an entity to write")?;
    let mut cond = None;
    if matches!(p.peek(), TokenKind::LParen) {
        p.bump();
        cond = Some(p.condition()?);
        p.expect(&TokenKind::RParen, "`)` closing the condition")?;
    }
    p.expect(&TokenKind::LBrace, "`{` opening the field block")?;
    let mut fields = Vec::new();
    if !matches!(p.peek(), TokenKind::RBrace) {
        loop {
            let f = ident(p, "a field name")?;
            p.expect(&TokenKind::Colon, "`:` after the field name")?;
            let v = p.expr()?;
            fields.push((f, v));
            if matches!(p.peek(), TokenKind::Comma) {
                p.bump();
                continue;
            }
            break;
        }
    }
    let end = p.expect(&TokenKind::RBrace, "`}` closing the field block")?;
    Ok(WriteStmt::Set {
        binding,
        entity,
        cond,
        fields,
        span: start.to(end.span),
    })
}

/// `{ id: $id }` — the addressed delete's braces, returned as the
/// condition they mean plus the span of the closing brace.
///
/// A delete's braces address, they do not assign: there is nothing to set
/// on a row that is about to stop existing. `id` is the only key this form
/// admits — rule 8's other identity spellings (a covered unique key) are
/// converge-or-create machinery, and resurrection-by-delete is not a
/// thing — so a non-`id` brace is a grammar error rather than a silently
/// ranged delete.
fn del_address(p: &mut Parser) -> Result<(Condition, Span), ParseError> {
    let open = p.expect(&TokenKind::LBrace, "`{` opening the addressed delete")?;
    let f = ident(p, "`id` — a delete's braces address one row")?;
    if f.name != "id" {
        return Err(p.err_at(
            f.span,
            format!(
                "a delete's braces address a row by `id`, not `{}` — \
                 use `del <entity>({} == …)` to delete by value",
                f.name, f.name
            ),
        ));
    }
    p.expect(&TokenKind::Colon, "`:` after `id`")?;
    let value = p.expr()?;
    let close = p.expect(
        &TokenKind::RBrace,
        "`}` closing the addressed delete — `id` is the only key it takes",
    )?;
    let span = open.span.to(close.span);
    let cond = Condition::Compare {
        op: crate::ast::CmpOp::Eq,
        lhs: Expr::Path {
            segments: vec![f.clone()],
            span: f.span,
        },
        rhs: value,
        span,
    };
    Ok((cond, close.span))
}

fn param_decl(p: &mut Parser) -> Result<ParamDecl, ParseError> {
    let TokenKind::Param(pname) = p.peek().clone() else {
        return Err(p.err_here(format!(
            "expected a `$parameter`, found {}",
            describe(p.peek())
        )));
    };
    let t = p.bump();
    let name = Ident {
        name: pname,
        span: t.span,
    };
    p.expect(&TokenKind::Colon, "`:` after the parameter name")?;
    let (ty, text_max) = scalar_ty(p)?;
    let max = param_bound(p, ty)?;
    // `filled` — DL §2 Parameters, "`filled` — the SDK-supplied marker
    // [settled]". It follows the type and the bound and precedes the
    // default, and the two are refused together: a default is a value in
    // the artifact, `filled` is the statement that the value is NOT in the
    // artifact and the generated client produces it. One parameter cannot
    // say both.
    let filled = if matches!(p.peek(), TokenKind::Keyword(Keyword::Filled)) {
        let t = p.bump();
        if matches!(p.peek(), TokenKind::Eq) {
            return Err(p.err_at(
                t.span,
                format!(
                    "`${}` is `filled` and has a default — `filled` says the generated client \
                     produces the value when the caller does not, so a default would never be \
                     taken; drop one of them",
                    name.name
                ),
            ));
        }
        true
    } else {
        false
    };
    let mut default = None;
    if matches!(p.peek(), TokenKind::Eq) {
        p.bump();
        default = Some(p.expr()?);
    }
    let span = name.span;
    Ok(ParamDecl {
        name,
        ty,
        max,
        text_max,
        default,
        filled,
        span,
    })
}

/// `<= <literal>` after a parameter's type (DL §2 Parameters).
///
/// # Which types admit a bound, and why not the others
///
/// A declared bound is a promise the DOOR keeps — DL: "Bounds are enforced
/// at the door, before planning" — so admitting one on a type the door
/// cannot compare would put an unenforced constraint on the manifest,
/// which is worse than not spelling it. `api-engine-design.md`'s door
/// already compares `int`/`money` as exact decimals and `date`
/// lexicographically, and says in as many words that "other types take no
/// bounds in v1"; `duration` joins them here (the door converts to
/// milliseconds, which is exactly what `engine-plan.md` G10 says the
/// duration ruling needs — "a door-level bound check that reuses the
/// machinery every other parameter bound already uses").
///
/// So:
///
/// - `int`, `money`, `date` — admitted.
/// - `text` — its bound is the length form `text(64)`, a different
///   construct (and one this parser does not have either).
/// - `bool`, `id` — nothing to order.
/// - `datetime` — **owed, not declined.** The type is orderable and the
///   spec would admit the bound; the door has no datetime comparison and
///   no `ParamTy` for it (a `datetime` parameter validates as text today),
///   so accepting the bound here would mint a promise nothing keeps.
/// - `duration` — **owed, and the owing is bigger than a bound.**
///   `$everyDays: duration(ms) <= 60d` is spelled in the docs and is
///   legal, but making it real at the door needs three things this engine
///   does not have: a unit conversion (`60d` against a `duration(ms)`
///   declaration — and `check_duration_units` refuses cross-unit
///   comparison *everywhere else*, deliberately, as owed work), a
///   `ParamTy::Duration` that carries its unit, and the class check for a
///   months-bearing value reaching an exact-class parameter
///   (`duration_unit_class`, DL §2 — no producer anywhere today). Any one
///   of them missing makes the bound decoration.
fn param_bound(p: &mut Parser, ty: ScalarTy) -> Result<Option<Expr>, ParseError> {
    // `>=` and a range form are X-077's *proposal*, not a settled
    // spelling. Refused by name so an author reaching for one learns the
    // language does not have it, instead of "expected `)`".
    if matches!(p.peek(), TokenKind::GtEq) {
        return Err(p.err_here(
            "a parameter bound is `<=` — the language has no `>=` and no range form \
             (every bound the spec writes is an upper bound)",
        ));
    }
    if !matches!(p.peek(), TokenKind::LtEq) {
        return Ok(None);
    }
    let le = p.bump();
    let admits = matches!(
        ty,
        ScalarTy::Int | ScalarTy::Money(_) | ScalarTy::Decimal(_) | ScalarTy::Date
    );
    if !admits {
        let why = match ty {
            ScalarTy::Text => "a `text` parameter's bound is its length, `text(64)`",
            ScalarTy::Datetime => {
                "`datetime` bounds are not enforced at the door yet, and a bound nothing \
                 checks is worse than no bound"
            }
            ScalarTy::Duration(_) => {
                "`duration` bounds are owed: the door would have to convert units, which \
                 this engine refuses to do anywhere else, and to reject a months-bearing \
                 value reaching an exact-class parameter, which it cannot yet say"
            }
            _ => "the type has no ordering to bound",
        };
        return Err(p.err_at(
            le.span,
            format!(
                "`{}` takes no `<=` bound — {why} (bounds: int, money, decimal, date)",
                ty.as_str()
            ),
        ));
    }
    let start = p.tok().span;
    let e = p.expr()?;
    // A bound is a LITERAL of the parameter's own type. Not a parameter
    // (bounds are proved at registration, and a value that arrives with
    // the call proves nothing), not an expression, not a literal of some
    // other type — `$n: int <= 5.5` would be a bound the door could only
    // enforce by rounding, and which way is a decision nobody made.
    let ok = matches!(
        (ty, &e),
        (ScalarTy::Int, Expr::IntLit { .. })
            | (
                ScalarTy::Money(_) | ScalarTy::Decimal(_),
                Expr::IntLit { .. } | Expr::DecimalLit { .. }
            )
            | (ScalarTy::Date, Expr::DateLit { .. })
    );
    if !ok {
        return Err(p.err_at(
            start.to(e.span()),
            format!(
                "a `{}` bound is a `{}` literal — a bound is proved at registration, \
                 so it cannot be a parameter or an expression",
                ty.as_str(),
                ty.as_str()
            ),
        ));
    }
    Ok(Some(e))
}

// ── pipelines ────────────────────────────────────────────────────────────

fn pipeline(p: &mut Parser) -> Result<Pipeline, ParseError> {
    let start = p.tok().span;

    // `let name = <pipeline>` bindings precede the source line. A let's
    // body is itself a pipeline (without further lets — nesting is spelled
    // by chaining lets, not indenting them).
    let mut lets = Vec::new();
    while p.eat_kw(Keyword::Let) {
        let name = ident(p, "a let name")?;
        p.expect(&TokenKind::Eq, "`=` after the let name")?;
        skip_newlines(p);
        let body = pipeline_body(p)?;
        let span = name.span.to(body.span);
        lets.push(LetDecl {
            name,
            pipeline: body,
            span,
        });
        skip_newlines(p);
    }

    let mut inner = pipeline_body(p)?;
    inner.lets = lets;
    inner.span = start.to(inner.span);
    Ok(inner)
}

/// A pipeline without a `let` prefix: the source line and its steps.
fn pipeline_body(p: &mut Parser) -> Result<Pipeline, ParseError> {
    let start = p.tok().span;

    let mut sources = vec![source(p)?];
    // Additional sources sit on the same line: `products[name]  productRevenue[…]`.
    while matches!(p.peek(), TokenKind::Ident(_)) {
        sources.push(source(p)?);
    }

    let mut steps = Vec::new();
    let mut end = sources.last().map(|s| s.span).unwrap_or(start);
    loop {
        // A step may follow on the same line or the next one; only a new
        // declaration keyword ends the body.
        let mark = p.mark();
        skip_newlines(p);
        let is_step = matches!(
            p.peek(),
            TokenKind::Bar
                | TokenKind::Keyword(Keyword::Sort)
                | TokenKind::Keyword(Keyword::Take)
                | TokenKind::Keyword(Keyword::Group)
                | TokenKind::Keyword(Keyword::Reduce)
                | TokenKind::Keyword(Keyword::Distinct)
                | TokenKind::Keyword(Keyword::Match)
                | TokenKind::Keyword(Keyword::Append)
        );
        if !is_step {
            p.reset(mark);
            break;
        }
        let s = step(p)?;
        end = step_span(&s);
        steps.push(s);
    }

    Ok(Pipeline {
        lets: Vec::new(),
        sources,
        steps,
        span: start.to(end),
    })
}

fn step_span(s: &Step) -> Span {
    match s {
        Step::Filter { span, .. }
        | Step::Sort { span, .. }
        | Step::Take { span, .. }
        | Step::Group { span, .. }
        | Step::Reduce { span, .. }
        | Step::Distinct { span }
        | Step::Match { span, .. }
        | Step::Append { span, .. } => *span,
    }
}

fn source(p: &mut Parser) -> Result<Source, ParseError> {
    let mut name = ident(p, "a source (an entity or a registered op)")?;
    reject_foreign(p, &name.name, name.span)?;
    qualify(p, &mut name);

    let mut args = Vec::new();
    if matches!(p.peek(), TokenKind::LParen) {
        let open = p.bump().span;
        // Empty parens are a spelling of "no arguments", and the language
        // already spells that by omitting them (R-C1, C099).
        if matches!(p.peek(), TokenKind::RParen) {
            return Err(p.err_at(
                open,
                "empty `()` — omit the parentheses when there are no arguments",
            ));
        }
        loop {
            args.push(source_arg(p)?);
            if matches!(p.peek(), TokenKind::Comma) {
                p.bump();
                continue;
            }
            break;
        }
        p.expect(&TokenKind::RParen, "`)` to close the call")?;
    }

    let mut projection = None;
    if matches!(p.peek(), TokenKind::LBracket) {
        p.bump();
        let mut items = Vec::new();
        if !matches!(p.peek(), TokenKind::RBracket) {
            loop {
                items.push(proj_item(p)?);
                if matches!(p.peek(), TokenKind::Comma) {
                    p.bump();
                    continue;
                }
                break;
            }
        }
        p.expect(&TokenKind::RBracket, "`]` to close the projection")?;
        projection = Some(items);
    }

    let span = name.span;
    Ok(Source {
        name,
        args,
        projection,
        span,
    })
}

/// `$cap: <expr>` is a named binding; anything else is a condition. One token
/// of lookahead settles it — a `$param` followed by `:` cannot begin a
/// condition, because no condition operator is `:`.
fn source_arg(p: &mut Parser) -> Result<SourceArg, ParseError> {
    if matches!(p.peek(), TokenKind::Param(_)) && matches!(p.peek_at(1), TokenKind::Colon) {
        let TokenKind::Param(pname) = p.peek().clone() else {
            unreachable!()
        };
        let t = p.bump();
        p.bump(); // `:`
        let value = p.expr()?;
        let span = t.span.to(value.span());
        return Ok(SourceArg::Named {
            name: Ident {
                name: pname,
                span: t.span,
            },
            value,
            span,
        });
    }
    Ok(SourceArg::Cond(p.condition()?))
}

fn proj_item(p: &mut Parser) -> Result<ProjItem, ParseError> {
    match p.peek().clone() {
        TokenKind::Star => {
            let t = p.bump();
            Ok(ProjItem::Star { span: t.span })
        }
        TokenKind::Minus => {
            let t = p.bump();
            let name = ident(p, "a field to exclude")?;
            Ok(ProjItem::Exclude {
                span: t.span.to(name.span),
                name,
            })
        }
        TokenKind::Ident(_) if matches!(p.peek_at(1), TokenKind::Colon) => {
            let out = ident(p, "an output field name")?;
            p.bump(); // `:`
            let value = p.expr()?;
            let span = out.span.to(value.span());
            Ok(ProjItem::Named { out, value, span })
        }
        // A path head. A reserved word heads a path only when a dot
        // follows it (`group.displayName` over `groupMembers`): the word
        // then names the platform's ref and nothing else could — see
        // `segment`. A bare keyword here is still the keyword.
        TokenKind::Ident(_) | TokenKind::Keyword(_)
            if matches!(p.peek(), TokenKind::Ident(_))
                || matches!(p.peek_at(1), TokenKind::Dot) =>
        {
            let mut path = vec![segment(p, "a field name")?];
            while matches!(p.peek(), TokenKind::Dot) {
                p.bump();
                path.push(segment(p, "a field name after `.`")?);
            }
            let span = path[0].span.to(path[path.len() - 1].span);
            Ok(ProjItem::Field { path, span })
        }
        other => Err(p.err_here(format!(
            "expected a projection item, found {}",
            describe(&other)
        ))),
    }
}

fn step(p: &mut Parser) -> Result<Step, ParseError> {
    match p.peek().clone() {
        TokenKind::Bar => {
            let t = p.bump();
            let cond = p.condition()?;
            Ok(Step::Filter {
                span: t.span.to(cond.span()),
                cond,
            })
        }
        TokenKind::Keyword(Keyword::Distinct) => {
            let t = p.bump();
            Ok(Step::Distinct { span: t.span })
        }
        TokenKind::Keyword(Keyword::Sort) => {
            let t = p.bump();
            let mut keys = Vec::new();
            loop {
                let desc = if matches!(p.peek(), TokenKind::Minus) {
                    p.bump();
                    true
                } else {
                    false
                };
                let expr = p.expr()?;
                keys.push(SortKey {
                    span: expr.span(),
                    expr,
                    desc,
                });
                if matches!(p.peek(), TokenKind::Comma) {
                    p.bump();
                    continue;
                }
                break;
            }
            let end = keys.last().map(|k| k.span).unwrap_or(t.span);
            Ok(Step::Sort {
                keys,
                span: t.span.to(end),
            })
        }
        TokenKind::Keyword(Keyword::Take) => {
            let t = p.bump();
            let mut keys = Vec::new();
            if matches!(p.peek(), TokenKind::LBracket) {
                p.bump();
                loop {
                    keys.push(p.expr()?);
                    if matches!(p.peek(), TokenKind::Comma) {
                        p.bump();
                        continue;
                    }
                    break;
                }
                p.expect(&TokenKind::RBracket, "`]` to close the take key list")?;
            }
            // DL "Other steps": the count is a literal OR a parameter. A
            // parameter must carry a bound, but that is the registration
            // gate's word (`unbounded_take`) — it needs the op's parameter
            // list, which a step parser does not have.
            let (n, end) = match p.peek().clone() {
                TokenKind::Int(v) => {
                    let nt = p.bump();
                    let v = i64::try_from(v)
                        .map_err(|_| p.err_at(nt.span, "take count out of range"))?;
                    (
                        TakeCount::Lit {
                            n: v,
                            span: nt.span,
                        },
                        nt.span,
                    )
                }
                TokenKind::Param(name) => {
                    let nt = p.bump();
                    (
                        TakeCount::Param {
                            name: Ident {
                                name,
                                span: nt.span,
                            },
                        },
                        nt.span,
                    )
                }
                other => {
                    return Err(p.err_here(format!(
                        "expected a row count or a `$parameter` after `take`, found {}",
                        describe(&other)
                    )))
                }
            };
            Ok(Step::Take {
                keys,
                n,
                span: t.span.to(end),
            })
        }
        TokenKind::Keyword(Keyword::Group) => {
            let t = p.bump();
            let mut keys = Vec::new();
            while !matches!(p.peek(), TokenKind::LBrace) {
                keys.push(p.expr()?);
                if matches!(p.peek(), TokenKind::Comma) {
                    p.bump();
                    continue;
                }
                break;
            }
            if keys.is_empty() {
                return Err(p.err_at(
                    t.span,
                    "`group` needs keys — a keyless aggregation is `reduce`",
                ));
            }
            let (aggs, end) = agg_block(p)?;
            Ok(Step::Group {
                keys,
                aggs,
                span: t.span.to(end),
            })
        }
        TokenKind::Keyword(Keyword::Reduce) => {
            let t = p.bump();
            let (aggs, end) = agg_block(p)?;
            Ok(Step::Reduce {
                aggs,
                span: t.span.to(end),
            })
        }
        TokenKind::Keyword(Keyword::Append) => {
            let t = p.bump();
            let source = source(p)?;
            let span = t.span.to(source.span);
            Ok(Step::Append { source, span })
        }
        TokenKind::Keyword(Keyword::Match) => {
            let t = p.bump();
            let kind = if p.eat_kw(Keyword::Any) {
                MatchKind::Any(source(p)?)
            } else if p.eat_kw(Keyword::None) {
                MatchKind::None(source(p)?)
            } else if p.eat_kw(Keyword::On) {
                MatchKind::On {
                    col: ident(p, "a join-back column")?,
                }
            } else {
                let mut path = vec![ident(p, "a ref path")?];
                while matches!(p.peek(), TokenKind::Dot) {
                    p.bump();
                    path.push(segment(p, "a ref path segment")?);
                }
                MatchKind::Ref { path }
            };
            Ok(Step::Match { kind, span: t.span })
        }
        other => Err(p.err_here(format!(
            "expected a pipeline step, found {}",
            describe(&other)
        ))),
    }
}

/// `spent: sum total` / `n: count`. Aggregate names are deliberately absent
/// from the expression function table (spec §3) — they are legal only here.
fn agg_block(p: &mut Parser) -> Result<(Vec<AggDecl>, Span), ParseError> {
    p.expect(&TokenKind::LBrace, "`{` to open the aggregate block")?;
    let mut aggs = Vec::new();
    loop {
        if matches!(p.peek(), TokenKind::RBrace) {
            break;
        }
        aggs.push(agg_decl(p)?);
        // Two separators, either of them trailing, and mixable: a comma, or
        // the end of the line. The multi-line spelling is what the docs and
        // both corpora write, and it used to be a parse error because the
        // lexer emits no `Newline` inside a brace (G6) — so the break is read
        // off the token spans instead. What is NOT admitted is two aggregates
        // sharing a line with nothing between them: that would let
        // `{ n: count rev: sum total }` parse, and `sum`'s argument is an
        // arbitrary expression, so the parse it produced would be a guess.
        if matches!(p.peek(), TokenKind::Comma) {
            p.bump();
            continue;
        }
        if p.at_line_break() || matches!(p.peek(), TokenKind::RBrace) {
            continue;
        }
        return Err(p.err_here(format!(
            "expected `,` or a line break between aggregates, found {}",
            describe(p.peek())
        )));
    }
    let end = p.expect(&TokenKind::RBrace, "`}` to close the aggregate block")?;
    Ok((aggs, end.span))
}

/// What an aggregate takes after its name — the shape `agg_decl` below
/// parses, named so a reader outside this file can ask.
///
/// It exists because `ordo-gen` generates the CDK's aggregate vocabulary and
/// has to know that `count` takes nothing (or one condition), that `sum`
/// takes one expression, and that `percentile` and `joined` take a literal
/// second argument the parser checks. Without it the generator would carry a
/// second table of the same facts, which is the drift this crate spends its
/// life refusing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggShape {
    /// `count`, and `count(<condition>)` — the conditional aggregate.
    NoneOrCondition,
    /// `sum total`, `avg x`, `min x`, `max x`, `median x`, `distinct x`.
    OneExpr,
    /// `percentile(total, 0.5)` — the second argument is a decimal literal
    /// strictly between 0 and 1, checked at parse time.
    ExprAndFraction,
    /// `joined(name, ", ")` — the second argument is a text literal.
    ExprAndSeparator,
}

/// One list, two projections — the `word_enum!` discipline of `token.rs`
/// applied to a table that is strings rather than an enum.
macro_rules! aggregate_table {
    ( $( $name:literal => $shape:ident ),* $(,)? ) => {
        /// Every aggregate name the language has (DL §Aggregation). Named
        /// here so the rejection message stays true when one is added — and
        /// public, because the editor grammar has to colour exactly this set
        /// and a highlighter carrying its own copy is the second list this
        /// crate spends its life refusing.
        ///
        /// `distinct` is in here *and* in `Keyword`: it is a pipeline step
        /// and an aggregate, which `agg_decl` below handles by hand. It is
        /// the only word in both inventories, and the grammar's parity test
        /// names it as such rather than excusing it.
        pub const AGGREGATE_NAMES: &[&str] = &[ $( $name ),* ];

        /// The same list with the argument shape `agg_decl` parses for each.
        /// A projection of the same table, so a new aggregate reaches every
        /// reader in the commit that adds it.
        pub const AGGREGATE_SHAPES: &[(&str, AggShape)] =
            &[ $( ($name, AggShape::$shape) ),* ];
    };
}

aggregate_table! {
    "count"      => NoneOrCondition,
    "sum"        => OneExpr,
    "avg"        => OneExpr,
    "min"        => OneExpr,
    "max"        => OneExpr,
    "median"     => OneExpr,
    "percentile" => ExprAndFraction,
    "distinct"   => OneExpr,
    "joined"     => ExprAndSeparator,
}

fn agg_decl(p: &mut Parser) -> Result<AggDecl, ParseError> {
    let out = ident(p, "an aggregate output name")?;
    p.expect(&TokenKind::Colon, "`:` after the aggregate name")?;
    // `distinct` is fully reserved (it is also a step), so it arrives as a
    // keyword where every other aggregate name arrives as an ident.
    let func = if matches!(p.peek(), TokenKind::Keyword(Keyword::Distinct)) {
        let t = p.bump();
        Ident {
            name: "distinct".into(),
            span: t.span,
        }
    } else {
        ident(p, "an aggregate function")?
    };
    let mut cond = None;
    let mut arg2 = None;
    let arg = match func.name.as_str() {
        "count" => {
            // `count(total > 5000)` — the conditional aggregate.
            if matches!(p.peek(), TokenKind::LParen) {
                p.bump();
                cond = Some(p.condition()?);
                p.expect(&TokenKind::RParen, "`)` to close the count condition")?;
            }
            None
        }
        // The one-argument family: `sum total`, `median value`,
        // `distinct customer` (count-distinct).
        "sum" | "avg" | "min" | "max" | "median" | "distinct" => {
            no_line_break_before_arg(p, &func)?;
            Some(p.expr()?)
        }
        // `percentile(total, 0.5)` — DL pins p as a literal strictly between
        // 0 and 1, so the check is here and not at run time.
        "percentile" => {
            no_line_break_before_arg(p, &func)?;
            p.expect(&TokenKind::LParen, "`(` after `percentile`")?;
            let field = p.expr()?;
            p.expect(&TokenKind::Comma, "`,` before percentile's fraction")?;
            let frac = p.expr()?;
            // Checked on the source digits, not through a float: `0 < p < 1`
            // means "0." followed by at least one non-zero digit, and nothing
            // else. 0 and 1 are excluded on purpose — those are `min` and
            // `max`, which already have a spelling.
            let ok = match &frac {
                Expr::DecimalLit { digits, .. } => digits
                    .strip_prefix("0.")
                    .is_some_and(|frac| frac.bytes().any(|b| (b'1'..=b'9').contains(&b))),
                _ => false,
            };
            if !ok {
                return Err(p.err_at(
                    frac.span(),
                    "percentile's fraction is a literal strictly between 0 and 1 \
                     — `percentile(salary, 0.95)`, not a percentage and not a parameter",
                ));
            }
            p.expect(&TokenKind::RParen, "`)` to close `percentile`")?;
            arg2 = Some(frac);
            Some(field)
        }
        // `joined(name, ", ")` — ordered string aggregation. The separator is
        // a literal: it is punctuation in the rendered value, not data.
        "joined" => {
            no_line_break_before_arg(p, &func)?;
            p.expect(&TokenKind::LParen, "`(` after `joined`")?;
            let field = p.expr()?;
            p.expect(&TokenKind::Comma, "`,` before joined's separator")?;
            let sep = p.expr()?;
            if !matches!(sep, Expr::StrLit { .. }) {
                return Err(p.err_at(
                    sep.span(),
                    "joined's separator is a text literal — `joined(name, \", \")`",
                ));
            }
            p.expect(&TokenKind::RParen, "`)` to close `joined`")?;
            arg2 = Some(sep);
            Some(field)
        }
        other => {
            // The class travels in the class slot, not smuggled into the
            // prose. It used to be the first word of the message, which is
            // exactly the failure mode 9r names: the corpus matched TEXT
            // for a class the inventory already had a name for.
            return Err(p.err_class_at(
                parser::BARE_FIELD_IN_GROUP,
                func.span,
                format!(
                    "`{other}` is not an aggregate; braces admit aggregates only ({})",
                    AGGREGATE_NAMES.join(", ")
                ),
            ));
        }
    };
    let span = out
        .span
        .to(arg.as_ref().map(|e| e.span()).unwrap_or(func.span));
    Ok(AggDecl {
        out,
        func,
        arg,
        arg2,
        cond,
        span,
    })
}

/// An argument-taking aggregate must have its argument on its own line. Once
/// the line break is a separator, `{ a: sum \n b: count }` would otherwise
/// read `b` as `sum`'s argument and then choke on the `:` — a confusing error
/// for the real mistake, which is a missing field.
fn no_line_break_before_arg(p: &Parser, func: &Ident) -> Result<(), ParseError> {
    if p.at_line_break() {
        return Err(p.err_at(
            func.span,
            format!("`{}` needs its argument on the same line", func.name),
        ));
    }
    Ok(())
}

/// `Condition` and `Expr` are re-exported through the parser's own entry
/// points; this alias keeps the `use` list above honest about what is used.
#[allow(dead_code)]
type _UsesConditionAndExpr = (Condition, Expr);

#[cfg(test)]
mod tests {
    use super::*;

    /// The `market` fixture of `composition-conformance.md` §2.1, verbatim.
    const MARKET: &str = r#"
domain market @ 1.0.0

entity vendors {
  name:   text
  region: text
  active: bool = true
}

entity categories {
  name:   text unique
  parent -> categories?  tree
}

entity products {
  vendor   -> vendors
  category -> categories
  name:   text
  price:  money
  active: bool = true
}

entity customers {
  name:  text
  email: text?  sealed indexed unique normalize(lower)
  tier:  text = "basic"
}

entity orders {
  customer -> customers
  placed: date
  status: text = "open"
  total:  money
  memo:   text = ""   sealed
}

entity orderItems {
  order   -> orders   owned
  product -> products
  qty:    int
  amount: money
}
"#;

    /// The callee library of §2.3, verbatim. All of it must parse before any
    /// corpus case may run, whether or not the planner can lower it yet.
    const LIBRARY: &str = r#"
op activeProducts =
  products(active == true)[name, price, vendor, category]

op cheapActive($cap: money) =
  activeProducts(price < $cap)[name, price, category]

op toolDeals($cap: money, $root: id) =
  cheapActive($cap: $cap, category under $root)[name, price]
  sort price

op recentOrders($since: date = 2026-03-01) =
  orders(placed >= $since)[placed, status, total]
  sort placed

op doneOrders($from: date) =
  orders(status == "done" and placed >= $from)[placed, total, customer]

op spendByCustomer($from: date) =
  doneOrders($from: $from)[total, customer]
  group customer { spent: sum total, n: count }

op monthlyRevenue($from: date) =
  doneOrders($from: $from)[placed, total]
  group month(placed) { revenue: sum total }

op productRevenue =
  orderItems[amount, product]
  group product { sold: count, revenue: sum amount }

op top3Orders =
  orders[placed, total, customer]
  sort -total
  take 3

op catInfo =
  categories[name, parent, depth]

op customerByEmail($e: text) =
  customers(email == $e)[name, tier]

op giftOrders unbounded =
  orders(memo == "gift")[placed, total, customer]

op orderMemos unbounded =
  orders[placed, total, memo]
"#;

    #[test]
    fn fixture_schema_parses() {
        let p = parse_program(MARKET).expect("market schema parses");
        assert_eq!(p.domain.as_ref().unwrap().name.name, "market");
        assert_eq!(p.domain.as_ref().unwrap().version.as_deref(), Some("1.0.0"));
        assert_eq!(p.entities.len(), 6);

        let names: Vec<&str> = p.entities.iter().map(|e| e.name.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "vendors",
                "categories",
                "products",
                "customers",
                "orders",
                "orderItems"
            ]
        );
    }

    #[test]
    fn field_modifiers_land_on_the_right_field() {
        let p = parse_program(MARKET).unwrap();
        let by = |e: &str, f: &str| -> FieldDecl {
            p.entities
                .iter()
                .find(|x| x.name.name == e)
                .unwrap()
                .fields
                .iter()
                .find(|x| x.name.name == f)
                .unwrap()
                .clone()
        };

        // A tree self-ref that is optional.
        let parent = by("categories", "parent");
        assert!(parent.optional);
        assert!(matches!(
            parent.kind,
            FieldKind::Ref {
                tree: true,
                owned: false,
                ..
            }
        ));

        // Four modifiers on one field, plus `?`, in source order.
        let email = by("customers", "email");
        assert!(email.optional && email.sealed && email.indexed && email.unique);
        assert_eq!(email.normalize.as_ref().unwrap().name, "lower");
        assert!(!email.refinable, "refinability is a separate opt-in");

        // A default followed by a modifier on the same field — the boundary
        // the brace block gives no separator for.
        let memo = by("orders", "memo");
        assert!(memo.sealed);
        assert!(memo.default.is_some());

        // Ownership marks the ref, not the entity.
        let order = by("orderItems", "order");
        assert!(matches!(order.kind, FieldKind::Ref { owned: true, .. }));
        assert!(!by("orderItems", "product").optional);
    }

    #[test]
    fn fixture_library_parses() {
        let p = parse_program(LIBRARY).expect("callee library parses");
        assert_eq!(p.ops.len(), 13);

        let op = |n: &str| p.ops.iter().find(|o| o.name.name == n).unwrap();

        // Projection subset on an entity source with a filter.
        let ap = op("activeProducts");
        let app = ap.read_pipeline().expect("read op");
        assert!(ap.params.is_empty());
        assert_eq!(app.sources.len(), 1);
        assert_eq!(app.sources[0].name.name, "products");
        assert_eq!(app.sources[0].projection.as_ref().unwrap().len(), 4);

        // A parameter with a default.
        let ro = op("recentOrders");
        assert_eq!(ro.params[0].name.name, "since");
        assert!(ro.params[0].default.is_some());

        // Named binding and a condition in one argument list.
        let td = op("toolDeals");
        let args = &td.read_pipeline().expect("read op").sources[0].args;
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0], SourceArg::Named { .. }));
        assert!(matches!(args[1], SourceArg::Cond(_)));

        // group with two aggregates, one of them nullary.
        let sc = op("spendByCustomer");
        let Step::Group { keys, aggs, .. } = &sc.read_pipeline().expect("read op").steps[0] else {
            panic!("expected a group step")
        };
        assert_eq!(keys.len(), 1);
        assert_eq!(aggs.len(), 2);
        assert_eq!(aggs[1].func.name, "count");
        assert!(aggs[1].arg.is_none());

        // A computed group key.
        let mr = op("monthlyRevenue");
        assert!(
            matches!(&mr.read_pipeline().expect("read op").steps[0], Step::Group { keys, .. }
            if matches!(keys[0], Expr::FnCall { .. }))
        );

        // Two steps, descending sort, then take.
        let t3 = op("top3Orders").read_pipeline().expect("read op");
        assert_eq!(t3.steps.len(), 2);
        assert!(matches!(&t3.steps[0], Step::Sort { keys, .. } if keys[0].desc));
        assert!(matches!(
            t3.steps[1],
            Step::Take {
                n: TakeCount::Lit { n: 3, .. },
                ..
            }
        ));

        // Registration flag before `=`.
        assert!(op("giftOrders").flags.unbounded);
        assert!(!op("activeProducts").flags.unbounded);
    }

    #[test]
    fn case_pipelines_parse() {
        // C001 — projection subset over a composed source, then a sort.
        let c1 = parse_pipeline("activeProducts[name, price]\nsort price").unwrap();
        assert_eq!(c1.sources[0].name.name, "activeProducts");
        assert_eq!(c1.steps.len(), 1);

        // C007 — rename.
        let c7 = parse_pipeline("activeProducts[label: name, price]\nsort price").unwrap();
        let proj = c7.sources[0].projection.as_ref().unwrap();
        assert!(matches!(&proj[0], ProjItem::Named { out, .. } if out.name == "label"));

        // C009 — `[*]`.
        let c9 = parse_pipeline("top3Orders[*]").unwrap();
        assert!(matches!(
            c9.sources[0].projection.as_ref().unwrap()[0],
            ProjItem::Star { .. }
        ));

        // C010 — everything-minus.
        let c10 = parse_pipeline("spendByCustomer($from: 2026-01-01)[*, -n]\nsort -spent").unwrap();
        let proj = c10.sources[0].projection.as_ref().unwrap();
        assert!(matches!(&proj[1], ProjItem::Exclude { name, .. } if name.name == "n"));

        // C011 — two sources on one line.
        let c11 = parse_pipeline("products[name]  productRevenue[sold, revenue]").unwrap();
        assert_eq!(c11.sources.len(), 2);

        // C008 — computed projection.
        let c8 =
            parse_pipeline("cheapActive($cap: 10000)[name, sale: price - 500]\nsort sale").unwrap();
        let proj = c8.sources[0].projection.as_ref().unwrap();
        assert!(matches!(
            &proj[1],
            ProjItem::Named {
                value: Expr::Binary { .. },
                ..
            }
        ));
    }

    #[test]
    fn closed_type_set_is_enforced() {
        let e = parse_program("entity x { a: varchar }").unwrap_err();
        assert!(e.message.contains("unknown type"), "{}", e.message);
    }

    #[test]
    fn unknown_aggregate_is_rejected() {
        // `median` graduated to a real aggregate (C059); `stddev` stays out.
        let e = parse_program("op q =\n  orders[total]\n  group status { t: stddev total }")
            .unwrap_err();
        assert_eq!(e.code, "bare_field_in_group", "{}", e.message);
    }

    /// The aggregates of one group step, by `out: func` — enough to assert
    /// what a brace block produced without repeating the AST.
    fn aggs_of(src: &str) -> Vec<String> {
        let pl = parse_pipeline(src).expect("pipeline parses");
        let (Step::Group { aggs, .. } | Step::Reduce { aggs, .. }) = pl
            .steps
            .iter()
            .find(|s| matches!(s, Step::Group { .. } | Step::Reduce { .. }))
            .expect("a group step")
        else {
            unreachable!()
        };
        aggs.iter()
            .map(|a| {
                format!(
                    "{}: {}{}",
                    a.out.name,
                    a.func.name,
                    if a.arg.is_some() { " <arg>" } else { "" }
                )
            })
            .collect()
    }

    /// 9d. The one-per-line spelling is what every doc and both corpora write.
    /// It is not a variant of the comma form — it was a parse error.
    #[test]
    fn newline_separates_aggregate_entries() {
        let multi = aggs_of("orders[total]\ngroup tier {\n  n: count\n  rev: sum total\n}");
        assert_eq!(multi, ["n: count", "rev: sum <arg>"]);
        // Identical AST from the comma form: one construct, two layouts.
        assert_eq!(
            multi,
            aggs_of("orders[total]\ngroup tier { n: count, rev: sum total }")
        );

        // Trailing comma, trailing line break, and the two mixed.
        assert_eq!(
            aggs_of("orders[total]\ngroup tier { n: count, rev: sum total, }"),
            ["n: count", "rev: sum <arg>"]
        );
        assert_eq!(
            aggs_of(
                "orders[total]\ngroup tier {\n  n: count,\n  rev: sum total\n  m: max total\n}"
            ),
            ["n: count", "rev: sum <arg>", "m: max <arg>"]
        );
        // An empty block still parses as it always did.
        assert!(aggs_of("orders[total]\nreduce {  }").is_empty());
        // …and `reduce`, which shares the block parser.
        assert_eq!(
            aggs_of("orders[total]\nreduce {\n  n: count\n  rev: sum total\n}"),
            ["n: count", "rev: sum <arg>"]
        );
    }

    /// The separator is a line break, not the absence of one: two aggregates
    /// sharing a line with nothing between them must still fail, or
    /// `{ n: count rev: sum total }` would parse on a guess about where
    /// `sum`'s expression argument ends.
    #[test]
    fn a_missing_separator_is_still_an_error() {
        let e =
            parse_pipeline("orders[total]\ngroup tier { n: count rev: sum total }").unwrap_err();
        assert!(e.message.contains("`,` or a line break"), "{}", e.message);

        // The mirror case: an argument-taking aggregate whose argument went
        // missing must not swallow the next entry's name.
        let e =
            parse_pipeline("orders[total]\ngroup tier {\n  rev: sum\n  n: count\n}").unwrap_err();
        assert!(
            e.message
                .contains("`sum` needs its argument on the same line"),
            "{}",
            e.message
        );
    }

    /// 9f. The three settled aggregates the grammar was missing.
    #[test]
    fn percentile_distinct_and_joined_parse() {
        assert_eq!(
            aggs_of("orders[total]\ngroup tier { p95: percentile(total, 0.95) }"),
            ["p95: percentile <arg>"]
        );
        // Count-distinct — DL's `distinct field`, which is a different thing
        // from the bare `distinct` STEP that W039 composes with `reduce`.
        assert_eq!(
            aggs_of("sessions[customer]\ngroup day { dau: distinct customer }"),
            ["dau: distinct <arg>"]
        );
        assert_eq!(
            aggs_of("products[id]\ngroup products.id { tags: joined(tag.name, \", \") }"),
            ["tags: joined <arg>"]
        );
        // The bare `distinct` step is untouched by the aggregate spelling.
        let pl = parse_pipeline("taskTags[tag]\ndistinct\nreduce { n: count }").unwrap();
        assert!(matches!(pl.steps[0], Step::Distinct { .. }));
    }

    /// DL pins percentile's `p` as a literal strictly between 0 and 1, so a
    /// percentage — the natural thing to write — must be refused at the
    /// spelling, not silently reinterpreted.
    #[test]
    fn percentile_fraction_is_a_literal_in_the_open_unit_interval() {
        for bad in ["percentile(total, 95)", "percentile(total, 1.0)"] {
            let e = parse_pipeline(&format!("orders[total]\nreduce {{ p: {bad} }}")).unwrap_err();
            assert!(
                e.message.contains("strictly between 0 and 1"),
                "{bad}: {}",
                e.message
            );
        }
        for bad in ["percentile(total, $p)", "percentile(total, 0.0)"] {
            let e = parse_pipeline(&format!("orders[total]\nreduce {{ p: {bad} }}")).unwrap_err();
            assert!(
                e.message.contains("strictly between 0 and 1"),
                "{bad}: {}",
                e.message
            );
        }
        // And the separator of `joined` is punctuation, not data.
        let e = parse_pipeline("products[id]\nreduce { t: joined(name, sep) }").unwrap_err();
        assert!(e.message.contains("text literal"), "{}", e.message);
    }

    #[test]
    fn stage1_constructs_parse() {
        // reduce after distinct — ruling 1's counting spelling.
        let pl = parse_pipeline("taskTags[tag]\ndistinct\nreduce { n: count }").unwrap();
        assert!(matches!(pl.steps[1], Step::Reduce { .. }));

        // Conditional count.
        let pl = parse_pipeline(
            "orders[total, customer]\ngroup customer { big: count(total > 5000), n: count }",
        )
        .unwrap();
        let Step::Group { aggs, .. } = &pl.steps[0] else {
            panic!()
        };
        assert!(aggs[0].cond.is_some());
        assert!(aggs[1].cond.is_none());

        // let chain before the source line.
        let pl = parse_pipeline(
            "let cap = orders[total] reduce { m: max total }\n\ncheapActive($cap: cap.m)[name]",
        )
        .unwrap();
        assert_eq!(pl.lets.len(), 1);
        assert_eq!(pl.lets[0].name.name, "cap");
        assert!(matches!(pl.lets[0].pipeline.steps[0], Step::Reduce { .. }));

        // match any / none / ref / on.
        let pl =
            parse_pipeline("customers[name]\nmatch none doneOrders($from: 2026-01-01)").unwrap();
        assert!(matches!(
            &pl.steps[0],
            Step::Match {
                kind: MatchKind::None(_),
                ..
            }
        ));
        let pl = parse_pipeline("transfers[at]\nmatch transfers.fromAccount").unwrap();
        assert!(matches!(
            &pl.steps[0],
            Step::Match {
                kind: MatchKind::Ref { .. },
                ..
            }
        ));

        // append takes one source form.
        let pl =
            parse_pipeline("orders[placed]\nappend recentOrders[placed]\nsort placed").unwrap();
        assert!(matches!(pl.steps[0], Step::Append { .. }));
        assert!(matches!(pl.steps[1], Step::Sort { .. }));
    }

    #[test]
    fn keyless_group_is_spelled_reduce() {
        let e = parse_pipeline("orders[total]\ngroup { t: sum total }").unwrap_err();
        assert!(e.message.contains("reduce"), "{}", e.message);
    }

    /// `unique (a, b)` and `index (a, b)` — DL §3, "Uniqueness [settled]"
    /// and "Indexes [settled]".
    #[test]
    fn composite_unique_keys_and_indexes_parse() {
        let p = parse_program(
            "entity leaseParties {\n  \
               lease  -> leases    owned\n  \
               person -> people    owned\n  \
               role:   text = \"tenant\"\n  \
               unique (lease, person)\n\
             }\n\
             entity locations {\n  \
               property -> properties owned\n  \
               name: text\n  \
               kind: text\n  \
               index (property, kind)\n\
             }\n",
        )
        .expect("both entity-level clauses");
        let cols = |v: &Vec<Vec<Ident>>| -> Vec<Vec<String>> {
            v.iter()
                .map(|k| k.iter().map(|c| c.name.clone()).collect())
                .collect()
        };
        assert_eq!(
            cols(&p.entities[0].unique_keys),
            vec![vec!["lease".to_string(), "person".to_string()]]
        );
        assert!(p.entities[0].indexes.is_empty());
        assert_eq!(
            cols(&p.entities[1].indexes),
            vec![vec!["property".to_string(), "kind".to_string()]]
        );
        assert!(p.entities[1].unique_keys.is_empty());
    }

    /// `index` as a field modifier (reported fix 25w), beside every other
    /// modifier and the entity clause of the same word on the next line.
    #[test]
    fn a_field_modifier_index_does_not_swallow_the_next_lines_clause() {
        let p = parse_program(
            "entity tickets {\n  \
               status: text = \"open\" index\n  \
               owner -> members index\n  \
               due: date? index\n  \
               kind: text\n  \
               index (kind, due)\n\
             }\n",
        )
        .expect("both spellings of `index` in one shape");
        let e = &p.entities[0];
        let indexed: Vec<&str> = e
            .fields
            .iter()
            .filter(|f| f.index)
            .map(|f| f.name.name.as_str())
            .collect();
        assert_eq!(indexed, ["status", "owner", "due"]);
        assert_eq!(e.indexes.len(), 1, "the clause is still the entity's");
        assert_eq!(e.indexes[0][0].name, "kind");
        let status = &e.fields[0];
        assert!(status.default.is_some() && !status.optional && !status.indexed);
        assert!(e.fields[2].optional);
    }

    /// The one real hazard in the grammar: `unique` is ALSO a field
    /// modifier, and the lexer emits no `Newline` inside a brace (G6), so
    /// a modifier loop that stopped at "not a modifier" would eat the
    /// `unique` opening the NEXT line and then choke on its `(`. The
    /// parenthesis is what separates them — a field's `unique` never has
    /// one — and one token of lookahead is enough to see it.
    #[test]
    fn a_field_modifier_unique_does_not_swallow_the_next_lines_key() {
        let p = parse_program(
            "entity units {\n  \
               property -> properties owned\n  \
               code: text unique\n  \
               name: text\n  \
               unique (property, name)\n\
             }\n",
        )
        .expect("both spellings of `unique` in one shape");
        let e = &p.entities[0];
        assert!(
            e.fields
                .iter()
                .find(|f| f.name.name == "code")
                .unwrap()
                .unique,
            "the field modifier is still the field's"
        );
        assert!(
            !e.fields
                .iter()
                .find(|f| f.name.name == "name")
                .unwrap()
                .unique,
            "and it did not leak onto the field above the composite key"
        );
        assert_eq!(e.unique_keys.len(), 1);
    }

    /// One construct per concept: the single-column form is the field
    /// modifier, so `unique (a)` is refused at the spelling rather than
    /// becoming a second way to say it.
    #[test]
    fn a_one_column_unique_or_index_is_refused_at_the_spelling() {
        let e = parse_program("entity x { a: text\n  unique (a)\n}").unwrap_err();
        assert!(e.message.contains("`a: … unique`"), "{}", e.message);
        let e = parse_program("entity x { a: text\n  index (a)\n}").unwrap_err();
        assert!(e.message.contains("names one column"), "{}", e.message);
        assert!(e.message.contains("`a: … index`"), "{}", e.message);
        // And the form it names parses (reported fix 25w: it used to point
        // at a modifier that did not exist).
        let p = parse_program("entity x { a: text index\n}").expect("the field modifier");
        assert!(p.entities[0].fields[0].index);
        assert!(p.entities[0].indexes.is_empty());
        // A repeat inside one clause names a column twice, which claims
        // nothing extra and is more likely a typo than a request.
        let e = parse_program("entity x { a: text\n  b: text\n  unique (a, a)\n}").unwrap_err();
        assert!(e.message.contains("twice"), "{}", e.message);
    }

    /// `let x = set E { … }` — DL §3, "Naming a freshly created row in the
    /// same op" [settled]; R-G19. The spec's own example, verbatim.
    #[test]
    fn a_write_valued_let_binds_the_created_row() {
        let p = parse_program(
            "op startClub($name: text, $coHost: id) =\n  \
               let club = set clubs { name: $name }\n  \
               set clubsAccess { club: club.id, member: $coHost, level: \"admin\" }\n",
        )
        .expect("the settled create-and-seed shape");
        let OpBody::Write(w) = &p.ops[0].body else {
            panic!("a write group, not a read pipeline — the `let` did not decide it")
        };
        assert_eq!(w.len(), 2, "two statements in ONE group");
        let WriteStmt::Set { binding, .. } = &w[0] else {
            panic!("a set")
        };
        assert_eq!(binding.as_ref().map(|b| b.name.as_str()), Some("club"));
        let WriteStmt::Set {
            binding, fields, ..
        } = &w[1]
        else {
            panic!("a set")
        };
        assert!(binding.is_none(), "the dependent write names nothing");
        // `club.id` is an ordinary two-segment path; the write grammar
        // needed no new expression form for it.
        assert!(
            matches!(&fields[0].1, Expr::Path { segments, .. }
                     if segments.len() == 2 && segments[0].name == "club"),
            "{:?}",
            fields[0].1
        );
    }

    /// The lookahead's real job: `let` opens a read pipeline's CTE binding
    /// too, and only what follows the `=` tells them apart. Both must
    /// still work, in the same file.
    #[test]
    fn a_read_let_and_a_write_let_are_told_apart_by_what_follows_the_eq() {
        let p = parse_program(
            "op cheap =\n  let cap = products[price]\n  cap[price]\n\
             op make($n: text) =\n  let r = set rooms { name: $n }\n  \
               set logs { room: r.id }\n",
        )
        .expect("both spellings of `let`");
        assert!(
            matches!(&p.ops[0].body, OpBody::Read(pl) if pl.lets.len() == 1),
            "the read let is still a CTE binding"
        );
        assert!(matches!(&p.ops[1].body, OpBody::Write(w) if w.len() == 2));
    }

    /// A delete produces no row, so there is nothing to name — refused at
    /// the spelling rather than left to the mode derivation, because the
    /// answer does not depend on any mode.
    #[test]
    fn a_delete_cannot_be_bound() {
        let e = parse_program("op x($i: id) =\n  let d = del rooms { id: $i }\n").unwrap_err();
        assert!(e.message.contains("names nothing"), "{}", e.message);
        assert!(e.message.contains("only a create"), "{}", e.message);
    }

    #[test]
    fn owned_on_a_scalar_is_rejected() {
        let e = parse_program("entity x { a: text owned }").unwrap_err();
        assert!(e.message.contains("`owned` marks a ref"), "{}", e.message);
    }

    /// Rule 5 [settled]: **bare `del entity` does not parse.** The whole
    /// point is greppability — an unconditional delete has to be a
    /// deliberate act somebody wrote, never what a dropped condition
    /// quietly becomes — so the grammar, not a lint, is where it dies.
    #[test]
    fn bare_del_does_not_parse() {
        let e = parse_program("op wipe =\n  del tasks\n").unwrap_err();
        assert!(
            e.message.contains("bare `del <entity>` does not parse"),
            "{}",
            e.message
        );
        assert!(
            e.message.contains("del <entity>(true)"),
            "and the message hands over the spelling that IS legal: {}",
            e.message
        );
        // The same rejection when the statement merely ENDS there: a
        // trailing newline is not a condition either.
        assert!(parse_program("op wipe =\n  del tasks\n\nop other =\n  tasks[id]\n").is_err());
        // Delete-everything, stated, parses.
        let p = parse_program("op wipe =\n  del tasks(true)\n").expect("the stated form");
        let OpBody::Write(w) = &p.ops[0].body else {
            panic!("a write body")
        };
        assert!(matches!(w[0], WriteStmt::Del { subtree: false, .. }));
    }

    /// Rule 6 / §tree [settled]: the cascade is its own greppable word,
    /// and it hangs off one addressed anchor.
    #[test]
    fn subtree_is_an_explicit_word_on_an_addressed_delete() {
        let p = parse_program(
            "op dropIt($id: id) =\n  del projects { id: $id } subtree\n\
             \nop dropOne($id: id) =\n  del projects { id: $id }\n",
        )
        .expect("both delete forms");
        let del = |i: usize| -> (bool, Condition) {
            let OpBody::Write(w) = &p.ops[i].body else {
                panic!("a write body")
            };
            let WriteStmt::Del { subtree, cond, .. } = &w[0] else {
                panic!("a delete")
            };
            (*subtree, cond.clone())
        };
        let (subtree, cond) = del(0);
        assert!(subtree, "the word is carried, not inferred");
        let (plain, plain_cond) = del(1);
        assert!(!plain, "and absent when it was not written");
        // The braces ARE `id == $id` — one representation, so the
        // addressed form and `del E(id == $id)` cannot drift apart.
        for c in [&cond, &plain_cond] {
            let Condition::Compare {
                op: crate::ast::CmpOp::Eq,
                lhs: Expr::Path { segments, .. },
                rhs: Expr::Param { name, .. },
                ..
            } = c
            else {
                panic!("the braces desugar to an id comparison, got {c:?}")
            };
            assert_eq!(segments.len(), 1);
            assert_eq!(segments[0].name, "id");
            assert_eq!(name, "id");
        }

        // A ranged delete cannot cascade: many anchors mean overlapping
        // subtrees, which is the same hazard `tree_bulk_reparent` refuses.
        let e = parse_program("op dropIt($id: id) =\n  del projects(parent == $id) subtree\n")
            .unwrap_err();
        assert!(
            e.message.contains("`subtree` cascades from one anchor"),
            "{}",
            e.message
        );

        // A delete's braces address; they do not assign.
        let e = parse_program("op dropIt($n: text) =\n  del projects { name: $n }\n").unwrap_err();
        assert!(e.message.contains("address a row by `id`"), "{}", e.message);
        let e = parse_program("op dropIt($id: id) =\n  del projects { id: $id, name: \"x\" }\n")
            .unwrap_err();
        assert!(e.message.contains("`id` is the only key"), "{}", e.message);
    }

    /// 9r — a parse failure carries a CLASS, and the classes it may carry
    /// are the closed set the two conformance inventories already name.
    ///
    /// The boundary this pins: the parser owns the SHAPE of an `access`
    /// clause — which of the two forms it is, and whether a walk is a
    /// well-formed chain of atoms. What a well-formed shape is allowed to
    /// MEAN is R-G3's, and R-G3 violations are `invalid_guard_walk`
    /// wherever they are noticed.
    #[test]
    fn a_parse_failure_carries_one_of_the_inventory_classes() {
        let class = |src: &str| parse_program(src).unwrap_err().code;
        let ent = |body: &str| format!("entity e {{\n  p -> projects  owned\n{body}\n}}\n");

        // Shape: the clause is `open` or a walk, and neither `true` nor a
        // stray `open` is either (GC100, both halves).
        assert_eq!(class(&ent("  access write  true")), "parse_error");
        assert_eq!(
            class(&ent("  access read  p.owner == $caller or open")),
            "parse_error"
        );
        // Shape: the verb set is closed (GC079).
        assert_eq!(
            class(&ent("  access admin  p.owner == $caller")),
            "parse_error"
        );

        // Meaning: a readable atom saying something R-G3 does not admit
        // (GC077). `length(title)` reads as a collection step — only the
        // schema knows `length` is not an entity — but `> 3` is already
        // outside the guard grammar, and so is every other operator.
        for walk in [
            "p.owner == $caller or length(title) > 3",
            "p.owner == $caller or n < 3",
            "p.owner != $caller",
            "p.owner == $someone",
        ] {
            assert_eq!(
                class(&ent(&format!("  access read  {walk}"))),
                "invalid_guard_walk",
                "{walk}"
            );
        }

        // A legal walk still parses: the operator check must not fire on
        // the field declaration that follows the clause.
        parse_program(&ent(
            "  access read  p.owner == $caller\n  flag: bool = false",
        ))
        .expect("a legal guard clause parses");

        // And the third class, from the other inventory.
        assert_eq!(
            class("op q =\n  orders[total]\n  group status { t: stddev total }"),
            "bare_field_in_group"
        );
    }

    // ── 9e · parameter bounds and `take $n` ──────────────────────────────

    fn params_of(src: &str) -> Vec<ParamDecl> {
        parse_program(src).expect("the op parses").ops[0]
            .params
            .clone()
    }

    /// DL §2 Parameters: "Parameters are declared with type and bounds at
    /// operation registration."
    #[test]
    fn parameter_bounds_parse_and_are_carried() {
        let ps = params_of(
            "op createOrder($loc: id, $descr: text, $priority: int <= 5) =\n\
             \n  set workOrders { location: $loc, descr: $descr, priority: $priority }\n",
        );
        assert!(ps[0].max.is_none(), "`$loc: id` declares no bound");
        assert!(ps[1].max.is_none());
        assert!(
            matches!(ps[2].max, Some(Expr::IntLit { value: 5, .. })),
            "the bound is the literal the author wrote: {:?}",
            ps[2].max
        );

        // A bound and a default coexist, in that order.
        let ps = params_of("op q($n: int <= 5 = 2) =\n  orders[id]\n  take $n\n");
        assert!(matches!(ps[0].max, Some(Expr::IntLit { value: 5, .. })));
        assert!(matches!(ps[0].default, Some(Expr::IntLit { value: 2, .. })));

        // The other admitted types.
        let ps = params_of("op q($m: money <= 10000, $d: date <= 2026-12-31) =\n  orders[id]\n");
        assert!(matches!(ps[0].max, Some(Expr::IntLit { value: 10000, .. })));
        assert!(matches!(
            ps[1].max,
            Some(Expr::DateLit {
                y: 2026,
                m: 12,
                d: 31,
                ..
            })
        ));
    }

    /// One spelling per concept. `>=` and a range form are X-077's
    /// *proposed* fix, not a settled spelling, so they must not parse —
    /// and must not parse with a message that says so, since an author
    /// reaching for one is reaching for something the language declined.
    #[test]
    fn a_bound_is_upper_only_and_says_so() {
        let e = parse_program("op q($n: int >= 5) =\n  orders[id]\n").unwrap_err();
        assert_eq!(e.code, "parse_error");
        assert!(e.message.contains("no `>=`"), "{}", e.message);
    }

    /// A bound is proved at registration and enforced at the door, so it
    /// is a literal of the parameter's own type and nothing else.
    #[test]
    fn a_bound_is_a_literal_of_the_parameters_type() {
        for src in [
            "op q($n: int <= 5.5) =\n  orders[id]\n",
            "op q($n: int <= \"5\") =\n  orders[id]\n",
            "op q($n: int <= $m) =\n  orders[id]\n",
            "op q($d: date <= 5) =\n  orders[id]\n",
        ] {
            let e = parse_program(src).unwrap_err();
            assert_eq!(e.code, "parse_error", "{src}");
            assert!(e.message.contains("literal"), "{src}: {}", e.message);
        }
    }

    /// The types that admit no bound each refuse with their own reason,
    /// because "expected `)`" would send the author looking for a typo.
    #[test]
    fn the_types_that_take_no_bound_say_why() {
        let cases = [
            ("$s: text <= \"m\"", "its length"),
            ("$b: bool <= true", "no ordering"),
            ("$i: id <= \"x_1\"", "no ordering"),
            (
                "$t: datetime <= 2026-01-01T00:00:00Z",
                "not enforced at the door",
            ),
            ("$e: duration(ms) <= 60d", "convert units"),
        ];
        for (decl, needle) in cases {
            let e = parse_program(&format!("op q({decl}) =\n  orders[id]\n")).unwrap_err();
            assert_eq!(e.code, "parse_error", "{decl}");
            assert!(e.message.contains(needle), "{decl}: {}", e.message);
        }
    }

    /// DL §2 Types: "`text` (boundable `text(64)`)"; DL §2 Parameters:
    /// `$payee: text(64)`. The bound rides beside the type on the
    /// declaration, and the type stays `text`.
    #[test]
    fn text_bounds_parse_on_fields_and_parameters() {
        let p = parse_program(
            "entity notes { title: text(64)   body: text   tag: text(8)? sealed }\n\
             op add($title: text(64), $body: text, $n: int <= 5) =\n\
             \n  set notes { title: $title, body: $body, tag: null }\n",
        )
        .expect("parses");
        let f = &p.entities[0].fields;
        assert_eq!(f[0].text_max, Some(64));
        assert!(matches!(f[0].kind, FieldKind::Scalar(ScalarTy::Text)));
        assert_eq!(f[1].text_max, None);
        assert_eq!(f[2].text_max, Some(8));
        assert!(
            f[2].optional && f[2].sealed,
            "the bound composes with `?` and modifiers"
        );
        let ps = &p.ops[0].params;
        assert_eq!(ps[0].text_max, Some(64));
        assert_eq!(ps[0].ty, ScalarTy::Text);
        assert!(ps[0].max.is_none(), "a text bound is not a `<=` bound");
        assert_eq!(ps[1].text_max, None);
        assert_eq!(ps[2].text_max, None);
    }

    /// The bound counts characters, so it is a positive integer literal
    /// and nothing else; and only `text` (and `duration`, for its unit)
    /// takes a parenthesis at all.
    #[test]
    fn a_text_bound_is_a_positive_integer_and_other_types_take_none() {
        for (src, needle) in [
            ("entity x { a: text(0) }", "positive integer"),
            ("entity x { a: text(\"64\") }", "positive integer"),
            ("entity x { a: text($n) }", "positive integer"),
            ("entity x { a: text(64 }", "`)` to close the text bound"),
            ("entity x { a: int(64) }", "takes no parenthesis"),
            ("entity x { a: bool(2) }", "takes no parenthesis"),
            (
                "op q($d: date(1)) =\n  orders[id]\n",
                "takes no parenthesis",
            ),
        ] {
            let e = parse_program(src).unwrap_err();
            assert_eq!(e.code, "parse_error", "{src}");
            assert!(e.message.contains(needle), "{src}: {}", e.message);
        }
    }

    /// Ruled 2026-10-04 (27ak): each money field declares its currency's
    /// places, `money(2)` or `money(0)`. Bare `money` is `money(0)`, and
    /// the places ride on parameters and through bounds the same way.
    #[test]
    fn money_declares_its_places_on_fields_and_parameters() {
        let p = parse_program(
            "entity prices { usd: money(2)   yen: money(0)   old: money   fine: money(4)? }\n\
             op cap($m: money(2) <= 100.00, $y: money) =\n  prices[usd]\n",
        )
        .expect("parses");
        let tys: Vec<ScalarTy> = p.entities[0]
            .fields
            .iter()
            .map(|f| match f.kind {
                FieldKind::Scalar(t) => t,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            tys,
            [
                ScalarTy::Money(2),
                ScalarTy::Money(0),
                ScalarTy::Money(0),
                ScalarTy::Money(4)
            ]
        );
        assert!(p.entities[0].fields[3].optional);
        let ps = &p.ops[0].params;
        assert_eq!(ps[0].ty, ScalarTy::Money(2));
        assert!(ps[0].max.is_some(), "the bound follows the places");
        assert_eq!(ps[1].ty, ScalarTy::Money(0));
        assert_eq!(ScalarTy::Money(0).spelled(), "money");
        assert_eq!(ScalarTy::Money(2).spelled(), "money(2)");
    }

    /// `decimal(p)` is storable since 27ak, for rates and unit prices, and
    /// its places are required: a rate's precision is the author's choice.
    #[test]
    fn decimal_declares_its_places() {
        let p = parse_program(
            "entity rates { r: decimal(4)   fx: decimal(12)? }\n\
             op cap($r: decimal(4) <= 1.5) =\n  rates[r]\n",
        )
        .expect("parses");
        assert!(matches!(
            p.entities[0].fields[0].kind,
            FieldKind::Scalar(ScalarTy::Decimal(4))
        ));
        assert_eq!(p.ops[0].params[0].ty, ScalarTy::Decimal(4));
        assert_eq!(ScalarTy::Decimal(4).spelled(), "decimal(4)");
        let e = parse_program("entity x { a: decimal }").unwrap_err();
        assert!(e.message.contains("declares its places"), "{}", e.message);
        let e = parse_program("entity x { a: decimal(13) }").unwrap_err();
        assert!(e.message.contains("from 0 to 12"), "{}", e.message);
    }

    /// The places count digits after the point, from 0 to the division
    /// scale of 12, and nothing else.
    #[test]
    fn money_places_are_a_small_integer() {
        for (src, needle) in [
            ("entity x { a: money(13) }", "from 0 to 12"),
            ("entity x { a: money(-1) }", "from 0 to 12"),
            ("entity x { a: money(usd) }", "from 0 to 12"),
            ("entity x { a: money(2 }", "`)` to close the places"),
        ] {
            let e = parse_program(src).unwrap_err();
            assert_eq!(e.code, "parse_error", "{src}");
            assert!(e.message.contains(needle), "{src}: {}", e.message);
        }
    }

    /// DL §2 Parameters, "`filled` — the SDK-supplied marker [settled]":
    /// `op thirsty($today: date filled) = …`. It follows the type and the
    /// bound, it is not a default, and it cannot be paired with one.
    #[test]
    fn filled_parses_after_the_type_and_refuses_a_default() {
        let ps =
            params_of("op thirsty($today: date filled, $n: int <= 5 filled) =\n  orders[id]\n");
        assert!(ps[0].filled);
        assert!(ps[0].default.is_none(), "`filled` is not a default");
        assert_eq!(ps[0].ty, ScalarTy::Date);
        assert!(
            ps[1].filled && ps[1].max.is_some(),
            "a bound and `filled` compose"
        );

        let e =
            parse_program("op q($today: date filled = 2026-01-01) =\n  orders[id]\n").unwrap_err();
        assert_eq!(e.code, "parse_error");
        assert!(e.message.contains("has a default"), "{}", e.message);

        // `filled` is a reserved word: it cannot name a field.
        let e = parse_program("entity x { filled: bool }").unwrap_err();
        assert_eq!(e.code, "parse_error");
    }

    /// DL "Other steps": "`take $n` — legal: the count may be a
    /// parameter". Whether the parameter carries a bound is the
    /// registration gate's question (`unbounded_take`) — it needs the op's
    /// parameter list, which a step parser does not have.
    #[test]
    fn take_accepts_a_parameter_count() {
        let p = parse_program("op q($n: int <= 20) =\n  orders[id]\n  sort id\n  take $n\n")
            .expect("`take $n` parses");
        let Some(pl) = p.ops[0].read_pipeline() else {
            panic!("a read op")
        };
        let Step::Take { keys, n, .. } = &pl.steps[1] else {
            panic!("a take step, got {:?}", pl.steps[1])
        };
        assert!(keys.is_empty());
        assert!(
            matches!(n, TakeCount::Param { name } if name.name == "n"),
            "{n:?}"
        );

        // The keyed spelling takes one too — brackets choose the buckets,
        // not the count's shape.
        let p = parse_program(
            "op q($n: int <= 20) =\n  orders[id, customer]\n  sort id\n  take[customer] $n\n",
        )
        .expect("`take[keys] $n` parses");
        let Some(pl) = p.ops[0].read_pipeline() else {
            panic!("a read op")
        };
        let Step::Take { keys, n, .. } = &pl.steps[1] else {
            panic!("a take step")
        };
        assert_eq!(keys.len(), 1);
        assert!(matches!(n, TakeCount::Param { .. }));

        // And the literal form still says what it always said.
        let e = parse_program("op q =\n  orders[id]\n  take total\n").unwrap_err();
        assert!(
            e.message.contains("row count or a `$parameter`"),
            "{}",
            e.message
        );
    }

    // ── 9e · parenthesized guard groups (R-G3) ───────────────────────────

    /// Render a parsed read guard as `disjunct | disjunct`, atoms joined by
    /// `&` — enough to assert the boolean structure without asserting the
    /// whole tree.
    fn read_guard_shape(src: &str) -> String {
        let p = parse_program(src).expect("the entity parses");
        let Some(GuardClause::Walk(w)) = &p.entities[0].access_read else {
            panic!("a walk guard")
        };
        w.disjuncts
            .iter()
            .map(|conj| {
                conj.iter()
                    .map(|a| match a {
                        GuardAtom::Cmp {
                            path, caller: true, ..
                        } => format!("{}==$caller", render_walk(path)),
                        GuardAtom::Cmp {
                            path,
                            lit: Some(Expr::StrLit { value, .. }),
                            ..
                        } => format!("{}==\"{value}\"", render_walk(path)),
                        GuardAtom::Cmp { path, .. } => format!("{}==?", render_walk(path)),
                        GuardAtom::Bare { path, .. } => render_walk(path),
                        GuardAtom::Step { path, inner, .. } => {
                            let sub = GuardExpr {
                                disjuncts: inner.disjuncts.clone(),
                                span: inner.span,
                            };
                            format!("{}({})", render_walk(path), shape_of(&sub))
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" & ")
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn shape_of(w: &GuardExpr) -> String {
        w.disjuncts
            .iter()
            .map(|conj| {
                conj.iter()
                    .map(|a| match a {
                        GuardAtom::Cmp {
                            path, caller: true, ..
                        } => format!("{}==$caller", render_walk(path)),
                        GuardAtom::Cmp {
                            path,
                            lit: Some(Expr::StrLit { value, .. }),
                            ..
                        } => format!("{}==\"{value}\"", render_walk(path)),
                        GuardAtom::Cmp { path, .. } => format!("{}==?", render_walk(path)),
                        GuardAtom::Bare { path, .. } => render_walk(path),
                        GuardAtom::Step { path, .. } => format!("{}(…)", render_walk(path)),
                    })
                    .collect::<Vec<_>>()
                    .join(" & ")
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// R-G3 admits parens when mixing `and`/`or`. They are DISTRIBUTED, so
    /// the tree stays disjunctive normal form and "a disjunct" keeps
    /// meaning "one independent authorization path" for every rule stated
    /// over that notion (G2, create-exemption, the semi-join union).
    #[test]
    fn parenthesized_guard_groups_normalize_to_dnf() {
        let ent = |g: &str| {
            format!(
                "entity docs {{\n  owner -> members\n  a: bool\n  b: bool\n\
                 \n  access read  {g}\n  access write  open\n}}\n"
            )
        };

        // The task's example: a group on the LEFT of an `and`.
        assert_eq!(
            read_guard_shape(&ent("(a or b) and owner == $caller")),
            "a & owner==$caller | b & owner==$caller"
        );
        // …and on the right, which must distribute the same way.
        assert_eq!(
            read_guard_shape(&ent("owner == $caller and (a or b)")),
            "owner==$caller & a | owner==$caller & b"
        );
        // Two groups multiply.
        assert_eq!(
            read_guard_shape(&ent("(a or b) and (owner == $caller or a)")),
            "a & owner==$caller | a & a | b & owner==$caller | b & a"
        );
        // A group that does not mix is still a group, and collapses.
        assert_eq!(
            read_guard_shape(&ent("(owner == $caller)")),
            "owner==$caller"
        );
        // An `or` of groups concatenates rather than multiplying.
        assert_eq!(
            read_guard_shape(&ent("(owner == $caller and a) or (b and a)")),
            "owner==$caller & a | b & a"
        );
    }

    /// The canonical multi-level ACL idiom (`syntax-examples.md` §6): the
    /// group lives INSIDE a reverse collection step, and normalizes there.
    #[test]
    fn the_multi_level_acl_idiom_parses() {
        let src = "entity units {\n  property -> properties  owned\n\
                   \n  access read   property.propertiesAccess(member == $caller)\n\
                   \n  access write  property.propertiesAccess(\n\
                   \n                  member == $caller and (level == \"write\" or level == \"admin\"))\n}\n";
        let p = parse_program(src).expect("the idiom parses");
        let Some(GuardClause::Walk(w)) = &p.entities[0].access_write else {
            panic!("a walk guard")
        };
        assert_eq!(w.disjuncts.len(), 1, "one outer path: the step");
        let [GuardAtom::Step { inner, .. }] = &w.disjuncts[0][..] else {
            panic!("a reverse step")
        };
        assert_eq!(
            shape_of(inner),
            "member==$caller & level==\"write\" | member==$caller & level==\"admin\"",
            "the group distributes inside the step, and `member == $caller` \
             survives into BOTH paths — dropping it from one is the whole hazard"
        );
    }

    /// DL §3 Groups: `member.groupMembers.group(member == $caller)` names
    /// the arriving ref, and that ref is called `group`. A reserved word
    /// after a dot is a segment — in a guard walk, a projection, a `match`
    /// ref path and an expression path alike — because nothing but a
    /// segment can follow a dot. This used to be "expected a walk segment,
    /// found group", which made DL's own settled spelling unwritable.
    #[test]
    fn a_reserved_word_after_a_dot_is_a_segment() {
        let src = "entity projectMembers {\n  project -> projects owned\n  member -> members owned\n\
                   \n  access read   project.projectMembers(member == $caller)\n\
                   \n                or project.projectMembers(member.groupMembers.group(member == $caller))\n\
                   \n  access write  member == $caller\n}\n\
                   op viaGroup unbounded =\n  projectMembers[] groupMembers[member, group.displayName]\n\
                   \n  match groupMembers.group\n  sort member.displayName\n";
        let p = parse_program(src).expect("parses");
        let Some(GuardClause::Walk(w)) = &p.entities[0].access_read else {
            panic!("a walk guard")
        };
        assert_eq!(w.disjuncts.len(), 2);
        let [GuardAtom::Step { path, inner, .. }] = &w.disjuncts[1][..] else {
            panic!("a reverse step")
        };
        assert_eq!(render_walk(path), "project.projectMembers");
        assert_eq!(shape_of(inner), "member.groupMembers.group(…)");

        let pl = p.ops[0].read_pipeline().expect("a read");
        let proj = pl.sources[1].projection.as_ref().expect("projected");
        assert!(
            matches!(&proj[1], ProjItem::Field { path, .. } if path[0].name == "group" && path[1].name == "displayName"),
            "a keyword heads a projection path when a dot follows it"
        );
        assert!(matches!(
            &pl.steps[0],
            Step::Match { kind: MatchKind::Ref { path }, .. } if path[1].name == "group"
        ));

        // The head of a path is still an identifier position: a keyword
        // there is the keyword.
        let e = parse_program("op q =\n  orders[group]\n").unwrap_err();
        assert_eq!(e.code, "parse_error");
        let e = parse_program("entity x { group: text }").unwrap_err();
        assert_eq!(e.code, "parse_error", "`group` still cannot be declared");
    }

    /// DL §3, "Sealed fields and derived std ops — `std sealed`
    /// [settled]": one shape line, once; `std` stays a usable field name.
    #[test]
    fn std_sealed_is_one_shape_line() {
        let p = parse_program(
            "entity patients {\n  name: text sealed\n  admitted: date\n  std sealed\n}\n\
             entity plain { std: text\n n: int }\n",
        )
        .expect("parses");
        assert!(p.entities[0].std_sealed);
        assert_eq!(p.entities[0].fields.len(), 2, "the line is not a field");
        assert!(!p.entities[1].std_sealed);
        assert_eq!(
            p.entities[1].fields[0].name.name, "std",
            "a field may be named `std`"
        );

        let e = parse_program("entity x { a: text sealed\n std sealed\n std sealed }").unwrap_err();
        assert!(
            e.message.contains("duplicate `std sealed`"),
            "{}",
            e.message
        );
        let e = parse_program("entity x { a: text\n std open }").unwrap_err();
        assert_eq!(
            e.code, "parse_error",
            "`std` followed by anything else is a field"
        );
    }

    /// Principle 2 has no exception for guards: mixing without parens is a
    /// parse error, not a precedence table. (It used to be read silently as
    /// `(a and b) or c`.)
    #[test]
    fn mixing_guard_connectives_without_parens_is_a_parse_error() {
        let ent = |g: &str| {
            format!(
                "entity docs {{\n  owner -> members\n  a: bool\n  b: bool\n\
                 \n  access read  {g}\n  access write  open\n}}\n"
            )
        };
        for g in [
            "a and b or owner == $caller",
            "owner == $caller or a and b",
            "docShares(a and b or member == $caller)",
        ] {
            let e = parse_program(&ent(g)).unwrap_err();
            assert_eq!(e.code, "parse_error", "{g}");
            assert!(e.message.contains("without parentheses"), "{g}: {e}");
        }
    }

    /// A group is an operand of `and`/`or` and of nothing else — R-G3 has
    /// no operator that takes one. Readable, and outside the grammar, so
    /// it is the walk's class (GC077's boundary).
    #[test]
    fn an_operator_after_a_guard_group_is_the_walks_class() {
        let e = parse_program(
            "entity docs {\n  owner -> members\n  a: bool\n\
             \n  access read  (a or owner == $caller) > 3\n  access write  open\n}\n",
        )
        .unwrap_err();
        assert_eq!(e.code, "invalid_guard_walk");
        assert!(e.message.contains("parenthesized group"), "{e}");
    }

    /// Distribution is a product, so the parser caps it rather than
    /// allocating `2^n` conjuncts for a guard nobody could review.
    #[test]
    fn a_guard_that_normalizes_too_wide_is_refused() {
        let group = "(a or b)";
        let mut g = String::from("owner == $caller");
        for _ in 0..7 {
            g.push_str(" and ");
            g.push_str(group);
        }
        let e = parse_program(&format!(
            "entity docs {{\n  owner -> members\n  a: bool\n  b: bool\n\
             \n  access read  {g}\n  access write  open\n}}\n"
        ))
        .unwrap_err();
        assert_eq!(e.code, "parse_error");
        assert!(e.message.contains("authorization paths"), "{e}");

        // Six groups is 64 and still legal: the cap admits its own edge.
        let mut ok = String::from("owner == $caller");
        for _ in 0..6 {
            ok.push_str(" and ");
            ok.push_str(group);
        }
        let p = parse_program(&format!(
            "entity docs {{\n  owner -> members\n  a: bool\n  b: bool\n\
             \n  access read  {ok}\n  access write  open\n}}\n"
        ))
        .expect("64 paths parse");
        let Some(GuardClause::Walk(w)) = &p.entities[0].access_read else {
            panic!("a walk")
        };
        assert_eq!(w.disjuncts.len(), 64);
    }
}

#[cfg(test)]
mod unbuilt_tests {
    use super::*;

    /// A reserved-and-unbuilt declaration says which state it is in, rather
    /// than listing the three declarations that do parse.
    ///
    /// The whole point of the finding this closes (`critique-consolidated-5.md`
    /// W-001): the tutorial teaches `routine` across a page, and the old
    /// message read as a typo to somebody who had typed exactly what the
    /// document showed them.
    #[test]
    fn a_reserved_but_unbuilt_declaration_says_so() {
        for (word, _) in UNBUILT_DECLARATIONS {
            let err = parse_program(word).expect_err(word);
            assert!(
                err.message.contains("reserved and not implemented"),
                "`{word}` answered: {}",
                err.message
            );
            assert!(
                !err.message.contains("expected a declaration"),
                "`{word}` still lists the three that parse: {}",
                err.message
            );
        }
    }

    /// The table has not quietly grown to cover a word the parser DOES
    /// implement, which would make a working declaration unusable.
    #[test]
    fn nothing_built_is_in_the_unbuilt_table() {
        for built in ["domain", "entity", "op", "routine", "trigger"] {
            assert!(
                !UNBUILT_DECLARATIONS.iter().any(|(k, _)| *k == built),
                "`{built}` parses and must not be refused as unbuilt"
            );
        }
    }
}

#[cfg(test)]
mod routine_tests {
    use super::*;
    use crate::trigger::{EventKind, KindPos, Pos, Schedule};

    fn one(src: &str) -> RoutineDecl {
        let p = parse_program(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
        assert_eq!(p.routines.len(), 1, "{src}");
        p.routines[0].clone()
    }

    /// The docs rerun found `trigger every day 09:00Z` refused at the `Z`.
    #[test]
    fn the_09_00z_form_parses() {
        let r = one("routine morning in notes {\n  trigger every day 09:00Z\n  \
             run($today: date):\n    set notes(day < $today) { stale: true }\n}\n");
        assert_eq!(r.name.name, "morning");
        assert_eq!(r.domain.as_ref().map(|d| d.name.as_str()), Some("notes"));
        assert_eq!(
            r.trigger,
            RoutineTrigger::Clock(Schedule::Daily { hour: 9, minute: 0 })
        );
        assert_eq!(r.params.len(), 1);
        assert_eq!(r.body.len(), 1);
    }

    #[test]
    fn a_notify_line_between_the_trigger_and_run_parses() {
        let r = one("routine remind in notes {\n  trigger every 30s\n  \
             notify admin \"A reminder is due\"\n  run($now: datetime):\n    \
             set reminders(due <= $now) { fired: $now }\n}\n");
        assert_eq!(
            r.trigger,
            RoutineTrigger::Clock(Schedule::Every { seconds: 30 })
        );
        let n = r.notify.expect("a notify line");
        assert_eq!(n.level.name, "admin");
        assert_eq!(n.text, "A reminder is due");
        assert_eq!(n.span.line, 3);
        assert!(one(
            "routine sweep {\n  trigger every 5m\n  run:\n    del drafts(done == true)\n}\n"
        )
        .notify
        .is_none());
        let err = parse_program(
            "routine remind {\n  trigger every 30s\n  notify admin\n  run:\n    del drafts(true)\n}\n",
        )
        .expect_err("no text");
        assert!(err.message.contains("quoted text"), "{}", err.message);
    }

    #[test]
    fn a_schedule_time_without_the_z_is_refused_by_name() {
        let err = parse_program(
            "routine morning {\n  trigger every day 09:00\n  run:\n    set notes(true) { a: 1 }\n}\n",
        )
        .expect_err("no Z");
        assert!(err.message.contains("`Z` suffix"), "{}", err.message);
        assert_eq!(err.span.line, 2, "the error points at the clause: {err:?}");
    }

    #[test]
    fn an_every_n_minutes_routine_parses() {
        let r =
            one("routine sweep {\n  trigger every 5m\n  run:\n    del drafts(done == true)\n}\n");
        assert_eq!(
            r.trigger,
            RoutineTrigger::Clock(Schedule::Every { seconds: 300 })
        );
    }

    #[test]
    fn a_change_routine_parses_with_its_modifiers_and_flags() {
        let r = one(
            "routine stamp {\n  trigger when notes.tasks.created debounce 60s coalesce\n  \
             run unguarded nopause:\n    set tasks(seen == false) { seen: true }\n}\n",
        );
        let RoutineTrigger::Change { pattern, modifiers } = &r.trigger else {
            panic!("a change trigger")
        };
        assert_eq!(pattern.domain, Pos::Named("notes".into()));
        assert_eq!(pattern.entity, Pos::Named("tasks".into()));
        assert_eq!(pattern.kind, KindPos::Kind(EventKind::Created));
        assert_eq!(modifiers.debounce, Some(60));
        assert!(modifiers.coalesce);
        assert!(r.flags.unguarded && r.flags.nopause);
    }

    #[test]
    fn an_unknown_event_kind_is_refused_with_its_class() {
        let err = parse_program(
            "routine stamp {\n  trigger when notes.tasks.updated\n  run:\n    set tasks(true) { a: 1 }\n}\n",
        )
        .expect_err("updated is not a kind");
        assert_eq!(err.code, crate::trigger::UNKNOWN_EVENT_KIND);
    }

    #[test]
    fn a_top_level_when_line_points_at_the_routine_form() {
        let err = parse_program("when notes.tasks.created -> run x\n").expect_err("top level");
        assert!(err.message.contains("inside a routine"), "{}", err.message);
    }

    #[test]
    fn a_routine_with_no_writes_says_what_its_body_is() {
        let err = parse_program("routine r {\n  trigger every 5m\n  run:\n    tasks[id]\n}\n")
            .expect_err("a read body");
        assert!(err.message.contains("each ->"), "{}", err.message);
    }

    #[test]
    fn an_each_body_reads_a_pipeline_and_carries_its_writes() {
        let r = one(
            "routine leaseExpiry in estate {\n  trigger every day 08:00Z\n  run($today: date):\n    \
             leases(status == \"active\" and end is not null and end <= $today + 60d)\n      \
             [id, end, unit.name]\n    each ->\n      \
             set alerts { key: \"leaseExpiry:\" + id, title: unit.name + \" lease ends \" + end }\n}\n",
        );
        let pl = r.each.as_ref().expect("a pipeline");
        assert_eq!(pl.sources.len(), 1);
        assert_eq!(pl.sources[0].name.name, "leases");
        assert_eq!(r.body.len(), 1);
    }

    #[test]
    fn an_each_with_steps_and_two_writes_parses() {
        let r = one(
            "routine thirstCheck {\n  trigger every day 09:00Z\n  run($today: date):\n    \
             plants[id, name, everyDays]  waterings[at]\n    sort -waterings.at\n    \
             take[plants.id] 1\n    | (at <= $today - everyDays) or at is null\n    each ->\n      \
             set alerts { key: \"thirsty:\" + plants.id, title: name }\n      \
             del drafts { id: id }\n}\n",
        );
        let pl = r.each.as_ref().expect("a pipeline");
        assert_eq!(pl.sources.len(), 2);
        assert_eq!(pl.steps.len(), 3);
        assert_eq!(r.body.len(), 2);
    }

    #[test]
    fn a_write_body_has_no_pipeline() {
        let r =
            one("routine sweep {\n  trigger every 5m\n  run:\n    del drafts(done == true)\n}\n");
        assert!(r.each.is_none());
    }

    #[test]
    fn an_each_with_no_writes_is_refused() {
        let err = parse_program(
            "routine r {\n  trigger every 5m\n  run:\n    tasks[id]\n    each ->\n}\n",
        )
        .expect_err("no writes");
        assert!(
            err.message.contains("writes to run for each row"),
            "{}",
            err.message
        );
    }
}
