//! **Ratchet 2: isomorphism over the corpus.**
//!
//! Track C's bar, verbatim: *"anything expressible in the builder is
//! expressible in `.ordo` and vice versa (isomorphism is a conformance-tested
//! property, not an aspiration)"*. This is the test.
//!
//! # The loop, and where each leg runs
//!
//! For every corpus program:
//!
//! | Leg | Runs in | What it does |
//! | --- | --- | --- |
//! | A | `cargo test` (this file) | parse the original; machine-translate the AST into builder calls; compare against the checked-in `test/corpus/<name>.gen.ts` |
//! | B | `npm run check` in `clients/ordo-cdk` | import that module, call `synth()`, compare against the checked-in `test/corpus/<name>.synth.ordo` |
//! | C | `cargo test` (this file) | parse `<name>.synth.ordo` and assert the AST equals the original's, spans aside |
//!
//! # Why the intermediate is checked in
//!
//! Leg B needs a JavaScript runtime and `cargo test --workspace` must stay
//! green on a machine with no node. The two ways to reconcile that are a
//! Rust test that shells out to node and skips when it is absent — which
//! means the property is *unverified* on exactly the machines that skip, and
//! nobody notices — or an intermediate artifact that both sides check.
//!
//! This takes the second. `<name>.synth.ordo` is node's output, checked in;
//! vitest asserts `synth()` still produces it, and **cargo asserts it parses
//! back to the original tree**. So on a node-less machine the round trip is
//! still checked end to end against real synthesized text; what is not
//! rechecked is that today's TypeScript still emits that text, and the
//! moment anything upstream changes, leg A's golden moves and the build
//! fails until both are regenerated. A stale `.synth.ordo` cannot hide
//! behind an unchanged `.gen.ts`, because the `.gen.ts` is a pure function
//! of the same parse.
//!
//! # "Canonical", honestly
//!
//! The plan says `synth` emits canonical `.ordo` *via `ordo fmt`*. There is
//! no `ordo fmt` — Track A1 is unbuilt — so the comparison here is
//! **parse-and-reparse equality**: `parse(original)` and
//! `parse(synth(emit(parse(original))))` must be the same AST, spans
//! excepted. That is weaker than byte equality and stronger than nothing,
//! and it is the claim actually checked. When `ordo fmt` lands it becomes
//! the normalizer on both sides and the claim tightens to bytes.
//!
//! # Regenerating
//!
//! ```text
//! ORDO_UPDATE_GOLDEN=1 cargo test -p ordo-gen        # legs A and the .ordo snapshots
//! cd clients/ordo-cdk && npm run gen:corpus          # leg B's .synth.ordo
//! ORDO_UPDATE_GOLDEN=1 cargo test -p ordo-gen        # leg C re-checks them
//! ```

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ordo_gen::emit::emit_program;
use ordo_syntax::{parse_program, Program};

const SYNTAX: &str = include_str!("../../../docs/syntax-examples.md");
const MARKET: &str = include_str!("../../../crates/ordo-conformance/fixtures/market.ordo");
const WIDE: &str = include_str!("../../../crates/ordo-conformance/fixtures/wide.ordo");

/// Where the emitted fixtures live: inside the package that runs leg B.
fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .join("clients/ordo-cdk/test/corpus")
}

/// The module specifier a fixture imports the package under. A relative path,
/// because the fixtures live inside the package and importing it by name
/// would need a workspace link that this repository does not have.
const IMPORT_FROM: &str = "../../src/index.js";

const HOW: &str = "\n\nregenerate with:\n  \
                   ORDO_UPDATE_GOLDEN=1 cargo test -p ordo-gen\n  \
                   (cd clients/ordo-cdk && npm run gen:corpus)\n  \
                   ORDO_UPDATE_GOLDEN=1 cargo test -p ordo-gen\n\
                   then READ the diff — a change here is a change to what the \
                   builder can say.";

fn golden(path: &Path, actual: &str) -> Result<(), String> {
    if std::env::var_os("ORDO_UPDATE_GOLDEN").is_some() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(path, actual).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(());
    }
    match std::fs::read_to_string(path) {
        Ok(expected) if expected == actual => Ok(()),
        Ok(_) => Err(format!(
            "{} differs from what the generator emits",
            path.display()
        )),
        Err(_) => Err(format!("{} is missing", path.display())),
    }
}

// ── the corpus ───────────────────────────────────────────────────────────

struct Candidate {
    name: String,
    /// The exact text handed to `parse_program`, wrapper included.
    source: String,
}

/// Fenced blocks, the way `docs_parse.rs` reads them: bare ``` fences are
/// candidates in this file, and a `ordo`-tagged one always is.
fn blocks(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut open: Option<(usize, Vec<&str>, bool)> = None;
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("```") {
            match open.take() {
                None => {
                    let candidate = rest == "ordo" || rest.is_empty();
                    open = Some((i + 2, Vec::new(), candidate));
                }
                Some((start, body, keep)) => {
                    if keep {
                        out.push((start, body.join("\n")));
                    }
                }
            }
        } else if let Some((_, body, _)) = open.as_mut() {
            body.push(line);
        }
    }
    out
}

