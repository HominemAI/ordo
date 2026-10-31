//! Hand-written recursive-descent parser for the expression and condition
//! grammar (spec §3–§4). Pipelines and entity declarations come later and
//! will reuse the same token-slice internals (`Parser`).
//!
//! Boolean structure follows spec §1.2: same-connective chains need no
//! parens (`a and b and c`); mixing `and`/`or` without parens is a parse
//! error. Arithmetic keeps conventional precedence (`* /` over `+ -`) —
//! only BOOLEAN mixing is parenthesized. Comparisons are non-associative.

use crate::ast::{BinOp, CmpOp, CondArm, Condition, Expr, Ident, TextOp};
use crate::lexer::{lex, LexError};
use crate::span::Span;
use crate::token::{Keyword, Token, TokenKind};

/// The closed set of error classes a *parse* failure may carry.
///
/// Classes — not messages — are the compiler's contract with a test:
/// `composition-conformance.md` §0 and `guards-conformance.md` §0 both say
/// so in as many words ("the harness asserts the class, not the message
/// text"). A `ParseError` with no class forced every reject case to match
/// message TEXT, so a reworded message silently broke an assertion and a
/// wrong-but-similar message kept one green.
///
/// Both classes below are already in those closed inventories; nothing here
/// is invented. Which one a failure carries is decided by *what* is wrong,
/// never by which pass noticed:
///
/// - [`PARSE_ERROR`] — "text outside the closed grammar". The parser could
///   not read the input at all, so it can say what it expected and nothing
///   about what the author meant.
/// - [`INVALID_GUARD_WALK`] — "a guard predicate falls outside the closed
///   guard grammar (R-G3)". The parser DID read a well-formed guard atom;
///   what the atom says is what R-G3 does not admit, so the walk can be
///   named. Most of these are raised by `ordo-schema` once the walk is
///   resolved against the graph; the ones the grammar itself can decide
///   (an operator that is not `==`, a `$param` other than `$caller`) are
///   raised here, under the same class, because the class describes the
///   defect and not the crate that found it.
/// - [`BARE_FIELD_IN_GROUP`] — "per-row field inside `{ }` after
///   group/reduce". The brace block parses; what it names is not an
///   aggregate, and only the grammar knows the closed aggregate set.
pub const PARSE_ERROR: &str = "parse_error";
/// See [`PARSE_ERROR`]. `guards-conformance.md` §0, R-G3.
pub const INVALID_GUARD_WALK: &str = "invalid_guard_walk";
/// See [`PARSE_ERROR`]. `composition-conformance.md` §0.
pub const BARE_FIELD_IN_GROUP: &str = "bare_field_in_group";
/// A second `release member` line. One rule per program (27aq).
pub const DUPLICATE_RELEASE: &str = "duplicate_release";
/// A `secret` bound to something other than an exact host name: a URL, a
/// port or a wildcard (`plan-backend-functions.md` §5).
pub const INVALID_SECRET_HOST: &str = "invalid_secret_host";
/// `$caller` inside a release rule. The seat being released is `$member`,
/// and the rule is about that seat whoever asks (27aq).
pub const RELEASE_SUBJECT: &str = "release_subject";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// One of the closed classes above. `&'static str` and not an enum for
    /// the same reason `SemaError::code` and `SchemaError::code` are: the
    /// three travel together through one harness, and one shape means one
    /// `class_of`.
    pub code: &'static str,
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {} at {}", self.code, self.message, self.span)
    }
}

impl std::error::Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        // A lexer failure is by construction "text outside the closed
        // grammar": it never got as far as a construct to misjudge.
        ParseError {
            code: PARSE_ERROR,
            message: e.message,
            span: e.span,
        }
    }
}

/// **`allocate`'s mode names, in the order the specification lists them** —
/// the closed set of answers to *which party absorbs the odd minor unit*.
///
/// The set lives here, in the crate that owns the surface spelling, and
/// `ordo_plan::AllocateMode` is checked against it in both directions by a
/// test in that crate. Two lists that can disagree about money would
/// disagree eventually.
///
/// It is a **closed set of bare words**, the same shape as `sort … desc`,
/// and the closure is the point: the third argument position of `allocate`
/// is a mode and nothing else, so a name that is not a mode is a parse
/// error and can never be read as a field, a fall back to the default, or
/// anything else that would move a penny quietly.
///
/// **They are lowerCamel because every name in this language is.** DL §1
/// *Identifiers* is `[settled]` and says "one rule for every declared name
/// … `[a-z][A-Za-z0-9]*` — lowerCamel, no underscores"; the lexer enforces
/// it, so `largest_remainder` is not a spelling this language has. The
/// literature's snake_case is a convention from elsewhere, and importing it
/// for four words would put an underscore in the grammar to save a
/// capital.
///
/// **`rotation` is not here and will not be.** Spreading the odd penny
/// across successive invoices needs to remember what it did last time; it is
/// not a function of `(total, weights)`, and an expression function that
/// answers differently on a retry breaks the determinism the language and
/// the write protocol both rest on. `definition-language.md` §3 carries the
/// ruling.
pub const ALLOCATE_MODES: &[&str] = &["largestRemainder", "first", "last", "largestShare"];

