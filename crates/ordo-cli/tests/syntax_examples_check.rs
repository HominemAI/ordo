//! `docs/syntax-examples.md` must compile, not only parse.
//!
//! `ordo-syntax`'s `docs_parse` test pushes every block through the parser. A
//! block can parse and still be refused by the checker (a default the engine
//! does not build, a join with no path, a missing required field), and a first
//! reader copies the block they see. This test runs the checker on every
//! block that parses and is not marked planned.
//!
//! How a block is checked:
//!
//! - The file is one domain, `estate`. Its first section (before `## 2.`) is
//!   the ontology, and every later block is checked against it.
//! - A bare pipeline gets an `op docBlock =` wrapper, as `docs_parse` does.
//! - An entity, op or routine in a block replaces the same name in the
//!   program. Entities a block declares carry on into the blocks after it, so
//!   section 8's `acl` on `properties` is there for the guards below it.
//! - A block starting with `domain`, or whose first line is `# standalone`,
//!   is a program of its own.
//! - A block whose first line is `# planned:` names a construct that is not
//!   built. It is skipped, and this file counts how many there are, so a new
//!   one is a visible choice.
//! - A block that does not parse is owed to `docs_parse`'s allow-list, not
//!   to this test.

use ordo_cli::Sources;
use ordo_syntax::{parse_pipeline, parse_program};

const SYNTAX: &str = include_str!("../../../docs/syntax-examples.md");

const DECL_HEADS: &[&str] = &["domain", "entity", "op", "routine"];

struct Block {
    line: usize,
    text: String,
}

fn blocks(src: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut open: Option<(usize, Vec<&str>)> = None;
    for (i, line) in src.lines().enumerate() {
        if line.trim().starts_with("```") {
            match open.take() {
                None => open = Some((i + 2, Vec::new())),
                Some((start, body)) => out.push(Block {
                    line: start,
                    text: body.join("\n"),
                }),
            }
        } else if let Some((_, body)) = open.as_mut() {
            body.push(line);
        }
    }
    out
}

fn lead_word(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("")
}

fn first_line_is(text: &str, prefix: &str) -> bool {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .is_some_and(|l| l.starts_with(prefix))
}

fn planned(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .take_while(|l| l.starts_with('#') || l.is_empty())
        .any(|l| l.starts_with("# planned"))
}

/// Does the block parse the way `docs_parse` reads it? The ones that do not
/// are that test's business.
fn parses(text: &str) -> bool {
    match lead_word(text) {
        "" | "whole" => false,
        w if [
            "domain", "entity", "op", "routine", "when", "every", "trigger",
        ]
        .contains(&w) =>
        {
            parse_program(text).is_ok()
        }
        _ => {
            parse_pipeline(text).is_ok() || parse_program(&format!("op docBlock =\n{text}")).is_ok()
        }
    }
}

/// `(kind name, text)` for each top-level declaration, in order. The text of a
/// declaration runs to the next one, so comments and `release` clauses travel
/// with the declaration before them.
fn split(src: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, Vec<&str>)> = vec![(String::new(), Vec::new())];
    for line in src.lines() {
        let mut words = line.split_whitespace();
        let head = words.next().unwrap_or("");
        let at_margin = !line.starts_with(char::is_whitespace);
        if at_margin && DECL_HEADS.contains(&head) {
            let name = words
                .next()
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric());
            out.push((format!("{head} {name}"), vec![line]));
        } else {
            out.last_mut().expect("seeded").1.push(line);
        }
    }
    out.into_iter().map(|(n, l)| (n, l.join("\n"))).collect()
}

fn merge(base: &str, block: &str) -> String {
    let theirs = split(block);
    let names: Vec<&String> = theirs
        .iter()
        .map(|(n, _)| n)
        .filter(|n| !n.is_empty())
        .collect();
    let has_domain = split(base).iter().any(|(n, _)| n.starts_with("domain "));
    let mut parts: Vec<String> = split(base)
        .into_iter()
        .filter(|(n, _)| !names.contains(&n))
        .map(|(_, t)| t)
        .collect();
    parts.extend(
        theirs
            .into_iter()
            .filter(|(n, _)| !(n.starts_with("domain ") && has_domain))
            .map(|(_, t)| t),
    );
    parts.join("\n")
}

