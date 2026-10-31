//! The trigger layer's **operand grammars** — §7's settled half.
//!
//! # Why this module exists and the `when …` declaration does not
//!
//! `definition-language.md` §7 is headed `## 7. Triggers [settled]` and ends
//! before §8 begins, so it is **not** under §8's `"All illustrative syntax
//! [lean] — concepts settled, file format not final"` pin. But §7 and §8
//! spell the same fact two irreconcilable ways — §7's top-level
//! `when finance.transactions.ingested -> run spendWatch` against §8's
//! nested `trigger when …` inside a `routine` block — and nothing in the
//! corpus says which is the declaration. `-> run <name>` also names a
//! **routine**, and every routine spelling is `[lean]`.
//!
//! So the *enclosing declaration form* is not owed and is deliberately
//! absent: `parse_program` still accepts `domain`/`entity`/`op` and nothing
//! else, and `crates/ordo-syntax/tests/docs_parse.rs` still records the
//! `when`/`every` doc blocks as legitimately unparsed.
//!
//! What §7 *does* pin, in prose, above the lean line, and without depending
//! on where the clause is written, is everything in this file:
//!
//! - **the event-kind vocabulary is a closed set** — "`<kind>` comes from
//!   this vocabulary and no other — an unrecognized kind is a compile error
//!   (`unknown_event_kind`), not a pattern that silently never fires";
//! - **wildcards are legal at each position and only at a whole position —
//!   no prefix globbing**;
//! - **schedule times must be written with the `Z` suffix** — "UTC is
//!   explicit at the keyboard, never silently implied";
//! - **the modifier set** — `debounce <dur>` (trailing-edge, with a declared
//!   max-wait) and `coalesce` (at most one pending run).
//!
//! Those are spellings of *operands*, and an operand grammar can be built
//! without choosing a declaration form. This module is exactly that much and
//! not one keyword more: it parses strings, and no production of it is
//! reachable from `parse_program`.
//!
//! # Classification lives elsewhere
//!
//! Deciding which kinds a *particular command* raises needs the command
//! envelope — actor, correlation, mode — which lives in `ordo-commandlog`.
//! Keeping that out of here keeps `ordo-syntax` a leaf. The vocabulary is
//! here because it is a spelling; the classification is in
//! `ordo_exec::triggers` because it is a fact about history.

use crate::parser::{ParseError, PARSE_ERROR};
use crate::span::Span;
use crate::token::DurationUnit;

/// The class §7 names for a listener pattern whose kind position is not in
/// the closed vocabulary.
///
/// Documented at `definition-language.md` §7 and, before this module, with
/// **no producer anywhere** — `audit-doc-contradictions.md` F7 files it as a
/// settled rule whose stated consequence names a class that exists nowhere.
/// It is a *compile* class, not a wire class, so it belongs to the
/// `err("…")` inventory the author-facing crates carry and not to
/// `api-engine-design.md` §4's closed status table.
pub const UNKNOWN_EVENT_KIND: &str = "unknown_event_kind";

fn err(code: &'static str, message: String) -> ParseError {
    ParseError {
        code,
        message,
        // These grammars are parsed from a standalone string rather than
        // from a position in a source file, because there is no declaration
        // form to hold them yet. A zero span is honest about that; when a
        // declaration lands it can re-span what it parsed.
        span: Span::new(0, 0, 1, 1),
    }
}

// ── the closed event-kind vocabulary ─────────────────────────────────────

/// §7's closed set. Seven words, no eighth.
///
/// The table in §7 is the whole definition:
///
/// | Kind | Fires on |
/// |---|---|
/// | `created` | a command whose mode created the row |
/// | `converged` | a command that changed an existing row |
/// | `deleted` | a row removal (including a `subtree` cascade's per-row commands) |
/// | `written` | the union of the three above |
/// | `ingested` | a create attributed to a `source:` principal |
/// | `fed` | a write attributed to `source:feed-*` |
/// | `reverted` | a group revert touching the entity |
///
/// `ingested` and `fed` are **narrowings** of `created`/`written` by actor
/// kind, not separate events — §7 is explicit that "a listener on `written`
/// sees feed copies too, which is what makes feeds locally reactive".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventKind {
    Created,
    Converged,
    Deleted,
    Written,
    Ingested,
    Fed,
    Reverted,
}