/// The closed expression-function set (spec §3) with its fixed arities —
/// checked at parse time. Aggregates (`count sum avg …`) are deliberately
/// absent: they are legal only inside `group`/`reduce` braces, which the
/// pipeline parser owns.
const FUNCTIONS: &[(&str, usize, usize)] = &[
    ("year", 1, 1),
    ("month", 1, 1),
    ("week", 1, 1),
    ("day", 1, 1),
    ("hour", 1, 1),
    ("dow", 1, 1),
    ("abs", 1, 1),
    ("round", 1, 2),
    ("floor", 1, 1),
    ("ceil", 1, 1),
    ("mod", 2, 2),
    ("least", 2, 2),
    ("greatest", 2, 2),
    // `hash(x)` is back, because G12 is ruled (2026-08-30,
    // `corpus-rulings.md`). It was removed by 9f″ for a good reason and the
    // reason is now gone: the algorithm was unruled while DL §3 promised
    // bit-stability across platform versions, and every hash Postgres offers
    // in core for the purpose (`hashtext`, `hashint8`) is explicitly NOT
    // stable across server versions — so a lowering would have made the
    // promise and broken it in the same commit. The ruling picks SHA-256,
    // which is in core, needs no extension, and is stable because it is not
    // ours to change. The truncation, the sign, and the encoding are pinned
    // beside the lowering in `ordo_plan::hash64`.
    //
    // Its argument is text or an id. That is not enforced here: the language
    // type-checks no expression function's arguments (`lower(price)` is
    // equally unchecked), and no closed inventory carries a class for
    // "argument of the wrong type to an expression function". What catches
    // it is F6's shape verification at registration, exactly as it catches
    // `lower(price)` — the pre-existing gap is recorded in the checklist
    // rather than papered over with an invented class here.
    ("hash", 1, 1),
    //
    // The blessed money-split primitive (DL §3, Allocation). It parses
    // here so that `allocate(…)` is diagnosed by the binder as
    // `invalid_allocate` — a class the closed inventory names — rather
    // than as an unknown function; where it is LEGAL is a question about
    // the surrounding statement, which a closed-arity table cannot ask.
    //
    // The third argument is the OPTIONAL mode — which party absorbs the odd
    // minor unit. It is a business rule, not a mathematical one, so the
    // caller names it; omitted, it is `largestRemainder`, the value four
    // documents print. See `ALLOCATE_MODES`.
    ("allocate", 2, 3),
    ("coalesce", 2, 2),
    ("lower", 1, 1),
    ("length", 1, 1),
    ("trim", 1, 1),
];

/// The expression-function *names*, for readers outside the parser that must
/// show exactly the set the parser accepts and no other — today the editor
/// grammar's inventory test (`docs/plan-editor-tooling.md` increment 0), which
/// pins the highlighter's `support.function` list against this one in both
/// directions. A projection of `FUNCTIONS`, never a copy of it: a function
/// added above reaches the highlighter's test the same commit.
pub fn function_names() -> impl Iterator<Item = &'static str> {
    FUNCTIONS.iter().map(|(name, _, _)| *name)
}

/// The same closed set **with its arities** — `(name, min, max)`.
///
/// The arities used to be private, with the note that "nothing outside needs
/// them, and a public tuple would invite a second arity check somewhere
/// else". Something outside needs them now: `ordo-gen` generates the CDK's
/// expression-function vocabulary, and a typed builder that offers
/// `round(x)` without `round(x, n)` — or offers `mod(x)` — is a second
/// answer about arity. The alternative was for the generator to carry its own
/// table, which is *exactly* the second arity check the note feared; a
/// projection of the one table cannot be that.
///
/// This is a projection, not a copy: adding a function above reaches every
/// reader in the same commit. `function_names()` stays because two callers
/// only want names and `(n, _, _)` at each of them reads worse.
pub fn functions() -> impl Iterator<Item = (&'static str, usize, usize)> {
    FUNCTIONS.iter().copied()
}

/// Parse one complete expression; trailing input is an error.
pub fn parse_expr(src: &str) -> Result<Expr, ParseError> {
    let toks = lex(src)?;
    let mut p = Parser::new(&toks);
    let e = p.expr()?;
    p.finish()?;
    Ok(e)
}

/// Parse one complete condition; trailing input is an error.
pub fn parse_condition(src: &str) -> Result<Condition, ParseError> {
    let toks = lex(src)?;
    let mut p = Parser::new(&toks);
    let c = p.condition()?;
    p.finish()?;
    Ok(c)
}

/// Of two attempts at ambiguous input, report whichever error got further —
/// the parse that understood more of the line usually knows what went wrong.
fn prefer(prior: Option<ParseError>, e: ParseError) -> ParseError {
    match prior {
        Some(p) if p.span.start >= e.span.start => p,
        _ => e,
    }
}

pub(crate) fn describe(kind: &TokenKind) -> String {
    use TokenKind::*;
    match kind {
        Int(n) => format!("`{n}`"),
        Decimal(d) => format!("`{d}`"),
        Str(s) => format!("`\"{s}\"`"),
        Date { .. } => "a date literal".into(),
        Datetime { .. } => "a datetime literal".into(),
        Duration { n, unit } => format!("`{n}{}`", unit.as_str()),
        UtcTime { hour, minute } => format!("`{hour:02}:{minute:02}Z`"),
        Ident(s) => format!("`{s}`"),
        Param(s) => format!("`${s}`"),
        Keyword(k) => format!("`{}`", k.as_str()),
        LParen => "`(`".into(),
        RParen => "`)`".into(),
        LBracket => "`[`".into(),
        RBracket => "`]`".into(),
        LBrace => "`{`".into(),
        RBrace => "`}`".into(),
        Comma => "`,`".into(),
        Colon => "`:`".into(),
        Dot => "`.`".into(),
        Bar => "`|`".into(),
        Arrow => "`->`".into(),
        Eq => "`=`".into(),
        EqEq => "`==`".into(),
        NotEq => "`!=`".into(),
        Lt => "`<`".into(),
        LtEq => "`<=`".into(),
        Gt => "`>`".into(),
        GtEq => "`>=`".into(),
        Plus => "`+`".into(),
        Minus => "`-`".into(),
        Star => "`*`".into(),
        Slash => "`/`".into(),
        Question => "`?`".into(),
        At => "`@`".into(),
        Newline => "end of line".into(),
        Eof => "end of input".into(),
    }
}

