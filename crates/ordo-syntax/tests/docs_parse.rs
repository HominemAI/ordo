//! The docs must compile. Every fenced block in `docs/syntax-examples.md` and
//! `docs/tutorial.md` is extracted and pushed through the real parser.
//!
//! A block that does not parse must appear on `ALLOWED` with a one-line reason
//! in one of three categories, because they are owed differently (9e′):
//!
//! 1. **Settled and unbuilt** — the spec pins the construct and the parser does
//!    not have it. This is a debt; it belongs to 9e.
//! 2. **[lean], not settled** — the spec has *not* pinned the construct, so
//!    refusing it is the parser being right and there is nothing owed. Building
//!    it would mean inventing the shape the spec declined to fix.
//! 3. **Not Ordo source** — prose, metasyntax, a rendered error, a deliberate
//!    reject.
//!
//! Conflating 1 and 2 turns an unsettled sketch into a work item. The list is
//! checked in BOTH directions:
//! an unlisted failure fails the test, and a listed reason that no longer
//! matches any failing block fails it too. So this file always says exactly
//! what is still owed, and it cannot pass vacuously — if a block that parses
//! today stops parsing, no allow-list entry will claim it.

use ordo_syntax::{parse_pipeline, parse_program, parse_whole};

const SYNTAX: &str = include_str!("../../../docs/syntax-examples.md");
const TUTORIAL: &str = include_str!("../../../docs/tutorial.md");
// The how-to guides. Added because their examples were written by reading the
// fixtures and the module docs, and "I copied it from a real file" is a claim
// a test should check rather than a promise a reader has to take. A guide that
// teaches a spelling the grammar refuses is worse than no guide.
const HOWTO_CUSTODY: &str = include_str!("../../../docs/howto-custody.md");
const HOWTO_VALUES: &str = include_str!("../../../docs/howto-values.md");

struct Block {
    file: &'static str,
    /// 1-based line of the block's first content line, for the failure report.
    line: usize,
    text: String,
}

/// Fenced blocks. The docs use bare ``` fences with no language tag, so every
/// fenced block is a candidate and the non-Ordo ones (a JSON response, a
/// rendered error, a prose table) are named on `ALLOWED` like any other
/// exclusion — nothing is skipped silently.
fn blocks(file: &'static str, src: &str, tagged_only: bool) -> Vec<Block> {
    let mut out = Vec::new();
    let mut open: Option<(usize, Vec<&str>, bool)> = None;
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        // A fence opens with ``` and may carry a language tag; it closes with
        // a bare ```. Treating every ``` as a toggle worked while the docs
        // used bare fences only, and silently inverted the moment one carried
        // `rust` — the opener did not match, so its closer opened a block and
        // the file read as unbalanced. Tag-aware because the docs now tag.
        if let Some(rest) = t.strip_prefix("```") {
            match open.take() {
                // A tagged fence that is not `ordo` is not a candidate: a
                // Rust snippet or a JSON response has no business being fed
                // to the Ordo parser, and putting it on the allow-list would
                // be excusing a block nobody ever claimed was Ordo.
                None => {
                    // Two conventions, per file. `syntax-examples.md` and
                    // `tutorial.md` are all-Ordo documents written before
                    // fences carried tags, so a bare fence there is a
                    // candidate. The how-to guides are mostly OUTPUT —
                    // tables, shell transcripts, rendered errors — so a bare
                    // fence there is not a claim about the grammar, and
                    // treating it as one would fill the allow-list with
                    // entries excusing prose.
                    let candidate = rest == "ordo" || (!tagged_only && rest.is_empty());
                    open = Some((i + 2, Vec::new(), candidate));
                }
                Some((start, body, keep)) => {
                    if keep {
                        out.push(Block {
                            file,
                            line: start,
                            text: body.join("\n"),
                        });
                    }
                }
            }
        } else if let Some((_, body, _)) = open.as_mut() {
            body.push(line);
        }
    }
    assert!(open.is_none(), "{file}: unbalanced ``` fence");
    out
}