impl EventKind {
    /// The vocabulary, in the order §7's table gives it. Iterating this is
    /// the only supported way to enumerate the set — a second list would be
    /// a second definition that could drift from the first.
    pub const ALL: &'static [EventKind] = &[
        EventKind::Created,
        EventKind::Converged,
        EventKind::Deleted,
        EventKind::Written,
        EventKind::Ingested,
        EventKind::Fed,
        EventKind::Reverted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Created => "created",
            EventKind::Converged => "converged",
            EventKind::Deleted => "deleted",
            EventKind::Written => "written",
            EventKind::Ingested => "ingested",
            EventKind::Fed => "fed",
            EventKind::Reverted => "reverted",
        }
    }

    /// `None` for anything outside the vocabulary. Callers that are parsing
    /// author text want [`ListenerPattern::parse`], which turns the `None`
    /// into `unknown_event_kind` rather than into silence.
    pub fn parse(s: &str) -> Option<EventKind> {
        EventKind::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── listener patterns ────────────────────────────────────────────────────

/// One position of a `<domain>.<entity>.<kind>` pattern.
///
/// `*` is the entire position or it is not a wildcard at all: §7 says
/// "Wildcards are legal at each position … **and only at a whole position —
/// no prefix globbing**". `fin*.transactions.written` is therefore a
/// `parse_error`, not a pattern that matches `finance`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pos {
    Any,
    Named(String),
}

impl Pos {
    pub fn matches(&self, s: &str) -> bool {
        match self {
            Pos::Any => true,
            Pos::Named(n) => n == s,
        }
    }

    fn render(&self) -> &str {
        match self {
            Pos::Any => "*",
            Pos::Named(n) => n,
        }
    }
}

/// The kind position: `*`, or exactly one word from the closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindPos {
    Any,
    Kind(EventKind),
}

impl KindPos {
    fn render(&self) -> &str {
        match self {
            KindPos::Any => "*",
            KindPos::Kind(k) => k.as_str(),
        }
    }
}

/// `<domain>.<entity>.<kind>` — a pattern on the **command envelope only**.
///
/// Every position it can name is outside the encryption boundary by
/// construction (§7: "event kind, space, entity type, actor — fields outside
/// the encryption boundary"), which is what makes the trigger layer blind.
/// Nothing here can name a field, and there is deliberately no place to put
/// one: "Anything that must *see a value* … is not a trigger — it is the
/// first line of a routine."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerPattern {
    pub domain: Pos,
    pub entity: Pos,
    pub kind: KindPos,
}

impl ListenerPattern {
    /// Parse `finance.transactions.ingested`, `finance.*.written`,
    /// `*.transactions.ingested`, `*.*.*`.
    ///
    /// Two classes, and which one a failure carries is decided by *what* is
    /// wrong, exactly as `parser.rs` decides between `parse_error` and
    /// `invalid_guard_walk`:
    ///
    /// - `parse_error` — the text is not three dot-separated positions, or a
    ///   position is not a bare name and not a bare `*`. The pattern could
    ///   not be read.
    /// - `unknown_event_kind` — the pattern read fine and the kind it names
    ///   is not in the vocabulary. §7 asks for this one by name, and asks
    ///   for it *loudly*: the alternative is a listener that compiles and
    ///   never fires.
    pub fn parse(s: &str) -> Result<ListenerPattern, ParseError> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(err(
                PARSE_ERROR,
                format!(
                    "a listener pattern is `<domain>.<entity>.<kind>` — three positions, \
                     got {} in `{s}`",
                    parts.len()
                ),
            ));
        }
        let domain = position(parts[0], s)?;
        let entity = position(parts[1], s)?;
        let kind = match position(parts[2], s)? {
            Pos::Any => KindPos::Any,
            Pos::Named(word) => match EventKind::parse(&word) {
                Some(k) => KindPos::Kind(k),
                None => {
                    return Err(err(
                        UNKNOWN_EVENT_KIND,
                        format!(
                            "`{word}` is not an event kind. The vocabulary is closed: {} \
                             (definition-language.md §7)",
                            EventKind::ALL
                                .iter()
                                .map(|k| k.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ))
                }
            },
        };
        Ok(ListenerPattern {
            domain,
            entity,
            kind,
        })
    }

    /// The canonical spelling, byte-identical to what parsed.
    pub fn render(&self) -> String {
        format!(
            "{}.{}.{}",
            self.domain.render(),
            self.entity.render(),
            self.kind.render()
        )
    }
}

