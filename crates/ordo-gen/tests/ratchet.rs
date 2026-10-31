//! **Ratchet 1: the exhaustive match, and the proof that it stays one.**
//!
//! `spell.rs` decides the TypeScript spelling of every closed vocabulary the
//! language has, matching each enum with one arm per variant and no catch-all.
//! Rust's own exhaustiveness check is what makes a new variant fail the build
//! — but only while the arms stay written that way. A single `_ => …` added
//! by someone in a hurry would restore the silent default and nothing would
//! notice, because every existing test would still pass.
//!
//! So this file checks the property mechanically, in two directions:
//!
//! 1. **No wildcard arm exists** in the files that own the decisions
//!    (`spell.rs`, and the AST walk in `emit.rs`). Source-level, because that
//!    is where the property lives — a wildcard is not observable from
//!    outside; it is observable only as an absence years later.
//! 2. **Every variant of every closed enum is actually decided**, by calling
//!    the decision function for each member of each producer's own `ALL` (or
//!    the written-out variant list, for `ScalarTy`, which cannot have one).
//!    A function could match exhaustively and still be unreachable; this is
//!    what makes the manifest a fact rather than a claim.
//!
//! # The three mutations that were run
//!
//! `editor_grammar.rs` records five mutations it was checked against; these
//! are the equivalent record, each applied to the PRODUCER, observed, and
//! reverted:
//!
//! | Mutation | What failed |
//! | --- | --- |
//! | `Keyword::Foo => "foo"` in `token.rs` | `error[E0004]: non-exhaustive patterns: Keyword::Foo not covered --> crates/ordo-gen/src/spell.rs:117:11` |
//! | `ScalarTy::Blob` in `decl.rs` | `error[E0004]: … ScalarTy::Blob not covered` — in [`spell::scalar`], the arm that decides a type's constructor and its wire shape |
//! | `CmpOp::Spaceship` in `ast.rs` | two `E0004`s — [`spell::cmp`] and the emitter's condition walk |
//!
//! Each is `cargo build`, not `cargo test`: the generator does not compile,
//! so `ordo gen ts` cannot run, so **neither TypeScript package can be
//! regenerated** until somebody writes down what the new word is called. That
//! is the ratchet — it fires before any test does, and it cannot be skipped
//! by someone who did not run the suite.
//!
//! A compile-fail harness (`trybuild`) would automate the three and was not
//! added: it would put a dev-dependency and a set of toolchain-version-
//! sensitive expected-output fixtures into a crate whose whole argument is
//! that it has almost no dependencies, to check a property `rustc` already
//! checks. What `rustc` does *not* check is that the arms stay wildcard-free,
//! and that is what test 1 above is for.

use std::collections::BTreeSet;

use ordo_gen::spell::{self, Spelling};
use ordo_syntax::decl::ScalarTy;
use ordo_syntax::token::{DurationUnit, Keyword};
use ordo_syntax::trigger::EventKind;
use ordo_syntax::{functions, AGGREGATE_NAMES, AGGREGATE_SHAPES, ALLOCATE_MODES};

const SPELL_SRC: &str = include_str!("../src/spell.rs");
const EMIT_SRC: &str = include_str!("../src/emit.rs");

/// The wildcard forms a match arm can take. `_ =>` is the plain one; `_ if`
/// is the guarded one, which is just as silent; `.. =>` is the struct form.
const WILDCARDS: &[&str] = &["_ =>", "_ if ", "_=>"];

/// A line that is a comment, a string, or a doc line does not declare an arm.
///
/// Crude on purpose: the point is to be *loud*, and a false positive costs a
/// rewritten comment while a false negative costs the ratchet.
fn is_prose(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with("/*") || t.starts_with('*')
}

