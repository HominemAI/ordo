//! Ordo surface syntax: tokens, lexer, AST, and parser.
//!
//! The grammar is closed and line-oriented (spec §1, engine-plan G6): one
//! spelling per concept, a step ends at end-of-line unless a delimiter is
//! open, and anything outside the grammar fails loudly with a spanned error.

pub mod ast;
pub mod auth;
pub mod decl;
pub mod declparse;
pub mod lexer;
pub mod numeric;
pub mod parser;
pub mod span;
pub mod text;
pub mod token;
pub mod trigger;

#[cfg(test)]
mod function_tests;

pub use ast::{BinOp, CmpOp, CondArm, Condition, Expr, Ident, TextOp};
pub use auth::{
    AuthDecl, AuthDuration, AuthGuest, AuthLinking, AuthMethods, AuthProvider, AuthProvisioning,
    AuthRange, AuthRemember, AuthSecondFactor, AuthSession, AuthSwitch, AuthText,
    AuthUserVerification, ProviderKind, ProviderSource, Provisioning, RememberMax, SecondFactor,
    UserVerification, AUTH_BUILT_IN_METHODS, AUTH_CLAUSES, AUTH_PRESETS, PROVISIONING_MODES,
};
pub use decl::{
    AggDecl, DomainDecl, EditionDecl, EntityDecl, ExternalDecl, ExternalField, ExternalKeyTy,
    FieldDecl, FieldKind, FunctionDecl, FunctionHost, FunctionLimits, GrantDecl, GrantLine,
    GrantPattern, GuardAtom, GuardClause, GuardExpr, OpBody, OpDecl, OpFlags, ParamDecl, PeerDecl,
    PeerUse, Pipeline, PrevName, Program, ProjItem, ReassignTarget, ReleaseAtom, ReleaseCond,
    ReleaseDecl, ReleaseEffect, ReleaseMode, RoutineDecl, RoutineNotify, RoutineTrigger, ScalarTy,
    SecretDecl, SortKey, Source, SourceArg, Step, TakeCount, WriteStmt, DIVISION_SCALE,
};
pub use declparse::{
    bare_money_sites, parse_pipeline, parse_program, parse_whole, AggShape, AGGREGATE_NAMES,
    AGGREGATE_SHAPES,
};
pub use lexer::{lex, LexError};
pub use parser::{
    function_names, functions, parse_condition, parse_expr, ParseError, Parser, ALLOCATE_MODES,
    BARE_FIELD_IN_GROUP, DUPLICATE_AUTH, DUPLICATE_EDITION, DUPLICATE_RELEASE, EXTERNAL_ACL,
    EXTERNAL_COMPOSITE_KEY, EXTERNAL_NOT_SHARED, EXTERNAL_OWNED, EXTERNAL_READ_ONLY,
    EXTERNAL_SEALED, EXTERNAL_UNSUPPORTED, GRANT_BLOCK, INVALID_GUARD_WALK, INVALID_SECRET_HOST,
    PARSE_ERROR, PEER_BLOCK, PEER_NAME, PREV_DUPLICATE, PREV_SELF, PREV_STILL_DECLARED,
    READER_LINE, RELEASE_SUBJECT,
};
pub use span::Span;
pub use text::{is_nfc, nfc};
pub use token::{Token, TokenKind};
pub use trigger::{
    EventKind, KindPos, ListenerPattern, Modifiers, Pos, Schedule, UNKNOWN_EVENT_KIND,
};