impl std::fmt::Display for ListenerPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

/// One position: `*` whole, or a bare name.
///
/// A name is `[A-Za-z][A-Za-z0-9_]*` — the identifier shape the rest of the
/// grammar uses. A position containing a `*` anywhere else is refused with
/// the reason spelled out, because "prefix globbing quietly not matching" is
/// precisely the failure §7's wildcard rule exists to prevent.
fn position(part: &str, whole: &str) -> Result<Pos, ParseError> {
    if part == "*" {
        return Ok(Pos::Any);
    }
    if part.contains('*') {
        return Err(err(
            PARSE_ERROR,
            format!(
                "`{part}` in `{whole}`: wildcards are legal only at a WHOLE position — \
                 no prefix globbing (definition-language.md §7)"
            ),
        ));
    }
    let mut chars = part.chars();
    let ok = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok {
        return Err(err(
            PARSE_ERROR,
            format!("`{part}` in `{whole}` is not a name or `*`"),
        ));
    }
    Ok(Pos::Named(part.to_string()))
}

// ── clock schedules ──────────────────────────────────────────────────────

/// A cron-grade schedule, in the two spellings §7 shows.
///
/// `Daily` carries the hour and minute it was written with, **not** an
/// offset: §7 pins that "schedule times must be written with the `Z` suffix
/// (`every day 08:00Z`) — UTC is explicit at the keyboard, never silently
/// implied", and it goes on to say per-space local scheduling is "a possible
/// later extension, DST sidestepped". A field for a zone here would be the
/// first half of the thing that was sidestepped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// `every day 02:00Z`
    Daily { hour: u8, minute: u8 },
    /// `every 5m`
    Every { seconds: u64 },
}

/// Seconds in a day. Named because it is the `Daily` period and appears in
/// three places.
pub const DAY_SECS: u64 = 86_400;

impl Schedule {
    /// Parse a whole `every …` clause.
    ///
    /// Taking the leading `every` rather than the text after it is
    /// deliberate: it is the one word both §7's and §8's spellings agree on,
    /// so a schedule string round-trips through `render` no matter which
    /// declaration form eventually wraps it.
    pub fn parse(s: &str) -> Result<Schedule, ParseError> {
        let words: Vec<&str> = s.split_whitespace().collect();
        match words.as_slice() {
            ["every", "day", time] => daily(time, s),
            ["every", dur] => Ok(Schedule::Every {
                seconds: duration_secs(dur, s)?,
            }),
            _ => Err(err(
                PARSE_ERROR,
                format!("a schedule is `every day <HH:MM>Z` or `every <duration>`, got `{s}`"),
            )),
        }
    }

    /// How long until this schedule is due again, from any firing.
    ///
    /// The job queue's `claim_schedule` takes a period and advances
    /// `next_due` by it, so this is the number that makes fires-once true
    /// for a schedule rather than for a fleet sharing one cadence.
    pub fn period_secs(self) -> u64 {
        match self {
            Schedule::Daily { .. } => DAY_SECS,
            Schedule::Every { seconds } => seconds,
        }
    }

    /// The first firing instant at or after `now` (unix seconds, UTC).
    ///
    /// Pure integer arithmetic on the epoch, with no calendar and no zone —
    /// which is exactly what the `Z` pin buys. `Daily` lands on the same
    /// wall-clock minute every day because a UTC day is always 86400
    /// seconds; that is the DST sidestep, not an approximation of one.
    pub fn next_due(self, now: u64) -> u64 {
        match self {
            Schedule::Daily { hour, minute } => {
                let target = hour as u64 * 3600 + minute as u64 * 60;
                let midnight = now - now % DAY_SECS;
                let today = midnight + target;
                if today >= now {
                    today
                } else {
                    today + DAY_SECS
                }
            }
            Schedule::Every { seconds } => now + seconds,
        }
    }

    pub fn render(self) -> String {
        match self {
            Schedule::Daily { hour, minute } => format!("every day {hour:02}:{minute:02}Z"),
            Schedule::Every { seconds } => format!("every {}", render_secs(seconds)),
        }
    }
}

impl std::fmt::Display for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