fn all_blocks() -> Vec<Block> {
    let mut v = blocks("docs/syntax-examples.md", SYNTAX, false);
    v.extend(blocks("docs/tutorial.md", TUTORIAL, false));
    v.extend(blocks("docs/howto-custody.md", HOWTO_CUSTODY, true));
    v.extend(blocks("docs/howto-values.md", HOWTO_VALUES, true));
    v
}

/// The first word that is neither blank nor a comment — what decides which
/// entry point a block belongs to.
fn lead_word(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("")
}

/// Words that can begin a declaration in the settled language. A block that
/// starts with one is a definition file; anything else is a fragment.
const DECL_HEADS: &[&str] = &[
    "domain",
    "entity",
    "op",
    "routine",
    "when",
    "every",
    "trigger",
    "principal",
    "app",
    "client",
    "policy",
    "channel",
    "route",
];

/// Parse a block the way the doc presents it: a definition file, standalone
/// `whole` reads, a bare pipeline, or a body fragment shown without its `op`
/// wrapper (which is then supplied).
fn parse_block(text: &str) -> Result<(), String> {
    match lead_word(text) {
        "" => Err("no Ordo source in the block".into()),
        // `whole` is not a pipeline and does not compose, so a block of them
        // is a block of independent statements, one per line.
        "whole" => {
            for line in text
                .lines()
                .filter(|l| l.trim_start().starts_with("whole "))
            {
                parse_whole(line).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        w if DECL_HEADS.contains(&w) => parse_program(text).map(|_| ()).map_err(|e| e.to_string()),
        _ => match parse_pipeline(text) {
            Ok(_) => Ok(()),
            // A write fragment (`set …` / `del …`) is only legal inside an op,
            // so give it one before calling it a failure.
            Err(pipe_err) => parse_program(&format!("op docBlock =\n{text}"))
                .map(|_| ())
                .map_err(|_| pipe_err.to_string()),
        },
    }
}

/// Blocks that legitimately do not parse: `(needle, reason)`. The needle is a
/// substring that identifies the block(s); the reason names the construct that
/// is owed, or says the block is not Ordo source.
const ALLOWED: &[(&str, &str)] = &[
    // ── 9e · constructs the spec settles and the parser does not have ──
    //
    // `unique (a, b)`, `index (…)` and now `check` all parse — the three
    // blocks that used to sit here came off when 9e's row invariants
    // landed (2026-09-05). What remains below is what is still owed.
    (
        "-> run meterAnomaly    debounce 5m",
        "12a — this is §7's top-level `when … -> run x` form. It is refused in \
         favour of the routine's own `trigger` clause (`routine r { trigger when \
         … run: … }`), which is the built spelling (checklist 12a, 2026-10-08)",
    ),
    // ── [lean], not settled: refusing these is the parser being RIGHT ──
    //
    // `definition-language.md` §8 opens with one sentence that governs every
    // declaration in it: **"All illustrative syntax [lean] — concepts settled,
    // file format not final."** A declaration form IS the file format, so
    // these five keywords' SEMANTICS are owed and their SPELLINGS are not —
    // the exact seam DL §3's Allocation entry has, read the other way round.
    // 9e′'s ruling applies unchanged: a [lean] construct is not owed, and
    // building one means inventing the shape the spec declined to fix.
    (
        "channel email { provider: resend }",
        "not owed — `channel` is under §8's [lean] spelling pin. Its semantics \
         are settled (\"Notification channels [settled]\", and outbound hooks are \
         a per-space granted sink) and there is no engine mechanism behind it \
         either: the only `channel` in this workspace is the LISTEN/NOTIFY \
         invalidation bus, a different sense of the word",
    ),
    (
        "route alerts.warning -> [push] fallback email",
        "not owed — same block, same §8 pin. `route` is not even a reserved \
         word in the lexer (it is contextual), which is the grammar agreeing \
         that the form is not fixed",
    ),
    (
        "principal simplefinHook",
        "not owed — `principal` is under §8's [lean] spelling pin. The mechanism \
         is fully built (`ordo-identity`: `Principal`, `PrincipalKind`, the \
         access engine, the directory); it is the DECLARATION that the spec \
         declines to fix",
    ),
    (
        "app plantcare {",
        "not owed, and the owning document says so in as many words. \
         `ui-gen.md` §5 is headed \"[mixed: binding grammar **normative**, \
         everything else illustrative]\" and lists block set, page composition \
         and the page-name/op-name namespacing as still open, then: \"a manifest \
         cannot be compiled against this section until they land\"",
    ),
    // …and the original [lean] pair, marked in §1 rather than §8. Building
    // one would mean inventing the unsettled shape it is defined against —
    // the mistake 9c declined to make with `allocate`, whose only legal
    // home is this one.
    (
        "$lines: list of {",
        "not owed — `list of { … }` + `each` over a list parameter is [lean] \
         (definition-language.md §1), and the doc block marks itself `# [lean]`",
    ),
    (
        "$split: list of {",
        "not owed — `list of { … }` + `each` over a list parameter is [lean] \
         (definition-language.md §1), and the doc block says so in its own comment",
    ),
    // ── not Ordo source ──
    (
        "owns: leaseParties · people owns: leaseParties",
        "not Ordo — a prose ownership map",
    ),
    (
        "# BOOTSTRAP, all platform-derived",
        "not Ordo — a comment-only block (the acl bootstrap rules in prose)",
    ),
    (
        "entity(conditions)[projection]",
        "not Ordo — grammar metasyntax, the read shape spelled abstractly",
    ),
    (
        "let <name> = <pipeline>",
        "not Ordo — grammar metasyntax for the `let` form",
    ),
    (
        "op name($params) flags = body",
        "not Ordo — grammar metasyntax for the op wrapper",
    ),
    (
        "schema error [incomplete_access_clause]",
        "not Ordo — a rendered compiler error",
    ),
    (
        "# ✗ not in a condition",
        "not Ordo — three spellings shown deliberately as rejects",
    ),
];

#[test]
fn every_doc_block_parses_or_is_owed() {
    let mut failures = Vec::new();
    let mut parsed = 0;
    for b in all_blocks() {
        match parse_block(&b.text) {
            Ok(()) => parsed += 1,
            Err(e) => failures.push((b, e)),
        }
    }

    // 1. Every failure is claimed by an allow-list entry.
    let mut unclaimed = String::new();
    for (b, e) in &failures {
        if !ALLOWED.iter().any(|(needle, _)| b.text.contains(needle)) {
            unclaimed.push_str(&format!(
                "\n  {}:{} — {e}\n    first line: {}",
                b.file,
                b.line,
                b.text.lines().next().unwrap_or("").trim()
            ));
        }
    }

    // 2. Every allow-list entry still claims a failure. An entry whose block
    //    now parses (or whose text left the docs) is stale and must go, or the
    //    list would quietly grow into an excuse.
    let mut stale = String::new();
    for (needle, reason) in ALLOWED {
        if !failures.iter().any(|(b, _)| b.text.contains(needle)) {
            stale.push_str(&format!("\n  {needle:?} — {reason}"));
        }
    }

    assert!(
        unclaimed.is_empty() && stale.is_empty(),
        "{parsed} of {} doc blocks parse.\
         \n\nblocks that fail to parse and are NOT on the allow-list:{}\
         \n\nallow-list entries that no longer match a failing block \
         (they parse now, or the doc changed — remove them):{}",
        parsed + failures.len(),
        if unclaimed.is_empty() {
            " (none)".into()
        } else {
            unclaimed
        },
        if stale.is_empty() {
            " (none)".into()
        } else {
            stale
        },
    );

    // A floor, so a refactor that made `parse_block` reject everything and an
    // allow-list that grew to match could not both pass.
    // 57 as of 9e: `unique (a, b)`, `index (…)` and the write-valued `let`
    // took it there from 52.
    assert!(
        parsed >= 63,
        "only {parsed} doc blocks parse; the count must not go backwards"
    );
}