/// Token-slice parser. Public so the future pipeline/entity parsers plug the
/// same expression and condition entry points into their own token streams.
pub struct Parser<'t> {
    toks: &'t [Token],
    pos: usize,
    /// Where a type was written as bare `money`, with no places. It means
    /// `money(0)`, so it parses; [`crate::bare_money_sites`] hands these to
    /// `ordo check` to warn about (ruled 2026-10-05).
    pub(crate) bare_money: Vec<crate::Span>,
    /// The parameter a guard comparison names its subject by: `caller` in
    /// an `access` clause, `member` inside a `release` rule (27aq). Either
    /// way the atom stores `caller: true`; the binder supplies the value.
    pub(crate) guard_subject: &'static str,
}

impl<'t> Parser<'t> {
    pub fn new(toks: &'t [Token]) -> Self {
        Parser {
            toks,
            pos: 0,
            bare_money: Vec::new(),
            guard_subject: "caller",
        }
    }

    // ── token plumbing ──

    pub(crate) fn tok(&self) -> &Token {
        // The lexer always terminates the stream with Eof.
        &self.toks[self.pos.min(self.toks.len().saturating_sub(1))]
    }

    pub(crate) fn peek(&self) -> &TokenKind {
        &self.tok().kind
    }

    /// Lookahead by `n` tokens. The declaration parser needs exactly one of
    /// these to tell `$cap: money` (a named argument) from `$cap` opening a
    /// condition, and to find field boundaries inside a brace block, where
    /// the lexer emits no separators (G6).
    pub(crate) fn peek_at(&self, n: usize) -> &TokenKind {
        let i = (self.pos + n).min(self.toks.len().saturating_sub(1));
        &self.toks[i].kind
    }

    /// Does the next token start on a later source line than the one just
    /// consumed?
    ///
    /// Inside `(` `[` `{` the lexer emits no `Newline` at all (G6), so a
    /// brace block that wants the line break as a separator cannot ask for a
    /// token — it has to read the break off the spans. `group`/`reduce` is
    /// the one construct that wants it: its entries are one-per-line in every
    /// doc and corpus, and `,` alone made that spelling a parse error.
    pub(crate) fn at_line_break(&self) -> bool {
        match self.pos.checked_sub(1).and_then(|i| self.toks.get(i)) {
            Some(prev) => self.tok().span.line > prev.span.line,
            None => false,
        }
    }

    /// Position bookmark for the one place the grammar needs backtracking:
    /// a source argument list mixes named bindings and conditions, and only
    /// a failed condition parse proves which one an item was.
    pub(crate) fn mark(&self) -> usize {
        self.pos
    }

    pub(crate) fn reset(&mut self, mark: usize) {
        self.pos = mark;
    }