/// `02:00Z` — and `02:00` is a **refusal**, not a default.
fn daily(time: &str, whole: &str) -> Result<Schedule, ParseError> {
    let Some(hhmm) = time.strip_suffix('Z') else {
        return Err(err(
            PARSE_ERROR,
            format!(
                "`{whole}`: a schedule time must carry the `Z` suffix (`every day 08:00Z`). \
                 UTC is explicit at the keyboard, never silently implied \
                 (definition-language.md §7)"
            ),
        ));
    };
    let Some((h, m)) = hhmm.split_once(':') else {
        return Err(err(
            PARSE_ERROR,
            format!("`{whole}`: a schedule time is `HH:MM` before the `Z`"),
        ));
    };
    // Two digits each, so `every day 2:0Z` is refused rather than guessed —
    // one spelling per concept.
    let bad = || {
        err(
            PARSE_ERROR,
            format!("`{whole}`: a schedule time is two-digit `HH:MM` in 24-hour UTC"),
        )
    };
    if h.len() != 2 || m.len() != 2 {
        return Err(bad());
    }
    let hour: u8 = h.parse().map_err(|_| bad())?;
    let minute: u8 = m.parse().map_err(|_| bad())?;
    if hour > 23 || minute > 59 {
        return Err(bad());
    }
    Ok(Schedule::Daily { hour, minute })
}

/// `5m`, `60s`, `2h`, `1d` — the closed duration unit set, restricted to the
/// exact-length units.
///
/// `mo` and `y` are refused here and nowhere else in the language, because a
/// schedule period is arithmetic on the epoch and a month is not a number of
/// seconds. The engine refuses unit conversion everywhere else deliberately;
/// silently deciding a month is 30 days inside the *clock* would be the
/// worst place to start.
fn duration_secs(lit: &str, whole: &str) -> Result<u64, ParseError> {
    let split = lit
        .find(|c: char| !c.is_ascii_digit())
        .filter(|&i| i > 0)
        .ok_or_else(|| {
            err(
                PARSE_ERROR,
                format!("`{whole}`: `{lit}` is not `<number><unit>`"),
            )
        })?;
    let (n, unit) = lit.split_at(split);
    let n: u64 = n.parse().map_err(|_| {
        err(
            PARSE_ERROR,
            format!("`{whole}`: `{n}` is not a whole number"),
        )
    })?;
    if n == 0 {
        return Err(err(
            PARSE_ERROR,
            format!("`{whole}`: a period of zero is not a schedule"),
        ));
    }
    let secs = match DurationUnit::parse(unit) {
        Some(DurationUnit::S) => 1,
        Some(DurationUnit::M) => 60,
        Some(DurationUnit::H) => 3600,
        Some(DurationUnit::D) => DAY_SECS,
        Some(DurationUnit::W) => 7 * DAY_SECS,
        Some(DurationUnit::Ms) => {
            return Err(err(
                PARSE_ERROR,
                format!("`{whole}`: `ms` is below the clock's resolution — seconds is the floor"),
            ))
        }
        Some(u @ (DurationUnit::Mo | DurationUnit::Y)) => {
            return Err(err(
                PARSE_ERROR,
                format!(
                    "`{whole}`: `{}` is not a fixed number of seconds, so it cannot be a \
                     schedule period. Write the days you mean",
                    u.as_str()
                ),
            ))
        }
        None => {
            return Err(err(
                PARSE_ERROR,
                format!("`{whole}`: `{unit}` is not a duration unit"),
            ))
        }
    };
    Ok(n * secs)
}

/// The inverse of [`duration_secs`] for the units it admits, largest first,
/// so `render(parse(x)) == x` for every literal a person would write.
fn render_secs(secs: u64) -> String {
    for (unit, n) in [("w", 7 * DAY_SECS), ("d", DAY_SECS), ("h", 3600), ("m", 60)] {
        if secs.is_multiple_of(n) {
            return format!("{}{unit}", secs / n);
        }
    }
    format!("{secs}s")
}

// ── modifiers ────────────────────────────────────────────────────────────