/// The first word that is neither blank nor a comment.
fn lead_word(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("")
}

const DECL_HEADS: &[&str] = &["domain", "entity", "op", "routine"];

/// Every corpus candidate: the two conformance fixtures, and every fenced
/// block of `syntax-examples.md` that `parse_program` accepts — the same set
/// `docs_parse.rs` walks, minus the fragments it wraps differently.
///
/// A block that is a bare pipeline is given the `op` wrapper `docs_parse.rs`
/// gives it, so the corpus reaches the pipeline grammar and not only the
/// declaration grammar.
fn candidates() -> Vec<Candidate> {
    let mut out = vec![
        Candidate {
            name: "fixture-market".into(),
            source: MARKET.to_string(),
        },
        Candidate {
            name: "fixture-wide".into(),
            source: WIDE.to_string(),
        },
    ];
    for (line, text) in blocks(SYNTAX) {
        let head = lead_word(&text);
        let source = if DECL_HEADS.contains(&head) {
            text.clone()
        } else if head.is_empty() || head == "whole" {
            // `whole` is a special form, not a pipeline, and it is out of the
            // builder's scope for this increment (see the crate README). A
            // comment-only block has no program in it.
            continue;
        } else {
            format!("op docBlock =\n{text}")
        };
        out.push(Candidate {
            name: format!("syntax-{line:04}"),
            source,
        });
    }
    out
}

// ── span-insensitive AST comparison ──────────────────────────────────────

