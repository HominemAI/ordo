//! **Language editions** (`plan-versions.md` §2.6, §2.8 step R4).
//!
//! A source names its edition with one line, `edition 2`. A source with no
//! edition line is edition 1, for good (board
//! `program-versions-the-core-under-schema` ask 5).
//!
//! What an edition means is this table, per Ordo release:
//!
//! - Each edition has the date of the Ordo release that made it current.
//! - The newest is the **current** edition. `ordo fix` rewrites to it.
//! - An older edition loads for `editions.load_window` (12 months) after
//!   the edition that replaced it became current. The window is measured
//!   against **this release's date**, never the clock, so a running
//!   deployment never stops loading a version as time passes. Only a newer
//!   release can drop an edition, and its release check names the versions
//!   first.
//! - Outside the window a source is refused, naming `ordo fix`.
//! - An edition newer than this release knows is refused, naming the fix.
//!
//! Each edition after the first carries its rule (what it refuses) and its
//! rewrite (what `ordo fix` changes). An edition's rule applies to every
//! source at that edition or later.
//!
//! **Only edition 1 is released.** Edition 2 is written and tested here,
//! but no release makes it current until the owner rules it. A debug build
//! reads [`TEST_EDITION_2_SINCE`] to make it current, and
//! [`TEST_RELEASE_DATE`] to date the build. A release build never reads
//! either.

use ordo_syntax::{ParseError, Span};

/// The edition a source with no edition line is in.
pub const FIRST: u32 = crate::artifact::FIRST_EDITION;

/// `editions.load_window`: how long an old edition loads after the next
/// one became current. An Ordo release setting (`plan-versions.md` §7).
pub const LOAD_WINDOW_MONTHS: u32 = 12;

/// How close to its window's end an edition is called out by the release
/// check, so the developer runs `ordo fix` before a release drops it.
pub const NEAR_END_MONTHS: u32 = 3;

/// The date of this Ordo release. The release procedure sets it with the
/// version pins. A stale date only keeps old editions loadable for longer.
pub const RELEASE_DATE: &str = "2026-11-23";

/// A debug build's flag: the date edition 2 became current. Unset, this
/// build knows edition 1 alone, as a release build does.
pub const TEST_EDITION_2_SINCE: &str = "ORDO_TEST_EDITION_2_SINCE";

/// A debug build's flag: this build's release date, in place of
/// [`RELEASE_DATE`].
pub const TEST_RELEASE_DATE: &str = "ORDO_TEST_RELEASE_DATE";

/// A source names an edition newer than this release knows.
pub const EDITION_UNKNOWN: &str = "edition_unknown";
/// A source's edition is outside `editions.load_window`.
pub const EDITION_EXPIRED: &str = "edition_expired";
/// A source breaks a rule of the edition it names.
pub const EDITION_RULE: &str = "edition_rule";

/// One edition after the first: what it changes.
struct Step {
    number: u32,
    /// One line, for `ordo fix` and the docs.
    change: &'static str,
    /// The places in one source text that break this edition's rule.
    breaks: fn(&str) -> Vec<Span>,
    /// What the rule says at each place.
    why: &'static str,
    /// One source text rewritten to keep this edition's rule.
    rewrite: fn(&str) -> String,
}

/// Every edition after the first this build has code for. Whether one is
/// released is [`Editions`]'s.
const STEPS: &[Step] = &[Step {
    number: 2,
    change: "bare `money` is written `money(0)`",
    breaks: ordo_syntax::bare_money_sites,
    why: "edition 2 states a money field's places: bare `money` is `money(0)`, so write \
          `money(0)` for whole units or `money(2)` for cents",
    rewrite: money_places,
}];

fn money_places(text: &str) -> String {
    let mut out = text.to_string();
    let mut sites = ordo_syntax::bare_money_sites(text);
    sites.sort_by_key(|s| std::cmp::Reverse(s.end));
    for s in sites {
        out.insert_str(s.end, "(0)");
    }
    out
}

