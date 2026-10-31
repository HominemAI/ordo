//! `ordo check` and `each ->` routine bodies (owner ruling 2026-10-08,
//! `triggers` ask 1).
//!
//! The tutorial's daily nag (`tutorial.md` §4, `thirstCheck`) is the
//! routine a reader meets first. Its block is taken from the tutorial as
//! written and checked against the tutorial's own shapes, so a change to
//! either that breaks it fails here.

use ordo_cli::{check_sources, Sources};

const TUTORIAL: &str = include_str!("../../../docs/tutorial.md");

fn one(text: &str) -> Sources {
    Sources::from_parts(vec![("app.ordo".into(), text.to_string())])
}

/// The fenced block that opens with `opening`, as the tutorial writes it.
fn block(opening: &str) -> String {
    let mut lines = TUTORIAL.lines();
    while let Some(l) = lines.next() {
        if l.trim() == "```" {
            let body: Vec<&str> = lines.by_ref().take_while(|l| l.trim() != "```").collect();
            if body.first().is_some_and(|f| f.starts_with(opening)) {
                return body.join("\n");
            }
        }
    }
    panic!("the tutorial has no block opening with `{opening}`");
}

/// The tutorial's plantcare shapes (§1), without the `today` defaults,
/// which are specified and not built.
const PLANTCARE: &str = "\
domain plantcare @ 0.1.0

entity plants {
  name:      text
  room:      text?
  everyDays: duration(ms) = 7d
  notes:     text = \"\"     sealed
}

entity waterings {
  plant -> plants   owned
  at:    date
  by:    text = \"\"
}

entity alerts {
  key:      text unique
  severity: text = \"warning\"
  title:    text
  status:   text = \"active\"
}
";

#[test]
fn the_tutorials_each_example_parses_and_checks() {
    let routine = block("routine thirstCheck in plantcare");
    assert!(routine.contains("each ->"), "{routine}");
    let out = check_sources(&one(&format!("{PLANTCARE}\n{routine}\n")))
        .unwrap_or_else(|e| panic!("the tutorial's routine checks: {e}"));
    assert!(out.contains("\nROUTINES\n"), "{out}");
    assert!(
        out.contains(
            "  thirstCheck (every day 09:00Z, each row of plants waterings, at most 500 a run)\n"
        ),
        "{out}"
    );
}

#[test]
fn check_refuses_a_name_the_row_does_not_carry() {
    let src = format!(
        "{PLANTCARE}
routine nag {{
  trigger every day 09:00Z
  run:
    plants[id]
    each ->
      set alerts {{ key: \"nag:\" + id, title: name }}
}}
"
    );
    let e = check_sources(&one(&src)).expect_err("`name` was not projected");
    assert!(e.contains("invalid_routine"), "{e}");
    assert!(e.contains("`name` is not a field of the row"), "{e}");
}