/// A pretty-printed AST with every `span: Span { … }` block removed.
///
/// Spans are byte offsets into the source, so the original and the
/// synthesized text can never share them — the trees are equal in every way
/// that is *about the program* and differ in every way that is about its
/// layout, which is exactly the distinction this test is making.
///
/// Line-based rather than a recursive rewrite because `Span` has one Debug
/// shape and there are twenty AST types that carry one; a rewrite would be
/// twenty visitors that could each forget a field, and forgetting one would
/// make the test *pass* more often.
fn strip_spans(program: &Program) -> String {
    let text = format!("{program:#?}");
    let mut out = String::new();
    let mut skip_until: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        if let Some(depth) = skip_until {
            // The block closes at the same indentation it opened at.
            if indent == depth && line.trim_start().starts_with('}') {
                skip_until = None;
            }
            continue;
        }
        let t = line.trim_start();
        // `trigger_span` is a span too (a routine's trigger clause).
        if t.starts_with("span: Span {") || t.starts_with("trigger_span: Span {") {
            skip_until = Some(indent);
            continue;
        }
        // The one-line form, for a `Span` printed inline.
        if t.starts_with("span: Span(") || t.starts_with("trigger_span: Span(") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The stripper is load-bearing, so it is checked rather than trusted: two
/// parses of the same program that differ only in layout must compare equal,
/// and two different programs must not.
#[test]
fn the_span_stripper_removes_layout_and_nothing_else() {
    let a = parse_program("op x =\n  orders[total]").expect("a");
    let b = parse_program("\n\nop x =\n      orders[total]\n").expect("b");
    assert_eq!(
        strip_spans(&a),
        strip_spans(&b),
        "the stripper leaves layout in, so every comparison below is really \
         comparing byte offsets"
    );
    assert!(
        !strip_spans(&a).contains("Span"),
        "a span survived the strip"
    );

    let c = parse_program("op x =\n  orders[placed]").expect("c");
    assert_ne!(
        strip_spans(&a),
        strip_spans(&c),
        "the stripper removes more than spans, so it would hide a real \
         difference between the original and the synthesized program"
    );
}

// ── the corpus round trip ────────────────────────────────────────────────

#[test]
fn the_corpus_round_trips_through_the_builder() {
    let dir = corpus_dir();
    let mut emitted: BTreeSet<String> = BTreeSet::new();
    let mut round_tripped = 0usize;
    let mut attempted = 0usize;
    let mut misses: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for c in candidates() {
        let Ok(original) = parse_program(&c.source) else {
            // A block `parse_program` refuses is not a corpus member: it is
            // `docs_parse.rs`'s allow-list, which owns the reason.
            continue;
        };
        attempted += 1;

        // ── leg A ──
        let ts = match emit_program(&original, IMPORT_FROM) {
            Ok(ts) => ts,
            Err(why) => {
                // A node kind the builder has no spelling for. Named, never
                // skipped: that is the whole difference between a corpus and
                // a demo.
                misses.push(format!("{}: {why}", c.name));
                continue;
            }
        };
        emitted.insert(format!("{}.gen.ts", c.name));
        emitted.insert(format!("{}.ordo", c.name));
        if let Err(e) = golden(&dir.join(format!("{}.gen.ts", c.name)), &ts) {
            failures.push(format!("{}: {e}", c.name));
        }
        // The original, beside the emitted module, so a reviewer of the
        // fixture directory can read both halves of the claim.
        if let Err(e) = golden(&dir.join(format!("{}.ordo", c.name)), &c.source) {
            failures.push(format!("{}: {e}", c.name));
        }

        // ── leg C ──
        let synth_path = dir.join(format!("{}.synth.ordo", c.name));
        let Ok(synthesized) = std::fs::read_to_string(&synth_path) else {
            failures.push(format!(
                "{}: {} is missing — leg B has not run",
                c.name,
                synth_path.display()
            ));
            continue;
        };
        match parse_program(&synthesized) {
            Err(e) => failures.push(format!(
                "{}: the builder synthesized text the parser refuses — {e}\n\
                 This is the isomorphism property failing in the direction that \
                 matters most: the builder can say something `.ordo` cannot.\n\
                 --- synthesized ---\n{synthesized}",
                c.name
            )),
            Ok(reparsed) => {
                let before = strip_spans(&original);
                let after = strip_spans(&reparsed);
                if before == after {
                    round_tripped += 1;
                } else {
                    failures.push(format!(
                        "{}: the synthesized program parses, but to a DIFFERENT tree.\n\
                         --- original ---\n{}\n--- synthesized ---\n{}\n\
                         --- first differing line ---\n{}",
                        c.name,
                        c.source.trim(),
                        synthesized.trim(),
                        first_difference(&before, &after),
                    ));
                }
            }
        }
    }

    // A fixture nobody emits any more is a fixture that will rot. The
    // directory is a projection of the corpus, so an extra file in it is as
    // much a defect as a missing one.
    let mut stray = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".synth.ordo") {
                let base = name.trim_end_matches(".synth.ordo");
                if !emitted.contains(&format!("{base}.gen.ts")) {
                    stray.push(name);
                }
            } else if (name.ends_with(".gen.ts") || name.ends_with(".ordo"))
                && !emitted.contains(&name)
            {
                stray.push(name);
            }
        }
    }

    let report = format!(
        "\n\nisomorphism over the corpus: {round_tripped} of {attempted} programs \
         round-trip.\n\
         builder misses ({}):{}\n\
         stray fixtures ({}):{}\n\
         failures ({}):\n{}{HOW}",
        misses.len(),
        if misses.is_empty() {
            " (none)".to_string()
        } else {
            format!("\n  - {}", misses.join("\n  - "))
        },
        stray.len(),
        if stray.is_empty() {
            " (none)".to_string()
        } else {
            format!("\n  - {}", stray.join("\n  - "))
        },
        failures.len(),
        if failures.is_empty() {
            "  (none)\n".to_string()
        } else {
            failures
                .iter()
                .map(|f| format!("  - {f}\n"))
                .collect::<String>()
        },
    );

    assert!(failures.is_empty() && stray.is_empty(), "{report}");

    // A floor, so a refactor that made the corpus empty and a report that
    // said so could not both pass. It is a floor and not an equality because
    // adding a doc example must not fail this test — it must show up in the
    // score.
    assert!(
        round_tripped >= 15,
        "only {round_tripped} programs round-trip; the count must not go \
         backwards{report}"
    );

    // Every miss is a builder gap with a name. The list is asserted EMPTY:
    // the honest position for a package whose claim is isomorphism is that a
    // corpus program it cannot express is a bug, not a footnote. When one
    // appears, either the builder grows the spelling or the crate README
    // grows a stated exclusion and the program leaves the corpus by name.
    assert!(
        misses.is_empty(),
        "the builder cannot express {} corpus program(s){report}",
        misses.len()
    );
}

/// The first line where two pretty-printed trees diverge, with context.
fn first_difference(a: &str, b: &str) -> String {
    for (i, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            return format!("line {}:\n  original:    {x}\n  synthesized: {y}", i + 1);
        }
    }
    format!(
        "one tree is a prefix of the other ({} vs {} lines)",
        a.lines().count(),
        b.lines().count()
    )
}

/// The release rule (27aq) goes through the builder and back: the emitted
/// module names the release vocabulary, and the `.ordo` the builder
/// synthesized from it parses to the same tree as the documented block.
#[test]
fn cdk_release_emits_the_same_ir() {
    let (line, block) = blocks(SYNTAX)
        .into_iter()
        .find(|(_, b)| b.contains("release member when"))
        .expect("syntax-examples.md documents a release rule");
    let original = parse_program(&block).expect("the documented rule parses");
    assert!(original.release.is_some());
    let emitted = emit_program(&original, "../../src/index.js").expect("emits");
    assert!(
        emitted.contains("release.member.when(") && emitted.contains("rel.reassign("),
        "{emitted}"
    );
    let synth = std::fs::read_to_string(corpus_dir().join(format!("syntax-{line:04}.synth.ordo")))
        .expect("leg B synthesized the release block (npm run gen:corpus)");
    let back = parse_program(&synth).expect("the builder's text parses");
    assert_eq!(strip_spans(&original), strip_spans(&back));
}