/// A calendar date, compared as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: u32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    /// `YYYY-MM-DD`.
    pub fn parse(s: &str) -> Option<Date> {
        let mut it = s.trim().split('-').map(|p| p.parse::<u32>().ok());
        let d = Date {
            year: it.next()??,
            month: it.next()??,
            day: it.next()??,
        };
        (it.next().is_none() && (1..=12).contains(&d.month) && (1..=31).contains(&d.day))
            .then_some(d)
    }

    /// This date `months` later, the day kept (a 31st past a short month
    /// reads as that month's end for comparison, which is all it is for).
    pub fn plus_months(self, months: u32) -> Date {
        let m = self.year * 12 + (self.month - 1) + months;
        Date {
            year: m / 12,
            month: m % 12 + 1,
            day: self.day,
        }
    }
}

impl std::fmt::Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Where one edition stands under this release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// The current edition.
    Current,
    /// An older edition inside its window. `near` when the window ends
    /// within [`NEAR_END_MONTHS`] of this release's date.
    Loadable { ends: Date, near: bool },
    /// An older edition whose window ended before this release.
    Expired { ended: Date },
    /// Newer than this release knows.
    Unknown,
}

/// The editions this build knows, and its release date.
#[derive(Debug, Clone)]
pub struct Editions {
    /// The date each released edition after the first became current, in
    /// order: `since[0]` is edition 2's.
    since: Vec<Date>,
    release: Date,
}

impl Editions {
    /// This build's editions: edition 1 alone in a release build.
    pub fn this_release() -> Editions {
        let mut e = Editions {
            since: Vec::new(),
            release: Date::parse(RELEASE_DATE).expect("RELEASE_DATE is a date"),
        };
        if cfg!(debug_assertions) {
            if let Some(d) = std::env::var(TEST_EDITION_2_SINCE)
                .ok()
                .and_then(|v| Date::parse(&v))
            {
                e.since.push(d);
            }
            if let Some(d) = std::env::var(TEST_RELEASE_DATE)
                .ok()
                .and_then(|v| Date::parse(&v))
            {
                e.release = d;
            }
        }
        e
    }

    /// A table given outright: the tests' seam.
    pub fn with(since: Vec<Date>, release: Date) -> Editions {
        assert!(since.len() <= STEPS.len(), "an edition with no code");
        Editions { since, release }
    }

    /// The current edition.
    pub fn current(&self) -> u32 {
        FIRST + self.since.len() as u32
    }

    /// This release's date.
    pub fn release_date(&self) -> Date {
        self.release
    }

    /// Where edition `n` stands under this release.
    pub fn standing(&self, n: u32) -> Standing {
        let current = self.current();
        if n == current {
            return Standing::Current;
        }
        if n > current || n < FIRST {
            return Standing::Unknown;
        }
        // Replaced when edition n + 1 became current.
        let replaced = self.since[(n - FIRST) as usize];
        let ends = replaced.plus_months(LOAD_WINDOW_MONTHS);
        if self.release > ends {
            Standing::Expired { ended: ends }
        } else {
            let near = self.release.plus_months(NEAR_END_MONTHS) >= ends;
            Standing::Loadable { ends, near }
        }
    }

    /// **The edition gate.** The edition `source` is in, or why this
    /// release does not load it. Every compile comes through here: the
    /// load at boot, an apply, `ordo check` and `ordo dev`.
    pub fn gate(&self, source: &str, program: &ordo_syntax::Program) -> Result<u32, ParseError> {
        let (n, span) = match &program.edition {
            Some(e) => (e.number, e.span),
            None => (FIRST, Span::new(0, 0, 1, 1)),
        };
        let current = self.current();
        match self.standing(n) {
            Standing::Unknown => {
                return Err(ParseError {
                    code: EDITION_UNKNOWN,
                    message: format!(
                        "this Ordo release knows {}, and the source says `edition {n}`. \
                         Upgrade Ordo to one that knows edition {n}, or write \
                         `edition {current}`",
                        if current == FIRST {
                            format!("edition {FIRST} only")
                        } else {
                            format!("editions {FIRST} to {current}")
                        }
                    ),
                    span,
                })
            }
            Standing::Expired { ended } => {
                return Err(ParseError {
                    code: EDITION_EXPIRED,
                    message: format!(
                        "edition {n} is no longer loaded: its load window \
                         (`editions.load_window`, {LOAD_WINDOW_MONTHS} months) ended on \
                         {ended}, before this Ordo release ({}). Run `ordo fix <dir> \
                         --write` to rewrite the source to edition {current}, then apply it",
                        self.release
                    ),
                    span,
                })
            }
            Standing::Current | Standing::Loadable { .. } => {}
        }
        for step in STEPS.iter().filter(|s| s.number <= n) {
            if let Some(at) = (step.breaks)(source).into_iter().next() {
                return Err(ParseError {
                    code: EDITION_RULE,
                    message: format!("{}. `ordo fix <dir> --write` does it for you", step.why),
                    span: at,
                });
            }
        }
        Ok(n)
    }

