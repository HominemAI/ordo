//! Checklist 9c, last mile: a division by zero must not reach a caller as
//! `internal`.
//!
//! `definition-language.md` §2 settles that `/ 0`, `mod(a, 0)` and
//! `int`/`money` overflow are each "a typed runtime error" and names no wire
//! class. **RULED (9w, `corpus-rulings.md`)**: one class, `arithmetic_error`,
//! 400 for all three, now in `api-engine-design.md`'s closed inventory. One
//! class because no repair for an overflow is not also the repair for a
//! division by zero; 400 not as an attribution of fault but as *this request
//! has no answer, pose a different one* — the engine cannot tell a
//! caller-supplied operand from a stored one, and would be wrong about it in
//! the interesting cases. What was never in doubt is the failure mode this
//! replaces: 500
//! `internal` tells a caller the server broke, when the arithmetic merely has
//! no answer, and the rules that ARE built become indistinguishable from a
//! server bug.
//!
//! The chain under test, end to end and without a database:
//!
//!   Postgres SQLSTATE 22012/22003
//!     → `ordo_exec::PoolError::Arithmetic` (its Display is class-prefixed)
//!     → `FixtureExecutor::classify`
//!     → `ExecError::Arithmetic`
//!     → `ErrorCode::ArithmeticError`, 400, on the wire.

use axum::http::StatusCode;
use ordo_api::error::{ApiError, ErrorCode};

/// The link that used to be missing. `classify` maps the write protocol's
/// class-prefixed strings onto the wire inventory, and anything it does not
/// recognise becomes `internal` — so an unmapped class is not a cosmetic
/// gap, it is a 500.
#[test]
fn a_typed_arithmetic_failure_is_classified_rather_than_swallowed() {
    // `ExecError` is now a { code, message } struct rather than an enum
    // restating the wire inventory — a refusal whose class has no wire
    // code cannot be constructed at all.
    let e = ordo_host::classify("arithmetic_error: division by zero".into());
    assert_eq!(e.code, ErrorCode::ArithmeticError);
    assert_eq!(e.message, "division by zero");

    // …and it is NOT internal, which is the whole point.
    assert_ne!(e.code, ErrorCode::Internal);
}

/// The wire answer: a named class and a status that is not 500.
#[test]
fn the_wire_answer_is_a_named_class_and_not_a_server_error() {
    assert_eq!(ErrorCode::ArithmeticError.as_str(), "arithmetic_error");
    assert_eq!(ErrorCode::ArithmeticError.status(), StatusCode::BAD_REQUEST);
    assert_ne!(
        ErrorCode::ArithmeticError.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );

    let api = ApiError::new(ErrorCode::ArithmeticError, "division by zero");
    let env = api.envelope();
    assert_eq!(env["error"]["code"], "arithmetic_error");

    // Not a replayable executor outcome: nothing was applied, and a retry
    // over changed data may well succeed.
    assert!(!ErrorCode::ArithmeticError.is_exec_conflict());
}

/// An unrecognised class still falls through to `internal` — the mapping is
/// a narrowing, not a rename of the default.
#[test]
fn an_unknown_class_still_falls_through_to_internal() {
    let e = ordo_host::classify("something_else: boom".into());
    assert_eq!(e.code, ErrorCode::Internal, "{e:?}");
}

/// A data exception the database raised for the caller's value (a
/// `matches` pattern above all) reaches the caller as `invalid_param`, a
/// 400, through the same mapper, and never as `internal`.
#[test]
fn a_data_exception_is_invalid_param() {
    let text = ordo_exec::invalid_input_text("2201B", Some("invalid backreference number"));
    let e = ordo_host::classify(format!("invalid_param: {text}"));
    assert_eq!(e.code, ErrorCode::InvalidParam, "{e:?}");
    assert_eq!(e.code.status(), StatusCode::BAD_REQUEST);
    assert!(!e.message.contains("db:") && !e.message.contains("ERROR"));
}