#[test]
fn the_decision_files_have_no_wildcard_arm() {
    let mut found = Vec::new();
    for (file, src) in [("src/spell.rs", SPELL_SRC), ("src/emit.rs", EMIT_SRC)] {
        for (i, line) in src.lines().enumerate() {
            if is_prose(line) {
                continue;
            }
            for w in WILDCARDS {
                if line.contains(w) {
                    found.push(format!("{file}:{}: {}", i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "a wildcard match arm in a file that decides TypeScript spellings:\n  {}\n\n\
         These files exist to make a new keyword, scalar type or AST node FAIL THE \
         BUILD until someone decides what it is called in TypeScript. A `_ =>` arm \
         gives it a silent default instead, both TypeScript packages keep \
         compiling, and the author of the new word never learns the packages \
         exist. Write the arm out, even if the answer is \
         `Spelling::Absent(Absent::DoesNotParse(\"…\"))` — that is a legal and \
         common answer, and it is published in `vocabulary.ts`.",
        found.join("\n  ")
    );
}

/// Every keyword is decided, and the decision is not empty prose.
#[test]
fn every_keyword_has_a_decision() {
    for &k in Keyword::ALL {
        let sp = spell::keyword(k);
        assert!(
            !sp.note().trim().is_empty(),
            "`{}` has an empty note; an absent word owes a reason and an offered \
             one owes its name",
            k.as_str()
        );
        if let Spelling::Absent(_) = sp {
            assert!(
                sp.note().len() > 20,
                "`{}` is absent from the builder with a one-word excuse. The \
                 reason is the whole value of recording it: a reader has to be \
                 able to tell \"the LANGUAGE refuses this\" from \"we did not \
                 get to it\".",
                k.as_str()
            );
        }
    }
}

/// The manifest is complete by construction, so this checks the thing
/// construction cannot: that two words do not claim one spelling.
///
/// A collision would mean two methods with one name — the second silently
/// overwriting the first in the emitted class — which is the one way a
/// generated vocabulary can lose a word without anything failing.
#[test]
fn no_two_words_claim_the_same_spelling() {
    let mut seen: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for &k in Keyword::ALL {
        if let Some(name) = spell::keyword(k).offered() {
            seen.entry(name).or_default().push(k.as_str());
        }
    }
    let clashes: Vec<String> = seen
        .iter()
        .filter(|(_, words)| words.len() > 1)
        .map(|(name, words)| format!("`{name}` claimed by {}", words.join(" and ")))
        .collect();
    assert!(
        clashes.is_empty(),
        "two reserved words share a TypeScript spelling: {}",
        clashes.join("; ")
    );
}

/// Every scalar type is decided, `decimal` is decided to have no constructor,
/// and every other one has one.
#[test]
fn the_scalar_table_is_complete_and_decimal_has_no_constructor() {
    let all = spell::all_scalars();
    // `all_scalars` IS `ScalarTy::ALL` now, so this asserts the projection
    // is total rather than that a second list was kept in step.
    //
    // It used to carry its own written-out set here, under a comment
    // saying `ScalarTy` could not have a `const ALL` because `Duration`
    // carries a unit. That was true of the `word_enum!` macro and not of
    // the enum — `Duration(DurationUnit::Ms)` is a fine const expression —
    // and the missing `ALL` was why `editors/ordo.tmLanguage.json`'s type
    // list was the one inventory with no parity test.
    let names: BTreeSet<&str> = all.iter().map(|(t, _)| t.as_str()).collect();
    let from_enum: BTreeSet<&str> = ScalarTy::ALL.iter().map(|t| t.as_str()).collect();
    assert_eq!(
        names, from_enum,
        "the scalar table does not cover the language's type set"
    );
    assert!(
        names.contains("blob") && names.contains("uri"),
        "the content types must reach TypeScript: without a `blob` in the \
         inventory a generated form cannot offer a file input, which is the \
         gap `ui-gen.md` §1 pins by tying the hint vocabulary to the types"
    );

    // `decimal(p)` is declarable since 27ak, so it has a constructor like
    // every other type, and its value is a string for money's reason.
    assert_eq!(spell::scalar(ScalarTy::Decimal(4)).ctor, Some("decimal"));
    assert_eq!(spell::scalar(ScalarTy::Decimal(4)).ts, "string");
    for (t, sc) in &all {
        assert!(
            sc.ctor.is_some(),
            "`{}` has a declaration form in the language and no constructor here",
            t.as_str()
        );
        assert!(!sc.ts.is_empty() && !sc.wire.is_empty());
    }
}

/// Money is a string, and that is a decision worth its own test.
///
/// §B3 of the plan proposed "bigint-minor-units in TS" before the wire was
/// settled; `api-engine-design.md` §5 settles it as an exact decimal string,
/// and `params[].max` is a decimal digit string in the descriptor. A client
/// that parsed money into a bigint would be choosing a second representation
/// for the value it exists to carry unchanged — and would round it on the way
/// back out.
#[test]
fn money_reaches_typescript_as_an_exact_string() {
    let money = spell::scalar(ScalarTy::Money(2));
    assert_eq!(money.ts, "string");
    assert!(money.wire.contains("never a float"));
}

/// Every operator in every operator enum has a distinct method name.
#[test]
fn operator_methods_are_distinct() {
    let mut names = BTreeSet::new();
    for op in spell::ALL_CMP {
        assert!(
            names.insert(spell::cmp(*op)),
            "duplicate: {}",
            spell::cmp(*op)
        );
    }
    for op in spell::ALL_TEXT_OPS {
        assert!(names.insert(spell::text_op(*op)));
    }
    for op in spell::ALL_BIN_OPS {
        assert!(names.insert(spell::bin_op(*op)));
    }
    assert_eq!(names.len(), 6 + 3 + 4);
}

/// The written-out operator lists are the enums, both ways.
///
/// `CmpOp` and friends have no `ALL` of their own, so `spell.rs` writes the
/// list out; the exhaustive matches beside it are what make an omission
/// visible, and this is the check that the list itself did not go short.
#[test]
fn the_operator_lists_are_the_operator_sets() {
    let spelled: BTreeSet<&str> = spell::ALL_CMP.iter().map(|o| o.as_str()).collect();
    assert_eq!(
        spelled,
        BTreeSet::from(["==", "!=", "<", "<=", ">", ">="]),
        "the comparison operator list is not the language's"
    );
    let text: BTreeSet<&str> = spell::ALL_TEXT_OPS.iter().map(|o| o.as_str()).collect();
    assert_eq!(text, BTreeSet::from(["contains", "starts", "matches"]));
    let arith: BTreeSet<&str> = spell::ALL_BIN_OPS.iter().map(|o| o.as_str()).collect();
    assert_eq!(arith, BTreeSet::from(["+", "-", "*", "/"]));
}

/// Duration units pass through unchanged, and every one is acknowledged.
#[test]
fn every_duration_unit_is_acknowledged() {
    for u in DurationUnit::ALL {
        assert_eq!(
            spell::duration_unit(*u),
            u.as_str(),
            "a duration unit's TypeScript spelling is its own word; the match \
             exists so a new one lands here as a compile error, not so it can \
             be renamed"
        );
    }
}

/// Every event kind is decided, and today every decision is the same one.
#[test]
fn every_event_kind_is_decided_and_none_is_offered() {
    for k in EventKind::ALL {
        let sp = spell::event_kind(*k);
        assert!(
            sp.offered().is_none(),
            "`{}` is offered by the builder, but no trigger declaration form \
             parses — `parse_program` accepts domain/entity/op and nothing \
             else. A builder that offered it could express what no `.ordo` \
             file can, which is the isomorphism property inverted.",
            k.as_str()
        );
        assert!(matches!(sp, Spelling::Absent(_)));
    }
}

// ── the tables the generator projects rather than restates ───────────────

/// `AGGREGATE_SHAPES` and `AGGREGATE_NAMES` are two projections of one list,
/// so this can only fail if the macro is bypassed.
///
/// Cheap, and worth having: `ordo-gen` generates one `agg.*` method per row of
/// `AGGREGATE_SHAPES`, and the editor grammar pins its palette to
/// `AGGREGATE_NAMES`. A divergence would be a highlighted word with no builder
/// method, or the reverse.
#[test]
fn the_aggregate_projections_agree() {
    let names: Vec<&str> = AGGREGATE_NAMES.to_vec();
    let shaped: Vec<&str> = AGGREGATE_SHAPES.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, shaped);
}

/// The generator never invents a function name or an arity.
#[test]
fn the_function_table_is_the_parsers() {
    let n = functions().count();
    assert!(n > 0);
    for (name, min, max) in functions() {
        assert!(!name.is_empty());
        assert!(min <= max, "`{name}` has an inverted arity range");
    }
    // `allocate`'s third argument is a mode word rather than an expression,
    // which the generator special-cases; if the arity ever stops admitting a
    // third argument, the special case is dead code claiming to be a rule.
    let allocate = functions().find(|(n, _, _)| *n == "allocate");
    assert_eq!(
        allocate,
        Some(("allocate", 2, 3)),
        "`allocate`'s arity moved; the CDK types its third argument as the \
         closed `AllocateMode` union and that special case is written against \
         this shape"
    );
    assert!(!ALLOCATE_MODES.is_empty());
}
