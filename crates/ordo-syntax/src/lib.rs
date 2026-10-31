//! Ordo surface syntax: tokens, lexer, AST, and parser.
//!
//! The grammar is closed and line-oriented (spec §1, engine-plan G6): one
//! spelling per concept, a step ends at end-of-line unless a delimiter is
//! open, and anything outside the grammar fails loudly with a spanned error.

pub mod ast;
pub mod decl;
pub mod declparse;
pub mod lexer;
pub mod numeric;
pub mod parser;
pub mod span;
pub mod text;
pub mod token;
pub mod trigger;

pub use ast::{BinOp, CmpOp, CondArm, Condition, Expr, Ident, TextOp};
pub use decl::{
    AggDecl, DomainDecl, EntityDecl, FieldDecl, FieldKind, GuardAtom, GuardClause, GuardExpr,
    OpBody, OpDecl, OpFlags, ParamDecl, Pipeline, Program, ProjItem, ReassignTarget, ReleaseAtom,
    ReleaseCond, ReleaseDecl, ReleaseEffect, ReleaseMode, RoutineDecl, RoutineNotify,
    RoutineTrigger, ScalarTy, SecretDecl, SortKey, Source, SourceArg, Step, TakeCount, WriteStmt,
    DIVISION_SCALE,
};
pub use declparse::{
    bare_money_sites, parse_pipeline, parse_program, parse_whole, AggShape, AGGREGATE_NAMES,
    AGGREGATE_SHAPES,
};
pub use lexer::{lex, LexError};
pub use parser::{
    function_names, functions, parse_condition, parse_expr, ParseError, Parser, ALLOCATE_MODES,
    BARE_FIELD_IN_GROUP, DUPLICATE_RELEASE, INVALID_GUARD_WALK, INVALID_SECRET_HOST, PARSE_ERROR,
    RELEASE_SUBJECT,
};
pub use span::Span;
pub use text::{is_nfc, nfc};
pub use token::{Token, TokenKind};
pub use trigger::{
    EventKind, KindPos, ListenerPattern, Modifiers, Pos, Schedule, UNKNOWN_EVENT_KIND,
};