    /// **`ordo fix`**: `files` (label, text), one program, rewritten to the
    /// current edition. Files that need no change come back unchanged.
    pub fn fix(&self, files: &[(String, String)]) -> Result<Fixed, String> {
        let whole: String = files
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let program = ordo_syntax::parse_program(&whole)
            .map_err(|e| format!("the source does not parse, so it is not rewritten: {e}"))?;
        let from = program.edition.as_ref().map_or(FIRST, |e| e.number);
        let to = self.current();
        if from > to {
            return Err(format!(
                "the source says `edition {from}`, newer than this Ordo release knows (up to \
                 {to}). Upgrade Ordo; `ordo fix` never moves a source back"
            ));
        }
        let steps: Vec<&Step> = STEPS
            .iter()
            .filter(|s| s.number > from && s.number <= to)
            .collect();
        let mut out = Vec::new();
        for (label, text) in files {
            let mut new = text.clone();
            for s in &steps {
                new = (s.rewrite)(&new);
            }
            out.push((label.clone(), text.clone(), new));
        }
        if from != to {
            let at = home_of_edition_line(&out);
            out[at].2 = with_edition_line(&out[at].2, to);
        }
        Ok(Fixed {
            from,
            to,
            changes: steps.iter().map(|s| s.change.to_string()).collect(),
            files: out,
        })
    }
}

/// The file the edition line belongs in: the one that has it, else the one
/// with the `domain` header, else the first.
fn home_of_edition_line(files: &[(String, String, String)]) -> usize {
    let has = |t: &str, f: fn(&ordo_syntax::Program) -> bool| {
        ordo_syntax::parse_program(t)
            .map(|p| f(&p))
            .unwrap_or(false)
    };
    files
        .iter()
        .position(|(_, _, t)| has(t, |p| p.edition.is_some()))
        .or_else(|| {
            files
                .iter()
                .position(|(_, _, t)| has(t, |p| p.domain.is_some()))
        })
        .unwrap_or(0)
}

/// `text` saying `edition n`: its line rewritten, or one added after the
/// `domain` header, or before the first declaration.
pub fn with_edition_line(text: &str, n: u32) -> String {
    let parsed = ordo_syntax::parse_program(text).ok();
    if let Some(e) = parsed.as_ref().and_then(|p| p.edition.as_ref()) {
        return format!(
            "{}edition {n}{}",
            &text[..e.span.start],
            &text[e.span.end..]
        );
    }
    let line = format!("edition {n}\n");
    let mut lines: Vec<&str> = text.split_inclusive('\n').collect();
    let at = match parsed.as_ref().and_then(|p| p.domain.as_ref()) {
        Some(d) => d.span.line as usize,
        None => lines
            .iter()
            .position(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('#')
            })
            .unwrap_or(lines.len()),
    };
    let ends_open = at == lines.len() && !text.is_empty() && !text.ends_with('\n');
    let fixed;
    if ends_open {
        fixed = format!("{}\n", lines[at - 1]);
        lines[at - 1] = &fixed;
    }
    lines.insert(at, &line);
    lines.concat()
}

/// What `ordo fix` would write.
#[derive(Debug, Clone)]
pub struct Fixed {
    pub from: u32,
    pub to: u32,
    /// One line per edition step crossed.
    pub changes: Vec<String>,
    /// (label, before, after), one per file given.
    pub files: Vec<(String, String, String)>,
}

impl Fixed {
    /// The files whose text changes.
    pub fn changed(&self) -> impl Iterator<Item = &(String, String, String)> {
        self.files.iter().filter(|(_, a, b)| a != b)
    }