/// `debounce <dur>` and `coalesce` — §7's "no data needed" modifiers.
///
/// # `max_wait` is engine-pinned, and this is why
///
/// §7 says `debounce` is "trailing-edge — the run fires 5 minutes after the
/// last event in the burst, **with a declared max-wait**", and then gives no
/// spelling for declaring one. Both halves are load-bearing: without a
/// max-wait a steady stream of events at a shorter interval than the window
/// defers the run **forever**, which is a listener that silently never
/// fires — the same failure `unknown_event_kind` exists to prevent, arriving
/// through the other door.
///
/// So the value is real and the surface for it is not settled. It is
/// therefore a **policy value with an engine-pinned default**, exactly as
/// `corpus-rulings.md` pinned `take count out of range`: a limit on what
/// this engine will do, not a claim about what the language means. The
/// default is [`DEFAULT_MAX_WAIT_FACTOR`] × the window, and a caller that
/// wants another may set it — from a policy, once a policy has a spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Trailing-edge window, in seconds. `None` is "fire on the first event".
    pub debounce: Option<u64>,
    /// The declared max-wait, in seconds. Always `Some` when `debounce` is,
    /// so no code path has to rediscover the default.
    pub max_wait: Option<u64>,
    /// At most one pending run.
    pub coalesce: bool,
}

/// A debounced batch may be deferred at most this many windows past its
/// first event. See [`Modifiers`] for why the number is here and not in the
/// grammar.
pub const DEFAULT_MAX_WAIT_FACTOR: u64 = 10;