fn entities_only(block: &str) -> String {
    split(block)
        .into_iter()
        .filter(|(n, _)| n.starts_with("entity "))
        .map(|(_, t)| t)
        .collect::<Vec<_>>()
        .join("\n")
}

fn compile(program: &str) -> Result<(), String> {
    let src = Sources::from_parts(vec![(String::new(), program.to_string())]);
    let failed = ordo_cli::uncompiled_ops(&src)?;
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed
            .iter()
            .map(|(op, why)| format!("`{op}` does not compile [{why}]"))
            .collect::<Vec<_>>()
            .join("; "))
    }
}

/// The line section 2 starts on: blocks before it are the ontology.
fn ontology_end() -> usize {
    SYNTAX
        .lines()
        .position(|l| l.starts_with("## 2."))
        .expect("section 2 heading")
        + 1
}

/// The estate ontology: every declaration block before section 2.
fn ontology() -> String {
    let end = ontology_end();
    blocks(SYNTAX)
        .iter()
        .filter(|b| b.line < end)
        .filter(|b| DECL_HEADS.contains(&lead_word(&b.text)) && !planned(&b.text))
        .map(|b| b.text.clone())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Reported fix 25au (`check-path-search-slow`): a join with no path tried
/// every subset of the schema's refs before saying so, about 75 s in a debug
/// build on this ontology. It now answers at once, and still names the
/// sources it could not join.
#[test]
fn a_failing_path_search_on_the_estate_schema_finishes_quickly() {
    let program = merge(
        &ontology(),
        "op lonely =
  let byName = vendors[name] group name { n: count }
  vendors[name]  byName[n]",
    );
    let started = std::time::Instant::now();
    let e = compile(&program).expect_err("byName keeps no key back to vendors");
    let took = started.elapsed();
    assert!(
        took < std::time::Duration::from_secs(2),
        "the failing search took {took:?}"
    );
    assert!(e.contains("no_match_path"), "{e}");
    assert!(
        e.contains("`vendors`") && e.contains("`byName`"),
        "the refusal names the sources it could not join: {e}"
    );
}

#[test]
fn every_block_not_marked_planned_passes_ordo_check() {
    let all = blocks(SYNTAX);
    let ontology_end = ontology_end();

    let mut base = ontology();
    compile(&base).unwrap_or_else(|e| panic!("the ontology does not compile: {e}"));

    let mut checked = 0;
    let mut skipped_planned = 0;
    let mut failures = String::new();
    for b in all.iter().filter(|b| b.line >= ontology_end) {
        if planned(&b.text) {
            skipped_planned += 1;
            continue;
        }
        if !parses(&b.text) {
            continue;
        }
        let lead = lead_word(&b.text);
        let alone = lead == "domain" || first_line_is(&b.text, "# standalone");
        let wrapped = if DECL_HEADS.contains(&lead) {
            b.text.clone()
        } else {
            format!("op docBlock =\n{}", b.text)
        };
        let program = if alone {
            wrapped.clone()
        } else {
            merge(&base, &wrapped)
        };
        checked += 1;
        match compile(&program) {
            Ok(()) => {
                if !alone {
                    let ents = entities_only(&wrapped);
                    if !ents.is_empty() {
                        base = merge(&base, &ents);
                    }
                }
            }
            Err(e) => failures.push_str(&format!(
                "\n  docs/syntax-examples.md:{} (first line: {}): {e}",
                b.line,
                b.text.lines().next().unwrap_or("").trim()
            )),
        }
    }

    assert!(
        failures.is_empty(),
        "blocks that parse but fail `ordo check`. Fix the block, or mark it \
         `# planned: <what is not built>` on its first line:{failures}"
    );
    // Floors, so a change that skips everything cannot pass.
    assert!(checked >= 25, "only {checked} blocks were checked");
    assert!(
        skipped_planned <= 13,
        "{skipped_planned} blocks are marked planned; a new one needs a reason \
         and a raise of this ceiling"
    );
}
