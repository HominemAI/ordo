//! AST for the expression and condition grammar (spec §3–§4). Real ADTs,
//! every node carrying its source span — diagnostics downstream (sema, the
//! planner) always have a place to point at.
//!
//! Pipeline and entity nodes arrive with their parsers; nothing here
//! anticipates them beyond `Condition`/`Expr` being reusable everywhere the
//! spec says "the closed expression grammar".

use crate::span::Span;
use crate::token::DurationUnit;

/// A single name with its span (path segments, function names).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// Arithmetic operators. `+` doubles as text concatenation — the parser does
/// not care; typing is sema's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
        }
    }
}

/// Comparison operators — non-associative by grammar (`a < b < c` is a parse
/// error; chain with `and`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    pub fn as_str(self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

/// Text-matching operators (spec §4): matching stays in operators, never
/// functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextOp {
    Contains,
    Starts,
    Matches,
}

impl TextOp {
    pub fn as_str(self) -> &'static str {
        match self {
            TextOp::Contains => "contains",
            TextOp::Starts => "starts",
            TextOp::Matches => "matches",
        }
    }
}

/// One `condition -> value` arm of a `cond(…)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CondArm {
    pub condition: Condition,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    IntLit {
        value: i128,
        span: Span,
    },
    /// Exact source digits (`10.50`) — arithmetic is exact decimal, no float
    /// ever exists (spec §2 numeric semantics).
    DecimalLit {
        digits: String,
        span: Span,
    },
    StrLit {
        value: String,
        span: Span,
    },
    DateLit {
        y: i32,
        m: u8,
        d: u8,
        span: Span,
    },
    DatetimeLit {
        y: i32,
        m: u8,
        d: u8,
        hh: u8,
        mm: u8,
        ss: u8,
        span: Span,
    },
    DurationLit {
        n: u64,
        unit: DurationUnit,
        span: Span,
    },
    BoolLit {
        value: bool,
        span: Span,
    },
    NullLit {
        span: Span,
    },
    /// `$name` parameter reference.
    Param {
        name: String,
        span: Span,
    },
    /// Dot-path: `payee.name` (a single field is a one-segment path).
    /// Values dot; collections match — direction is checked by sema.
    Path {
        segments: Vec<Ident>,
        span: Span,
    },
    /// Unary minus: `-1050`, `-(a + b)`.
    Neg {
        expr: Box<Expr>,
        span: Span,
    },
    /// Left-associative arithmetic with conventional precedence (`* /` over
    /// `+ -`) — only BOOLEAN mixing is parenthesized (spec §1.2).
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    /// Call of one of the closed expression functions (spec §3) — name and
    /// fixed arity already parse-checked.
    FnCall {
        name: Ident,
        args: Vec<Expr>,
        span: Span,
    },
    /// `cond(c1 -> v1, c2 -> v2, …, velse)` — the only conditional. The
    /// final bare else is mandatory (totality); arms evaluate in order.
    Cond {
        arms: Vec<CondArm>,
        else_: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::IntLit { span, .. }
            | Expr::DecimalLit { span, .. }
            | Expr::StrLit { span, .. }
            | Expr::DateLit { span, .. }
            | Expr::DatetimeLit { span, .. }
            | Expr::DurationLit { span, .. }
            | Expr::BoolLit { span, .. }
            | Expr::NullLit { span }
            | Expr::Param { span, .. }
            | Expr::Path { span, .. }
            | Expr::Neg { span, .. }
            | Expr::Binary { span, .. }
            | Expr::FnCall { span, .. }
            | Expr::Cond { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// `lhs op rhs` — both sides any row-scoped expression
    /// (field-to-field comparisons are legal, spec §4 batch pins).
    Compare {
        op: CmpOp,
        lhs: Expr,
        rhs: Expr,
        span: Span,
    },
    /// `expr in ($a, $b)` — membership over a parenthesized comma list.
    In {
        expr: Expr,
        list: Vec<Expr>,
        span: Span,
    },
    /// `lhs contains rhs` / `starts` / `matches`.
    Text {
        op: TextOp,
        lhs: Expr,
        rhs: Expr,
        span: Span,
    },
    /// `expr is null` / `expr is not null` — the only null tests.
    IsNull {
        expr: Expr,
        negated: bool,
        span: Span,
    },
    /// `expr since 90d` — trailing window of execution time.
    Since { expr: Expr, dur: Expr, span: Span },
    /// `expr under $id` (tree entities only).
    Under { expr: Expr, id: Expr, span: Span },
    /// `expr above $id` (tree entities only).
    Above { expr: Expr, id: Expr, span: Span },
    /// A bare boolean field/path as a complete condition (G7) — the parser
    /// admits paths and bool literals here; typing is sema's check.
    BareBool(Expr),
    /// Same-connective chain `a and b and c` — flat by construction, since
    /// mixing without parens does not parse.
    And { items: Vec<Condition>, span: Span },
    /// Same-connective chain `a or b or c`.
    Or { items: Vec<Condition>, span: Span },
    /// `not <atom>` — binds to the following comparison or parenthesized
    /// group.
    Not { inner: Box<Condition>, span: Span },
}

impl Condition {
    pub fn span(&self) -> Span {
        match self {
            Condition::Compare { span, .. }
            | Condition::In { span, .. }
            | Condition::Text { span, .. }
            | Condition::IsNull { span, .. }
            | Condition::Since { span, .. }
            | Condition::Under { span, .. }
            | Condition::Above { span, .. }
            | Condition::And { span, .. }
            | Condition::Or { span, .. }
            | Condition::Not { span, .. } => *span,
            Condition::BareBool(expr) => expr.span(),
        }
    }
}