impl Modifiers {
    /// Parse a trailing modifier list: `debounce 5m coalesce`, `coalesce`,
    /// `debounce 60s`, or the empty string.
    ///
    /// Order-independent and repetition-refusing: a second `debounce` is a
    /// `parse_error` rather than a silent last-wins, because "the window I
    /// wrote is not the window that ran" is invisible at runtime.
    pub fn parse(s: &str) -> Result<Modifiers, ParseError> {
        let mut out = Modifiers::default();
        let words: Vec<&str> = s.split_whitespace().collect();
        let mut i = 0;
        while i < words.len() {
            match words[i] {
                "debounce" => {
                    if out.debounce.is_some() {
                        return Err(err(PARSE_ERROR, format!("`{s}`: `debounce` appears twice")));
                    }
                    let Some(lit) = words.get(i + 1) else {
                        return Err(err(
                            PARSE_ERROR,
                            format!("`{s}`: `debounce` takes a duration"),
                        ));
                    };
                    let secs = duration_secs(lit, s)?;
                    out.debounce = Some(secs);
                    out.max_wait = Some(secs * DEFAULT_MAX_WAIT_FACTOR);
                    i += 2;
                }
                "coalesce" => {
                    if out.coalesce {
                        return Err(err(PARSE_ERROR, format!("`{s}`: `coalesce` appears twice")));
                    }
                    out.coalesce = true;
                    i += 1;
                }
                other => {
                    return Err(err(
                        PARSE_ERROR,
                        format!(
                            "`{s}`: `{other}` is not a trigger modifier. The set is \
                             `debounce <dur>` and `coalesce` (definition-language.md §7)"
                        ),
                    ))
                }
            }
        }
        Ok(out)
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if let Some(d) = self.debounce {
            out.push_str(&format!("debounce {}", render_secs(d)));
        }
        if self.coalesce {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str("coalesce");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(e: &ParseError) -> &'static str {
        e.code
    }

    #[test]
    fn the_vocabulary_is_seven_words_and_round_trips() {
        assert_eq!(EventKind::ALL.len(), 7);
        for k in EventKind::ALL {
            assert_eq!(EventKind::parse(k.as_str()), Some(*k));
        }
        // The words §7 does NOT admit, including the two that read like
        // they should be there.
        for absent in ["updated", "changed", "inserted", "upserted", "", "Created"] {
            assert_eq!(EventKind::parse(absent), None, "`{absent}` is not a kind");
        }
    }

    #[test]
    fn patterns_parse_and_render_identically() {
        for src in [
            "finance.transactions.ingested",
            "finance.*.written",
            "*.transactions.ingested",
            "*.*.*",
            "estate.readings.written",
            "market.tasks.reverted",
        ] {
            let p = ListenerPattern::parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
            assert_eq!(p.render(), src);
        }
    }

    #[test]
    fn an_unrecognized_kind_is_a_compile_error_not_a_silent_never() {
        // The sentence this test is named after: "an unrecognized kind is a
        // compile error (`unknown_event_kind`), not a pattern that silently
        // never fires".
        let e = ListenerPattern::parse("finance.transactions.updated").unwrap_err();
        assert_eq!(class(&e), UNKNOWN_EVENT_KIND);
        // A plausible near-miss on a real kind is the same answer.
        let e = ListenerPattern::parse("finance.transactions.create").unwrap_err();
        assert_eq!(class(&e), UNKNOWN_EVENT_KIND);
    }

    #[test]
    fn wildcards_are_whole_positions_only() {
        for globbed in [
            "fin*.transactions.written",
            "finance.trans*.written",
            "finance.transactions.writ*",
            "*x.y.written",
        ] {
            let Err(e) = ListenerPattern::parse(globbed) else {
                panic!("`{globbed}` must not parse");
            };
            assert_eq!(class(&e), PARSE_ERROR, "{globbed}");
        }
    }

    #[test]
    fn a_pattern_is_exactly_three_positions() {
        for wrong in [
            "finance.transactions",
            "finance.transactions.ingested.extra",
            "written",
            "",
        ] {
            let e = ListenerPattern::parse(wrong).unwrap_err();
            assert_eq!(class(&e), PARSE_ERROR, "{wrong}");
        }
    }

    #[test]
    fn the_z_suffix_is_required_at_the_keyboard() {
        assert_eq!(
            Schedule::parse("every day 08:00Z").unwrap(),
            Schedule::Daily { hour: 8, minute: 0 }
        );
        // The whole point: no default, no local zone, no guess.
        let e = Schedule::parse("every day 08:00").unwrap_err();
        assert_eq!(class(&e), PARSE_ERROR);
        assert!(e.message.contains('Z'), "the message must name the fix");
        // And no other zone spelling sneaks in.
        for zoned in [
            "every day 08:00+01:00",
            "every day 08:00 UTC",
            "every day 08:00z",
        ] {
            assert!(Schedule::parse(zoned).is_err(), "{zoned}");
        }
    }

    #[test]
    fn schedules_round_trip() {
        for src in [
            "every day 02:00Z",
            "every day 23:59Z",
            "every 5m",
            "every 30s",
            "every 2h",
            "every 1d",
        ] {
            let s = Schedule::parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
            assert_eq!(s.render(), src);
        }
    }

    #[test]
    fn a_month_is_not_a_number_of_seconds() {
        for refused in [
            "every 1mo",
            "every 1y",
            "every 500ms",
            "every 0m",
            "every m",
        ] {
            assert!(Schedule::parse(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn daily_lands_on_the_same_utc_minute_every_day() {
        let s = Schedule::Daily { hour: 2, minute: 0 };
        // 1970-01-01T00:00:00Z → today's 02:00 is still ahead.
        assert_eq!(s.next_due(0), 7200);
        // Exactly at the target, the target is still "at or after now".
        assert_eq!(s.next_due(7200), 7200);
        // One second past it, tomorrow.
        assert_eq!(s.next_due(7201), 7200 + DAY_SECS);
        assert_eq!(s.period_secs(), DAY_SECS);
    }

    #[test]
    fn modifiers_parse_in_either_order_and_refuse_repeats() {
        let m = Modifiers::parse("debounce 5m coalesce").unwrap();
        assert_eq!(m.debounce, Some(300));
        assert!(m.coalesce);
        assert_eq!(m.max_wait, Some(300 * DEFAULT_MAX_WAIT_FACTOR));
        assert_eq!(m.render(), "debounce 5m coalesce");

        let m = Modifiers::parse("coalesce debounce 60s").unwrap();
        assert_eq!(m.debounce, Some(60));
        assert!(m.coalesce);

        assert_eq!(Modifiers::parse("").unwrap(), Modifiers::default());
        assert!(Modifiers::parse("debounce 5m debounce 6m").is_err());
        assert!(Modifiers::parse("coalesce coalesce").is_err());
        assert!(Modifiers::parse("debounce").is_err());
        assert!(Modifiers::parse("ratecap 5").is_err());
    }

    #[test]
    fn a_debounce_window_always_carries_a_max_wait() {
        // The invariant the runtime relies on: a burst that never stops
        // cannot defer a run forever, because the pair is set together and
        // there is no path that sets one without the other.
        for src in ["debounce 5m", "debounce 1h coalesce", "debounce 30s"] {
            let m = Modifiers::parse(src).unwrap();
            assert!(m.debounce.is_some() && m.max_wait.is_some(), "{src}");
        }
        let m = Modifiers::parse("coalesce").unwrap();
        assert!(m.debounce.is_none() && m.max_wait.is_none());
    }
}