    pub(crate) fn bump(&mut self) -> Token {
        let t = self.tok().clone();
        if self.pos < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    /// The default class: the parser could not read the text. Every site
    /// that does not deliberately say otherwise is `parse_error`, which is
    /// what the composition inventory names for "text outside the closed
    /// grammar".
    pub(crate) fn err_here(&self, message: impl Into<String>) -> ParseError {
        self.err_class_here(PARSE_ERROR, message)
    }

    pub(crate) fn err_at(&self, span: Span, message: impl Into<String>) -> ParseError {
        self.err_class_at(PARSE_ERROR, span, message)
    }

    /// A parse failure that belongs to a class other than `parse_error` —
    /// today only `invalid_guard_walk`, for guard atoms the grammar reads
    /// but R-G3 does not admit.
    pub(crate) fn err_class_here(
        &self,
        code: &'static str,
        message: impl Into<String>,
    ) -> ParseError {
        self.err_class_at(code, self.tok().span, message)
    }

    pub(crate) fn err_class_at(
        &self,
        code: &'static str,
        span: Span,
        message: impl Into<String>,
    ) -> ParseError {
        ParseError {
            code,
            message: message.into(),
            span,
        }
    }

    pub(crate) fn eat_kw(&mut self, kw: Keyword) -> bool {
        if matches!(self.peek(), TokenKind::Keyword(k) if *k == kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, kind: &TokenKind, what: &str) -> Result<Token, ParseError> {
        if self.peek() == kind {
            Ok(self.bump())
        } else {
            Err(self.err_here(format!("expected {what}, found {}", describe(self.peek()))))
        }
    }

    /// After a complete parse from a standalone entry point, only line end
    /// may remain.
    pub fn finish(&mut self) -> Result<(), ParseError> {
        while matches!(self.peek(), TokenKind::Newline) {
            self.bump();
        }
        if matches!(self.peek(), TokenKind::Eof) {
            Ok(())
        } else {
            Err(self.err_here(format!("unexpected trailing {}", describe(self.peek()))))
        }
    }

    // ── conditions ──

    /// condition := atom (('and' atom)* | ('or' atom)*) — same-connective
    /// chains only; mixing without parens is the spec §1.2 parse error.
    pub fn condition(&mut self) -> Result<Condition, ParseError> {
        let first = self.cond_atom()?;
        let conn = match self.peek() {
            TokenKind::Keyword(Keyword::And) => Keyword::And,
            TokenKind::Keyword(Keyword::Or) => Keyword::Or,
            _ => return Ok(first),
        };
        let mut items = vec![first];
        while self.eat_kw(conn) {
            items.push(self.cond_atom()?);
        }
        if let TokenKind::Keyword(other @ (Keyword::And | Keyword::Or)) = self.peek() {
            let other = other.as_str();
            return Err(self.err_here(format!(
                "cannot mix `{conn}` and `{other}` without parentheses — parenthesize one \
                 side, e.g. `(a {conn} b) {other} c` (no precedence table, spec §1)",
                conn = conn.as_str(),
            )));
        }
        let span = items
            .first()
            .unwrap()
            .span()
            .to(items.last().unwrap().span());
        Ok(match conn {
            Keyword::And => Condition::And { items, span },
            _ => Condition::Or { items, span },
        })
    }

    /// atom := 'not' atom | '(' condition ')' | comparison
    fn cond_atom(&mut self) -> Result<Condition, ParseError> {
        match self.peek() {
            TokenKind::Keyword(Keyword::Not) => {
                let not_tok = self.bump();
                // `not` binds to the following atom only — `not a and b`
                // negates `a`, never the chain.
                let inner = self.cond_atom()?;
                let span = not_tok.span.to(inner.span());
                Ok(Condition::Not {
                    inner: Box::new(inner),
                    span,
                })
            }
            TokenKind::LParen => self.paren_cond_or_comparison(),
            _ => self.comparison(),
        }
    }

    /// `(` in condition position is ambiguous: a boolean group `(a and b)`
    /// or a parenthesized value `(a + b) < c`. Try the group first; fall
    /// back to the comparison read, keeping whichever error got further.
    fn paren_cond_or_comparison(&mut self) -> Result<Condition, ParseError> {
        let snap = self.pos;
        match self.paren_group() {
            Ok(c) => {
                if self.continues_expr() {
                    if matches!(c, Condition::BareBool(_)) {
                        // `(a) < b`, `(a + b) * 2 > c` — the parens were an
                        // expression's; reread as a comparison.
                        self.pos = snap;
                        self.comparison()
                    } else {
                        Err(self.err_here(format!(
                            "a parenthesized condition is not a value — it cannot continue \
                             with {}",
                            describe(self.peek())
                        )))
                    }
                } else {
                    Ok(c)
                }
            }
            Err(group_err) => {
                self.pos = snap;
                match self.comparison() {
                    Ok(c) => Ok(c),
                    Err(e) => Err(prefer(Some(group_err), e)),
                }
            }
        }
    }

    fn paren_group(&mut self) -> Result<Condition, ParseError> {
        self.expect(&TokenKind::LParen, "`(`")?;
        let c = self.condition()?;
        self.expect(&TokenKind::RParen, "`)` to close the group")?;
        Ok(c)
    }

    /// Could the current token extend an expression or start a comparison?
    fn continues_expr(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::EqEq
                | TokenKind::NotEq
                | TokenKind::Lt
                | TokenKind::LtEq
                | TokenKind::Gt
                | TokenKind::GtEq
                | TokenKind::Keyword(
                    Keyword::In
                        | Keyword::Is
                        | Keyword::Contains
                        | Keyword::Starts
                        | Keyword::Matches
                        | Keyword::Since
                        | Keyword::Under
                        | Keyword::Above
                )
        )
    }

    fn cmp_op(&self) -> Option<CmpOp> {
        Some(match self.peek() {
            TokenKind::EqEq => CmpOp::Eq,
            TokenKind::NotEq => CmpOp::Ne,
            TokenKind::Lt => CmpOp::Lt,
            TokenKind::LtEq => CmpOp::Le,
            TokenKind::Gt => CmpOp::Gt,
            TokenKind::GtEq => CmpOp::Ge,
            _ => return None,
        })
    }

    /// comparison := expr cmp expr | expr 'in' '(' expr,+ ')'
    ///             | expr ('contains'|'starts'|'matches') expr
    ///             | expr 'is' ['not'] 'null' | expr 'since' duration
    ///             | expr ('under'|'above') expr | bare-bool
    fn comparison(&mut self) -> Result<Condition, ParseError> {
        let lhs = self.expr()?;
        if let Some(op) = self.cmp_op() {
            self.bump();
            let rhs = self.expr()?;
            if self.cmp_op().is_some() {
                return Err(self.err_here(
                    "comparisons do not chain — chain comparisons with `and`: \
                     `a < b and b < c`",
                ));
            }
            let span = lhs.span().to(rhs.span());
            return Ok(Condition::Compare { op, lhs, rhs, span });
        }
        match self.peek() {
            TokenKind::Eq => {
                Err(self.err_here("`=` is declaration syntax — equality in a condition is `==`"))
            }
            TokenKind::Keyword(Keyword::In) => {
                self.bump();
                self.expect(
                    &TokenKind::LParen,
                    "`(` — `in` takes a parenthesized list: `x in ($a, $b)`",
                )?;
                let mut list = vec![self.expr()?];
                while matches!(self.peek(), TokenKind::Comma) {
                    self.bump();
                    list.push(self.expr()?);
                }
                let rp = self.expect(&TokenKind::RParen, "`)` to close the `in` list")?;
                let span = lhs.span().to(rp.span);
                Ok(Condition::In {
                    expr: lhs,
                    list,
                    span,
                })
            }
            TokenKind::Keyword(Keyword::Is) => {
                self.bump();
                let negated = self.eat_kw(Keyword::Not);
                if !matches!(self.peek(), TokenKind::Keyword(Keyword::Null)) {
                    return Err(self.err_here(format!(
                        "`is` is followed by `null` or `not null`, found {}",
                        describe(self.peek())
                    )));
                }
                let null_tok = self.bump();
                let span = lhs.span().to(null_tok.span);
                Ok(Condition::IsNull {
                    expr: lhs,
                    negated,
                    span,
                })
            }
            TokenKind::Keyword(kw @ (Keyword::Contains | Keyword::Starts | Keyword::Matches)) => {
                let op = match kw {
                    Keyword::Contains => TextOp::Contains,
                    Keyword::Starts => TextOp::Starts,
                    _ => TextOp::Matches,
                };
                self.bump();
                let rhs = self.expr()?;
                let span = lhs.span().to(rhs.span());
                Ok(Condition::Text { op, lhs, rhs, span })
            }
            TokenKind::Keyword(Keyword::Since) => {
                self.bump();
                if let TokenKind::Duration { n, unit } = *self.peek() {
                    let t = self.bump();
                    let dur = Expr::DurationLit {
                        n,
                        unit,
                        span: t.span,
                    };
                    let span = lhs.span().to(t.span);
                    Ok(Condition::Since {
                        expr: lhs,
                        dur,
                        span,
                    })
                } else {
                    Err(self.err_here(format!(
                        "`since` takes a duration literal (`90d`, `1mo`), found {}",
                        describe(self.peek())
                    )))
                }
            }
            TokenKind::Keyword(kw @ (Keyword::Under | Keyword::Above)) => {
                let is_under = matches!(kw, Keyword::Under);
                self.bump();
                let id = self.expr()?;
                let span = lhs.span().to(id.span());
                Ok(if is_under {
                    Condition::Under {
                        expr: lhs,
                        id,
                        span,
                    }
                } else {
                    Condition::Above {
                        expr: lhs,
                        id,
                        span,
                    }
                })
            }
            _ => match lhs {
                // G7: a bare bool field/path (or bool literal) is a complete
                // condition; anything else standing alone is an error.
                Expr::Path { .. } | Expr::BoolLit { .. } => Ok(Condition::BareBool(lhs)),
                other => Err(self.err_at(
                    other.span(),
                    "an expression is not a condition on its own — compare it, or use a \
                     bare boolean field",
                )),
            },
        }
    }

    // ── expressions ──

    /// expr := term (('+' | '-') term)* — left-associative.
    pub fn expr(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.term()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.term()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    /// term := unary (('*' | '/') unary)*
    fn term(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.unary()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                _ => break,
            };
            self.bump();
            let rhs = self.unary()?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    /// unary := '-' unary | primary
    fn unary(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek(), TokenKind::Minus) {
            let minus = self.bump();
            let inner = self.unary()?;
            let span = minus.span.to(inner.span());
            return Ok(Expr::Neg {
                expr: Box::new(inner),
                span,
            });
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            TokenKind::Int(value) => {
                let t = self.bump();
                Ok(Expr::IntLit {
                    value,
                    span: t.span,
                })
            }
            TokenKind::Decimal(digits) => {
                let t = self.bump();
                Ok(Expr::DecimalLit {
                    digits,
                    span: t.span,
                })
            }
            TokenKind::Str(value) => {
                let t = self.bump();
                Ok(Expr::StrLit {
                    value,
                    span: t.span,
                })
            }
            TokenKind::Date { y, m, d } => {
                let t = self.bump();
                Ok(Expr::DateLit {
                    y,
                    m,
                    d,
                    span: t.span,
                })
            }
            TokenKind::Datetime {
                y,
                m,
                d,
                hh,
                mm,
                ss,
            } => {
                let t = self.bump();
                Ok(Expr::DatetimeLit {
                    y,
                    m,
                    d,
                    hh,
                    mm,
                    ss,
                    span: t.span,
                })
            }
            TokenKind::Duration { n, unit } => {
                let t = self.bump();
                Ok(Expr::DurationLit {
                    n,
                    unit,
                    span: t.span,
                })
            }
            TokenKind::Param(name) => {
                let t = self.bump();
                Ok(Expr::Param { name, span: t.span })
            }
            TokenKind::Keyword(Keyword::True) => {
                let t = self.bump();
                Ok(Expr::BoolLit {
                    value: true,
                    span: t.span,
                })
            }
            TokenKind::Keyword(Keyword::False) => {
                let t = self.bump();
                Ok(Expr::BoolLit {
                    value: false,
                    span: t.span,
                })
            }
            TokenKind::Keyword(Keyword::Null) => {
                let t = self.bump();
                Ok(Expr::NullLit { span: t.span })
            }
            TokenKind::Keyword(Keyword::Cond) => {
                let t = self.bump();
                self.cond_expr(t.span)
            }
            TokenKind::LParen => {
                self.bump();
                let e = self.expr()?;
                self.expect(&TokenKind::RParen, "`)`")?;
                Ok(e)
            }
            TokenKind::Ident(name) => {
                let t = self.bump();
                let head = Ident { name, span: t.span };
                if matches!(self.peek(), TokenKind::LParen) {
                    self.fn_call(head)
                } else {
                    self.path(head)
                }
            }
            // A reserved word heads a path only when a dot follows it
            // (`sort group.displayName` over `groupMembers`): the word then
            // names the platform's ref, and no step or literal is spelled
            // `<keyword>.`, so the position decides. See `path`.
            TokenKind::Keyword(k) if matches!(self.peek_at(1), TokenKind::Dot) => {
                let t = self.bump();
                self.path(Ident {
                    name: k.as_str().to_string(),
                    span: t.span,
                })
            }
            other => Err(self.err_here(format!(
                "expected an expression, found {}",
                describe(&other)
            ))),
        }
    }

    /// path := ident ('.' segment)* — values dot (many→one); walking into
    /// collections is sema's rejection, not a parse shape.
    ///
    /// A segment after the dot may be a reserved word, read as the field
    /// name it is in that position: after `.` nothing but a segment can
    /// follow, so position disambiguates (DL §2's rule for the contextual
    /// tier). The only such fields are the platform's — `groupMembers.group`
    /// (DL §3 Groups) — because an author cannot declare one. Refusing the
    /// word here left DL's own `match groupMembers.group` unwritable.
    ///
    /// A segment may also be written in brackets, for a key inside a `json`
    /// document that is not a name: `prefs["my key"]`, or an array position,
    /// `tags[0]`. A bracketed key is the same segment its dotted spelling
    /// would be (`prefs["theme"]` is `prefs.theme`), and a position is its
    /// digits, so the tree has one shape for both. Whether the path walks
    /// into a document is sema's question, as every path's meaning is.
    fn path(&mut self, head: Ident) -> Result<Expr, ParseError> {
        let mut span = head.span;
        let mut segments = vec![head];
        loop {
            if matches!(self.peek(), TokenKind::LBracket) {
                let open = self.bump();
                let name = match self.peek().clone() {
                    TokenKind::Str(key) => {
                        self.bump();
                        key
                    }
                    TokenKind::Int(n) if n >= 0 => {
                        self.bump();
                        n.to_string()
                    }
                    other => {
                        return Err(self.err_here(format!(
                            "expected a key in quotes or an array position after `[`, found {}",
                            describe(&other)
                        )));
                    }
                };
                let close = self.expect(&TokenKind::RBracket, "`]` to close the key")?;
                let seg_span = open.span.to(close.span);
                span = span.to(seg_span);
                segments.push(Ident {
                    name,
                    span: seg_span,
                });
                continue;
            }
            if !matches!(self.peek(), TokenKind::Dot) {
                break;
            }
            self.bump();
            match self.peek().clone() {
                TokenKind::Ident(name) => {
                    let t = self.bump();
                    span = span.to(t.span);
                    segments.push(Ident { name, span: t.span });
                }
                TokenKind::Keyword(k) => {
                    let t = self.bump();
                    span = span.to(t.span);
                    segments.push(Ident {
                        name: k.as_str().to_string(),
                        span: t.span,
                    });
                }
                other => {
                    return Err(self.err_here(format!(
                        "expected a field name after `.`, found {}",
                        describe(&other)
                    )));
                }
            }
        }
        Ok(Expr::Path { segments, span })
    }

    fn fn_call(&mut self, name: Ident) -> Result<Expr, ParseError> {
        let Some(&(_, min, max)) = FUNCTIONS.iter().find(|(n, _, _)| *n == name.name) else {
            return Err(self.err_at(
                name.span,
                format!(
                    "unknown function `{}` — expression functions are a closed set: {}",
                    name.name,
                    FUNCTIONS
                        .iter()
                        .map(|(n, _, _)| *n)
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            ));
        };
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut args = Vec::new();
        if !matches!(self.peek(), TokenKind::RParen) {
            args.push(self.expr()?);
            while matches!(self.peek(), TokenKind::Comma) {
                self.bump();
                args.push(self.expr()?);
            }
        }
        let rp = self.expect(&TokenKind::RParen, "`)` to close the call")?;
        if args.len() < min || args.len() > max {
            let want = if min == max {
                format!("exactly {min} argument{}", if min == 1 { "" } else { "s" })
            } else {
                format!("{min} or {max} arguments")
            };
            return Err(self.err_at(
                name.span.to(rp.span),
                format!("`{}` takes {want}, got {}", name.name, args.len()),
            ));
        }
        let span = name.span.to(rp.span);
        if name.name == "allocate" && args.len() == 3 {
            self.allocate_mode(&args[2])?;
        }
        Ok(Expr::FnCall { name, args, span })
    }

    /// `allocate`'s third argument is a **mode name**, not an expression.
    ///
    /// Checked here rather than in the binder because it is a question about
    /// the written form, and because the answer must be the same wherever
    /// `allocate` eventually becomes legal — the site is still `[lean]`, and
    /// a rule that lives in the site's binder would be written by whoever
    /// lands the site.
    ///
    /// The refusal carries no class, like every other parse error: the
    /// closed inventory is the *compile* inventory, and a mode name is
    /// grammar. The message names the legal modes, and names `rotation`
    /// specifically, because it is the mode people reach for and the reason
    /// it is missing is a ruling rather than an omission.
    fn allocate_mode(&self, arg: &Expr) -> Result<(), ParseError> {
        let (written, span) = match arg {
            Expr::Path { segments, span } if segments.len() == 1 => {
                (segments[0].name.as_str(), *span)
            }
            other => {
                return Err(self.err_at(
                    other.span(),
                    format!(
                        "`allocate`'s third argument is the mode — a bare name from {} — \
                         not an expression. Omit it for `largestRemainder`.",
                        ALLOCATE_MODES.join(" ")
                    ),
                ))
            }
        };
        if ALLOCATE_MODES.contains(&written) {
            return Ok(());
        }
        let hint = if written == "rotation" {
            " — `rotation` is deliberately absent: spreading the odd unit over \
             successive invoices has to remember what it did last time, which is \
             application state, not a function of (total, weights)"
        } else {
            ""
        };
        Err(self.err_at(
            span,
            format!(
                "unknown allocation mode `{written}` — the modes are: {}{hint}",
                ALLOCATE_MODES.join(" ")
            ),
        ))
    }

    /// cond(c1 -> v1, c2 -> v2, …, velse) — arms in order, final bare else
    /// mandatory (totality, spec §3).
    fn cond_expr(&mut self, kw_span: Span) -> Result<Expr, ParseError> {
        self.expect(
            &TokenKind::LParen,
            "`(` — cond is `cond(c1 -> v1, …, velse)`",
        )?;
        if matches!(self.peek(), TokenKind::RParen) {
            return Err(self.err_here(
                "cond needs at least one `condition -> value` arm and a final bare else value",
            ));
        }
        let mut arms = Vec::new();
        loop {
            let snap = self.pos;
            match self.condition() {
                Ok(condition) if matches!(self.peek(), TokenKind::Arrow) => {
                    self.bump(); // ->
                    let value = self.expr()?;
                    arms.push(CondArm { condition, value });
                    match self.peek() {
                        TokenKind::Comma => {
                            self.bump();
                        }
                        TokenKind::RParen => {
                            return Err(self.err_here(
                                "cond needs a final bare else value — the last entry is a \
                                 value with no `->`",
                            ));
                        }
                        other => {
                            return Err(self.err_here(format!(
                                "expected `,` or `)` in cond(…), found {}",
                                describe(other)
                            )));
                        }
                    }
                }
                Ok(_) if matches!(self.peek(), TokenKind::Comma) => {
                    return Err(self.err_here(
                        "this cond arm is missing `->` — arms are `condition -> value`",
                    ));
                }
                other => {
                    // Not an arm: this entry must be the final bare else.
                    let prior = other.err();
                    self.pos = snap;
                    let else_ = match self.expr() {
                        Ok(e) => e,
                        Err(e) => return Err(prefer(prior, e)),
                    };
                    if matches!(self.peek(), TokenKind::Comma) {
                        return Err(prefer(
                            prior,
                            self.err_here("in cond(…), only the final else value omits `->`"),
                        ));
                    }
                    if arms.is_empty() {
                        return Err(prefer(
                            prior,
                            self.err_at(
                                else_.span(),
                                "cond needs at least one `condition -> value` arm before the \
                                 else value",
                            ),
                        ));
                    }
                    let rp = match self.expect(&TokenKind::RParen, "`)` to close cond(…)") {
                        Ok(t) => t,
                        Err(e) => return Err(prefer(prior, e)),
                    };
                    let span = kw_span.to(rp.span);
                    return Ok(Expr::Cond {
                        arms,
                        else_: Box::new(else_),
                        span,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(src: &str) -> Condition {
        parse_condition(src).unwrap_or_else(|e| panic!("`{src}` should parse: {e}"))
    }

    fn expr(src: &str) -> Expr {
        parse_expr(src).unwrap_or_else(|e| panic!("`{src}` should parse: {e}"))
    }

    fn cond_err(src: &str) -> String {
        parse_condition(src)
            .expect_err(&format!("`{src}` should be rejected"))
            .message
    }

    fn expr_err(src: &str) -> String {
        parse_expr(src)
            .expect_err(&format!("`{src}` should be rejected"))
            .message
    }

    fn path(e: &Expr) -> Vec<&str> {
        match e {
            Expr::Path { segments, .. } => segments.iter().map(|s| s.name.as_str()).collect(),
            other => panic!("expected a path, got {other:?}"),
        }
    }

    // ── accepts ──

    #[test]
    fn a_bracketed_key_or_position_is_a_path_segment() {
        let e = parse_expr(r#"prefs["my key"].deep[0]"#).expect("parses");
        assert_eq!(path(&e), ["prefs", "my key", "deep", "0"]);
        // A name-shaped key in brackets is the dotted segment.
        let bracketed = parse_expr(r#"prefs["theme"]"#).expect("parses");
        let dotted = parse_expr("prefs.theme").expect("parses");
        assert_eq!(path(&bracketed), path(&dotted));
        let e = parse_expr(r#"prefs["it's"]"#).expect("parses");
        assert_eq!(path(&e), ["prefs", "it's"]);
    }

    #[test]
    fn a_bracket_takes_only_a_string_or_a_position() {
        assert!(expr_err("tags[-1]").contains("array position"));
        assert!(expr_err("tags[x]").contains("array position"));
        assert!(expr_err("tags[1.5]").contains("array position"));
        assert!(expr_err(r#"tags["a""#).contains("`]`"));
    }

    #[test]
    fn simple_comparisons() {
        match cond(r#"status == "open""#) {
            Condition::Compare {
                op: CmpOp::Eq,
                lhs,
                rhs,
                ..
            } => {
                assert_eq!(path(&lhs), ["status"]);
                assert!(matches!(rhs, Expr::StrLit { ref value, .. } if value == "open"));
            }
            other => panic!("{other:?}"),
        }
        // Field-to-field comparison (spec §4 batch pins).
        match cond("commitDate < receiptDate") {
            Condition::Compare {
                op: CmpOp::Lt,
                lhs,
                rhs,
                ..
            } => {
                assert_eq!(path(&lhs), ["commitDate"]);
                assert_eq!(path(&rhs), ["receiptDate"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bare_bool_and_null_tests() {
        // G7: a bare bool field is a complete condition.
        assert!(matches!(
            cond("active"),
            Condition::BareBool(Expr::Path { .. })
        ));
        assert!(matches!(
            cond("payee is null"),
            Condition::IsNull { negated: false, .. }
        ));
        assert!(matches!(
            cond("payee is not null"),
            Condition::IsNull { negated: true, .. }
        ));
    }

    #[test]
    fn same_connective_chains_flat() {
        match cond("a and b and c") {
            Condition::And { items, .. } => assert_eq!(items.len(), 3),
            other => panic!("{other:?}"),
        }
        match cond("(a and b) or c") {
            Condition::Or { items, .. } => {
                assert_eq!(items.len(), 2);
                assert!(matches!(items[0], Condition::And { .. }));
            }
            other => panic!("{other:?}"),
        }
        match cond("not (a or b)") {
            Condition::Not { inner, .. } => assert!(matches!(*inner, Condition::Or { .. })),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn since_window() {
        match cond("amount < 0 and date since 90d") {
            Condition::And { items, .. } => {
                assert!(matches!(items[0], Condition::Compare { op: CmpOp::Lt, .. }));
                assert!(matches!(items[1], Condition::Since { .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn membership_list() {
        match cond("category in ($a, $b)") {
            Condition::In { expr, list, .. } => {
                assert_eq!(path(&expr), ["category"]);
                assert_eq!(list.len(), 2);
                assert!(matches!(list[0], Expr::Param { ref name, .. } if name == "a"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn null_trap_example() {
        // The outer-preserving null-comparison trap (spec §5): `or at is null`.
        match cond("at <= $x or at is null") {
            Condition::Or { items, .. } => {
                assert!(matches!(items[0], Condition::Compare { op: CmpOp::Le, .. }));
                assert!(matches!(items[1], Condition::IsNull { negated: false, .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tree_operators() {
        match cond(r#"project under $root and status == "open""#) {
            Condition::And { items, .. } => {
                assert!(matches!(items[0], Condition::Under { .. }));
                assert!(matches!(items[1], Condition::Compare { .. }));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(cond("node above $leaf"), Condition::Above { .. }));
    }

    #[test]
    fn text_operators() {
        match cond(r#"payee.name contains "Amazon""#) {
            Condition::Text {
                op: TextOp::Contains,
                lhs,
                ..
            } => {
                assert_eq!(path(&lhs), ["payee", "name"]);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            cond(r#"name starts "A""#),
            Condition::Text {
                op: TextOp::Starts,
                ..
            }
        ));
        assert!(matches!(
            cond("name matches $pattern"),
            Condition::Text {
                op: TextOp::Matches,
                ..
            }
        ));
    }

    #[test]
    fn arithmetic_precedence_and_parens() {
        // * binds over + — conventional arithmetic precedence (G5 family;
        // only BOOLEAN mixing needs parens).
        match expr("1 + 2 * 3") {
            Expr::Binary {
                op: BinOp::Add,
                rhs,
                ..
            } => {
                assert!(matches!(*rhs, Expr::Binary { op: BinOp::Mul, .. }));
            }
            other => panic!("{other:?}"),
        }
        match expr("3 * baseline.a") {
            Expr::Binary {
                op: BinOp::Mul,
                rhs,
                ..
            } => assert_eq!(path(&rhs), ["baseline", "a"]),
            other => panic!("{other:?}"),
        }
        assert!(matches!(expr("-(a + b)"), Expr::Neg { .. }));
        // Scalar-baseline example: rhs is the whole product.
        match cond("amount < 3 * baseline.a") {
            Condition::Compare {
                op: CmpOp::Lt, rhs, ..
            } => {
                assert!(matches!(rhs, Expr::Binary { op: BinOp::Mul, .. }));
            }
            other => panic!("{other:?}"),
        }
        // Parenthesized value on the left of a comparison — the paren
        // ambiguity resolves to an expression.
        assert!(matches!(
            cond("(a + b) < c"),
            Condition::Compare {
                op: CmpOp::Lt,
                lhs: Expr::Binary { .. },
                ..
            }
        ));
    }

    #[test]
    fn function_calls_closed_set() {
        match expr("coalesce(round(amount, 2), 0)") {
            Expr::FnCall { name, args, .. } => {
                assert_eq!(name.name, "coalesce");
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0], Expr::FnCall { .. }));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(expr("year(date)"), Expr::FnCall { .. }));
        assert!(matches!(expr("least(a, b)"), Expr::FnCall { .. }));
        // Aggregate names stay contextual: `count` is a fine field name.
        assert_eq!(path(&expr("count")), ["count"]);
    }

    #[test]
    fn cond_expression() {
        match expr(r#"cond(spent > 1000 -> "vip", spent > 100 -> "core", "casual")"#) {
            Expr::Cond { arms, else_, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(matches!(
                    arms[0].condition,
                    Condition::Compare { op: CmpOp::Gt, .. }
                ));
                assert!(matches!(*else_, Expr::StrLit { ref value, .. } if value == "casual"));
            }
            other => panic!("{other:?}"),
        }
        // Arm conditions use the full grammar; a bare-path else is fine too.
        assert!(matches!(
            expr(r#"cond(status == "urgent" and open -> 0, fallback)"#),
            Expr::Cond { .. }
        ));
    }

    #[test]
    fn duration_and_date_arithmetic() {
        // `end <= $today + 60d` (the lease-expiry rule's shape).
        match cond("end <= $today + 60d") {
            Condition::Compare {
                op: CmpOp::Le, rhs, ..
            } => {
                assert!(matches!(rhs, Expr::Binary { op: BinOp::Add, .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    // ── rejects ──

    #[test]
    fn mixing_and_or_needs_parens() {
        let msg = cond_err("a and b or c");
        assert!(msg.contains("parenthes"), "got: {msg}");
        assert!(msg.contains("`and`") && msg.contains("`or`"), "got: {msg}");
        assert!(cond_err("a or b and c").contains("parenthes"));
        // The same rule inside a cond arm.
        assert!(expr_err("cond(a and b or c -> 1, 2)").contains("parenthes"));
    }

    #[test]
    fn comparisons_do_not_chain() {
        let msg = cond_err("a < b < c");
        assert!(msg.contains("chain comparisons with `and`"), "got: {msg}");
    }

    #[test]
    fn unknown_function_rejected() {
        let msg = expr_err("foo(x)");
        assert!(msg.contains("unknown function `foo`"), "got: {msg}");
        assert!(msg.contains("closed set"), "got: {msg}");
    }

    #[test]
    fn arity_checked_at_parse_time() {
        assert!(expr_err("mod(1)").contains("exactly 2 arguments"));
        assert!(expr_err("round(1, 2, 3)").contains("1 or 2 arguments"));
        assert!(expr_err("least(1)").contains("exactly 2 arguments"));
    }

    /// G12 is ruled (2026-08-30), so `hash` is in the language again — with
    /// the arity DL §3 gives it, and with an algorithm the compatibility
    /// promise can actually be held to (`ordo_plan::hash64`). It spent 9f″
    /// out of the list on purpose: an unruled algorithm behind a
    /// bit-stability promise is a promise made and broken in one commit.
    #[test]
    fn hash_is_a_one_argument_function() {
        assert!(matches!(expr("hash(id)"), Expr::FnCall { .. }));
        assert!(expr_err("hash(a, b)").contains("exactly 1 argument"));
        assert!(expr_err("hash()").contains("exactly 1 argument"));
    }

    #[test]
    fn cond_shape_errors() {
        let msg = expr_err(r#"cond(spent > 1000 -> "vip")"#);
        assert!(msg.contains("final bare else"), "got: {msg}");
        assert!(expr_err("cond(a > 1 -> 2, b > 1, 3)").contains("missing `->`"));
        assert!(expr_err("cond()").contains("cond needs"));
    }

    #[test]
    fn dangling_and_bare_rejections() {
        assert!(cond_err("x == ").contains("expected an expression"));
        assert!(cond_err("not").contains("expected an expression"));
        // A non-boolean bare expression is not a condition (G7).
        assert!(cond_err("a + b").contains("not a condition"));
        // `=` is not equality.
        assert!(cond_err("x = 5").contains("`==`"));
        // Trailing junk after a complete condition.
        assert!(cond_err("a == b c").contains("unexpected trailing"));
    }

    #[test]
    fn since_takes_a_duration_literal() {
        assert!(cond_err("date since $w").contains("duration literal"));
        assert!(cond_err("date since 90").contains("duration literal"));
    }
}