    /// Every change as a line diff, file by file.
    pub fn diff(&self) -> String {
        self.changed()
            .map(|(label, a, b)| line_diff(label, a, b))
            .collect()
    }
}

/// A unified-style line diff of `a` to `b`: removed lines `-`, added `+`,
/// each run headed with its line in `a`. Sources are small, so a plain
/// longest-common-subsequence table does.
pub fn line_diff(label: &str, a: &str, b: &str) -> String {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = format!("--- {label}\n+++ {label}\n");
    let (mut i, mut j) = (0, 0);
    let mut open = false;
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            i += 1;
            j += 1;
            open = false;
            continue;
        }
        if !open {
            out.push_str(&format!("@@ line {} @@\n", i + 1));
            open = true;
        }
        if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push_str(&format!("-{}\n", a[i]));
            i += 1;
        } else {
            out.push_str(&format!("+{}\n", b[j]));
            j += 1;
        }
    }
    out
}

/// The edition `source` names: its `edition` line, or edition 1. A source
/// that does not parse is edition 1 here; its compile says why it fails.
pub fn of_source(source: &str) -> u32 {
    ordo_syntax::parse_program(source)
        .ok()
        .and_then(|p| p.edition)
        .map_or(FIRST, |e| e.number)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    #[test]
    fn edition_one_alone_is_current() {
        let e = Editions::with(vec![], d("2026-11-23"));
        assert_eq!(e.current(), 1);
        assert_eq!(e.standing(1), Standing::Current);
        assert_eq!(e.standing(2), Standing::Unknown);
    }

    #[test]
    fn the_window_is_measured_against_the_release_date() {
        let since = d("2026-01-10");
        let inside = Editions::with(vec![since], d("2026-06-01"));
        assert_eq!(
            inside.standing(1),
            Standing::Loadable {
                ends: d("2027-01-10"),
                near: false
            }
        );
        let near = Editions::with(vec![since], d("2026-11-01"));
        assert_eq!(
            near.standing(1),
            Standing::Loadable {
                ends: d("2027-01-10"),
                near: true
            }
        );
        let last_day = Editions::with(vec![since], d("2027-01-10"));
        assert!(matches!(last_day.standing(1), Standing::Loadable { .. }));
        let after = Editions::with(vec![since], d("2027-01-11"));
        assert_eq!(
            after.standing(1),
            Standing::Expired {
                ended: d("2027-01-10")
            }
        );
    }

    #[test]
    fn dates_add_months_across_a_year() {
        assert_eq!(d("2026-11-23").plus_months(3), d("2027-02-23"));
        assert_eq!(d("2026-12-01").plus_months(12), d("2027-12-01"));
        assert!(Date::parse("2026-13-01").is_none());
    }

    #[test]
    fn the_line_diff_shows_only_what_moved() {
        let diff = line_diff(
            "a.ordo",
            "one\ntwo\nthree\n",
            "one\nedition 2\ntwo\nthree\n",
        );
        assert_eq!(diff, "--- a.ordo\n+++ a.ordo\n@@ line 2 @@\n+edition 2\n");
        let diff = line_diff("a.ordo", "one\n  x: money\n", "one\n  x: money(0)\n");
        assert_eq!(
            diff,
            "--- a.ordo\n+++ a.ordo\n@@ line 2 @@\n-  x: money\n+  x: money(0)\n"
        );
    }

    #[test]
    fn the_edition_line_goes_after_the_domain_header() {
        let src = "# a note\n\ndomain shop @ 0.1.0\n\nentity items {\n  name: text\n}\n";
        assert_eq!(
            with_edition_line(src, 2),
            "# a note\n\ndomain shop @ 0.1.0\nedition 2\n\nentity items {\n  name: text\n}\n"
        );
        let written = with_edition_line(src, 2);
        assert_eq!(
            with_edition_line(&written, 3),
            written.replace("edition 2", "edition 3")
        );
        assert_eq!(
            with_edition_line("# c\nentity a {\n  n: text\n}", 2),
            "# c\nedition 2\nentity a {\n  n: text\n}"
        );
    }
}
