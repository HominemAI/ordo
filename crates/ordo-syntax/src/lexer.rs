//! Hand-written lexer. Line-aware: a `Newline` token is emitted only at
//! delimiter depth zero (engine-plan G6) — inside any open `(` `[` `{` the
//! line break is ordinary whitespace, which is exactly how every multi-line
//! `group { … }` in the spec already reads.

use crate::span::Span;
use crate::token::{DurationUnit, Keyword, Token, TokenKind};

const MAX_IDENT_LEN: usize = 64;

/// Is `y` a leap year in the proleptic Gregorian calendar? Divisible by 4,
/// except centuries, except every fourth century — so 1900 is not a leap year
/// and 2000 is.
fn is_leap_year(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days in `m` (1–12) of year `y`; `None` for a month outside 1–12.
fn days_in_month(y: i32, m: u8) -> Option<u8> {
    Some(match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(y) => 29,
        2 => 28,
        _ => return None,
    })
}

/// Is `YYYY-MM-DD` a date that exists?
///
/// A range check on the parts is not this question: `1..=12` and `1..=31`
/// admit `2026-02-31`, `2026-04-31`, and `2026-02-29`, all of which would then
/// travel through the AST as if they named a day (Z-010 / M2-012). The
/// calendar is the constraint, so the calendar is what the lexer checks —
/// month length and the Gregorian leap rule, in arithmetic, with no
/// dependency added for twelve lines of it.
fn is_real_date(y: i32, m: u8, d: u8) -> bool {
    days_in_month(y, m).is_some_and(|last| (1..=last).contains(&d))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.span, self.message)
    }
}

impl std::error::Error for LexError {}

pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(src).run()
}

struct Lexer<'s> {
    src: &'s str,
    bytes: &'s [u8],
    pos: usize,
    line: u32,
    col: u32,
    depth: u32,
    out: Vec<Token>,
}

impl<'s> Lexer<'s> {
    fn new(src: &'s str) -> Self {
        Lexer {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
            depth: 0,
            out: Vec::new(),
        }
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        while self.pos < self.bytes.len() {
            let start = self.pos;
            let (line, col) = (self.line, self.col);
            let b = self.bytes[self.pos];
            match b {
                b' ' | b'\t' | b'\r' => self.bump(),
                b'\n' => {
                    self.bump_newline();
                    // Only a separator at depth zero, and never stacked.
                    if self.depth == 0
                        && !matches!(
                            self.out.last().map(|t| &t.kind),
                            Some(TokenKind::Newline) | None
                        )
                    {
                        self.push(TokenKind::Newline, start, line, col);
                    }
                }
                b'#' => {
                    while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                        self.bump();
                    }
                }
                b'"' => self.string(start, line, col)?,
                b'$' => self.param(start, line, col)?,
                b'0'..=b'9' => self.number(start, line, col)?,
                b'a'..=b'z' => self.ident(start, line, col)?,
                b'A'..=b'Z' | b'_' => {
                    let word = self.take_word();
                    return Err(self.err_at(
                        format!(
                            "identifiers start with a lowercase letter and use no underscores: `{word}`"
                        ),
                        start,
                        line,
                        col,
                    ));
                }
                _ => self.operator(start, line, col)?,
            }
        }
        if !matches!(
            self.out.last().map(|t| &t.kind),
            Some(TokenKind::Newline) | None
        ) {
            self.push(TokenKind::Newline, self.pos, self.line, self.col);
        }
        self.push(TokenKind::Eof, self.pos, self.line, self.col);
        Ok(self.out)
    }

    // ── character plumbing ──

    fn bump(&mut self) {
        // Advance one char (source is validated UTF-8 by &str).
        let ch_len = self.src[self.pos..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(1);
        self.pos += ch_len;
        self.col += 1;
    }

    fn bump_newline(&mut self) {
        self.pos += 1;
        self.line += 1;
        self.col = 1;
    }

    fn peek(&self, off: usize) -> u8 {
        *self.bytes.get(self.pos + off).unwrap_or(&0)
    }

    fn push(&mut self, kind: TokenKind, start: usize, line: u32, col: u32) {
        self.out.push(Token {
            kind,
            span: Span::new(start, self.pos, line, col),
        });
    }

    fn err_at(&self, message: String, start: usize, line: u32, col: u32) -> LexError {
        LexError {
            message,
            span: Span::new(start, self.pos.max(start + 1), line, col),
        }
    }

    fn take_word(&mut self) -> String {
        let start = self.pos;
        while matches!(self.peek(0), b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_') {
            self.bump();
        }
        self.src[start..self.pos].to_string()
    }

    // ── token families ──

    fn string(&mut self, start: usize, line: u32, col: u32) -> Result<(), LexError> {
        self.bump(); // opening quote
        let mut value = String::new();
        loop {
            match self.peek(0) {
                0 => {
                    return Err(self.err_at("unterminated string".into(), start, line, col));
                }
                b'\n' => {
                    return Err(self.err_at(
                        "strings do not span lines (concatenate with +)".into(),
                        start,
                        line,
                        col,
                    ));
                }
                b'"' => {
                    self.bump();
                    break;
                }
                b'\\' => {
                    self.bump();
                    let esc = self.peek(0);
                    // Closed escape set (G4): exactly these four.
                    let resolved = match esc {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'n' => '\n',
                        b't' => '\t',
                        _ => {
                            return Err(self.err_at(
                                format!(
                                    "unknown escape `\\{}` — the escapes are \\\" \\\\ \\n \\t",
                                    esc as char
                                ),
                                start,
                                line,
                                col,
                            ));
                        }
                    };
                    value.push(resolved);
                    self.bump();
                }
                _ => {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    value.push(ch);
                    self.bump();
                }
            }
        }
        // 9u: a literal is NFC from the moment it exists, so a guard or a
        // condition comparing against a composed literal matches a stored
        // value regardless of how the SOURCE FILE was encoded — two editors
        // can disagree about how to spell `é` and produce the same program.
        self.push(TokenKind::Str(crate::text::nfc(&value)), start, line, col);
        Ok(())
    }

    fn param(&mut self, start: usize, line: u32, col: u32) -> Result<(), LexError> {
        self.bump(); // $
        if !self.peek(0).is_ascii_lowercase() {
            return Err(self.err_at(
                "a parameter is `$` followed by a lowercase name".into(),
                start,
                line,
                col,
            ));
        }
        let name = self.take_word();
        self.check_name(&name, start, line, col)?;
        self.push(TokenKind::Param(name), start, line, col);
        Ok(())
    }

    fn ident(&mut self, start: usize, line: u32, col: u32) -> Result<(), LexError> {
        let word = self.take_word();
        self.check_name(&word, start, line, col)?;
        // A digit right after a word never means anything; catch `foo1bar$`-ish
        // confusion early? No — trailing digits are legal in names. Nothing to do.
        match Keyword::parse(&word) {
            Some(kw) => self.push(TokenKind::Keyword(kw), start, line, col),
            None => self.push(TokenKind::Ident(word), start, line, col),
        }
        Ok(())
    }

    fn check_name(&self, word: &str, start: usize, line: u32, col: u32) -> Result<(), LexError> {
        if word.contains('_') {
            return Err(self.err_at(
                format!("names use lowerCamel, no underscores: `{word}`"),
                start,
                line,
                col,
            ));
        }
        if word.len() > MAX_IDENT_LEN {
            return Err(self.err_at(
                format!("name exceeds {MAX_IDENT_LEN} characters"),
                start,
                line,
                col,
            ));
        }
        Ok(())
    }

    /// Digits lead four ways: date `2026-07-01`, datetime with `T…Z`,
    /// duration `90d`/`1mo`, decimal `10.50`, or a plain int.
    fn number(&mut self, start: usize, line: u32, col: u32) -> Result<(), LexError> {
        while self.peek(0).is_ascii_digit() {
            self.bump();
        }
        let digits = &self.src[start..self.pos];

        // Date: exactly 4 digits, then -DD-DD with no spaces.
        if digits.len() == 4
            && self.peek(0) == b'-'
            && self.peek(1).is_ascii_digit()
            && self.peek(2).is_ascii_digit()
            && self.peek(3) == b'-'
            && self.peek(4).is_ascii_digit()
            && self.peek(5).is_ascii_digit()
        {
            return self.date(start, line, col);
        }

        // A schedule time: exactly `HH:MMZ` (DL §7, "schedule times must be
        // written with the `Z` suffix"). Only the suffixed form is a token:
        // `09:00` without the `Z` stays `Int Colon Int`, and the trigger
        // clause refuses it by name, because UTC is never silently implied.
        if digits.len() == 2
            && self.peek(0) == b':'
            && self.peek(1).is_ascii_digit()
            && self.peek(2).is_ascii_digit()
            && self.peek(3) == b'Z'
            && !self.peek(4).is_ascii_alphanumeric()
        {
            for _ in 0..4 {
                self.bump();
            }
            let text = &self.src[start..self.pos];
            let hour: u8 = text[0..2].parse().unwrap_or(99);
            let minute: u8 = text[3..5].parse().unwrap_or(99);
            if hour > 23 || minute > 59 {
                return Err(self.err_at(
                    format!("invalid time `{text}`: a schedule time is `HH:MMZ`, 24-hour UTC"),
                    start,
                    line,
                    col,
                ));
            }
            self.push(TokenKind::UtcTime { hour, minute }, start, line, col);
            return Ok(());
        }

        // Decimal: digits '.' digits (a bare trailing dot is a dot-path error).
        if self.peek(0) == b'.' && self.peek(1).is_ascii_digit() {
            self.bump(); // .
            while self.peek(0).is_ascii_digit() {
                self.bump();
            }
            let text = self.src[start..self.pos].to_string();
            if self.peek(0).is_ascii_lowercase() {
                return Err(self.err_at(
                    "a decimal cannot carry a duration unit".into(),
                    start,
                    line,
                    col,
                ));
            }
            self.push(TokenKind::Decimal(text), start, line, col);
            return Ok(());
        }

        // Duration: unit letters glued to the digits, from the closed set.
        if self.peek(0).is_ascii_lowercase() {
            let unit_start = self.pos;
            while self.peek(0).is_ascii_lowercase() {
                self.bump();
            }
            let unit_text = &self.src[unit_start..self.pos];
            let Some(unit) = DurationUnit::parse(unit_text) else {
                if matches!(unit_text, "kb" | "mb" | "gb") {
                    return Err(self.err_at(
                        format!(
                            "a size is written with a space: `{digits} {unit_text}` \
                             (a number glued to letters is a duration)"
                        ),
                        start,
                        line,
                        col,
                    ));
                }
                return Err(self.err_at(
                    format!("unknown duration unit `{unit_text}` — units are ms s m h d w mo y"),
                    start,
                    line,
                    col,
                ));
            };
            let n: u64 = digits
                .parse()
                .map_err(|_| self.err_at("duration count too large".into(), start, line, col))?;
            self.push(TokenKind::Duration { n, unit }, start, line, col);
            return Ok(());
        }

        let n: i128 = digits
            .parse()
            .map_err(|_| self.err_at("integer literal too large".into(), start, line, col))?;
        self.push(TokenKind::Int(n), start, line, col);
        Ok(())
    }

    fn date(&mut self, start: usize, line: u32, col: u32) -> Result<(), LexError> {
        // Already positioned after YYYY; consume -MM-DD.
        for _ in 0..6 {
            self.bump();
        }
        let text = &self.src[start..self.pos];
        let y: i32 = text[0..4].parse().unwrap();
        let m: u8 = text[5..7].parse().unwrap();
        let d: u8 = text[8..10].parse().unwrap();
        // The calendar, not a range: `2026-02-31` and `2026-04-31` pass a
        // `1..=31` check and name no day (Z-010). Applied before the datetime
        // tail so a bad date is reported as a bad date, whatever follows it.
        if !is_real_date(y, m, d) {
            return Err(self.err_at(format!("invalid date `{text}`"), start, line, col));
        }

        // Datetime tail: T HH:MM:SS Z — UTC only, the Z is mandatory (spec §7).
        if self.peek(0) == b'T' {
            let ok = self.peek(1).is_ascii_digit()
                && self.peek(2).is_ascii_digit()
                && self.peek(3) == b':'
                && self.peek(4).is_ascii_digit()
                && self.peek(5).is_ascii_digit()
                && self.peek(6) == b':'
                && self.peek(7).is_ascii_digit()
                && self.peek(8).is_ascii_digit()
                && self.peek(9) == b'Z';
            if !ok {
                // Consume through the malformed tail for a readable span.
                while matches!(self.peek(0), b'0'..=b'9' | b'T' | b':' | b'Z' | b'+') {
                    self.bump();
                }
                return Err(self.err_at(
                    "datetime is `YYYY-MM-DDThh:mm:ssZ` — UTC, the Z is mandatory".into(),
                    start,
                    line,
                    col,
                ));
            }
            for _ in 0..10 {
                self.bump();
            }
            let t = &self.src[start..self.pos];
            let hh: u8 = t[11..13].parse().unwrap();
            let mm: u8 = t[14..16].parse().unwrap();
            let ss: u8 = t[17..19].parse().unwrap();
            if hh > 23 || mm > 59 || ss > 59 {
                return Err(self.err_at(format!("invalid time in `{t}`"), start, line, col));
            }
            self.push(
                TokenKind::Datetime {
                    y,
                    m,
                    d,
                    hh,
                    mm,
                    ss,
                },
                start,
                line,
                col,
            );
            return Ok(());
        }

        self.push(TokenKind::Date { y, m, d }, start, line, col);
        Ok(())
    }

    fn operator(&mut self, start: usize, line: u32, col: u32) -> Result<(), LexError> {
        let b = self.bytes[self.pos];
        // `get` and not an index: the next character may be several bytes
        // long, and a slice through it would panic (audit 2026-10-11, 29).
        let two = self.src.get(self.pos..self.pos + 2).unwrap_or("");

        let kind = match two {
            "->" => Some(TokenKind::Arrow),
            "==" => Some(TokenKind::EqEq),
            "!=" => Some(TokenKind::NotEq),
            "<=" => Some(TokenKind::LtEq),
            ">=" => Some(TokenKind::GtEq),
            _ => None,
        };
        if let Some(kind) = kind {
            self.bump();
            self.bump();
            self.push(kind, start, line, col);
            return Ok(());
        }

        let kind = match b {
            b'(' => {
                self.depth += 1;
                TokenKind::LParen
            }
            b')' => {
                self.depth = self.depth.saturating_sub(1);
                TokenKind::RParen
            }
            b'[' => {
                self.depth += 1;
                TokenKind::LBracket
            }
            b']' => {
                self.depth = self.depth.saturating_sub(1);
                TokenKind::RBracket
            }
            b'{' => {
                self.depth += 1;
                TokenKind::LBrace
            }
            b'}' => {
                self.depth = self.depth.saturating_sub(1);
                TokenKind::RBrace
            }
            b',' => TokenKind::Comma,
            b':' => TokenKind::Colon,
            b'.' => TokenKind::Dot,
            b'|' => TokenKind::Bar,
            b'=' => TokenKind::Eq,
            b'<' => TokenKind::Lt,
            b'>' => TokenKind::Gt,
            b'+' => TokenKind::Plus,
            b'-' => TokenKind::Minus,
            b'*' => TokenKind::Star,
            b'/' => TokenKind::Slash,
            b'?' => TokenKind::Question,
            b'@' => TokenKind::At,
            b'!' => {
                self.bump();
                return Err(self.err_at(
                    "`!` is not an operator — negation is the word `not`, inequality is `!=`"
                        .into(),
                    start,
                    line,
                    col,
                ));
            }
            _ => {
                let ch = self.src[self.pos..].chars().next().unwrap_or('\u{fffd}');
                self.bump();
                return Err(self.err_at(
                    format!("unexpected character `{ch}` (U+{:04X})", ch as u32),
                    start,
                    line,
                    col,
                ));
            }
        };
        self.bump();
        self.push(kind, start, line, col);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    fn err(src: &str) -> String {
        lex(src).unwrap_err().message
    }

    #[test]
    fn source_line_lexes() {
        use TokenKind::*;
        assert_eq!(
            kinds(r#"orders(status == "open" and total > 100)[placed, total]"#),
            vec![
                Ident("orders".into()),
                LParen,
                Ident("status".into()),
                EqEq,
                Str("open".into()),
                Keyword(super::Keyword::And),
                Ident("total".into()),
                Gt,
                Int(100),
                RParen,
                LBracket,
                Ident("placed".into()),
                Comma,
                Ident("total".into()),
                RBracket,
                Newline,
                Eof
            ]
        );
    }

    #[test]
    fn dates_datetimes_durations() {
        assert_eq!(
            kinds("2026-07-01"),
            vec![
                TokenKind::Date {
                    y: 2026,
                    m: 7,
                    d: 1
                },
                TokenKind::Newline,
                TokenKind::Eof
            ]
        );
        assert_eq!(
            kinds("2026-07-01T14:30:00Z")[0],
            TokenKind::Datetime {
                y: 2026,
                m: 7,
                d: 1,
                hh: 14,
                mm: 30,
                ss: 0
            }
        );
        assert_eq!(
            kinds("90d")[0],
            TokenKind::Duration {
                n: 90,
                unit: DurationUnit::D
            }
        );
        assert_eq!(
            kinds("1mo")[0],
            TokenKind::Duration {
                n: 1,
                unit: DurationUnit::Mo
            }
        );
        assert_eq!(
            kinds("5m")[0],
            TokenKind::Duration {
                n: 5,
                unit: DurationUnit::M
            }
        );
        assert_eq!(
            kinds("500ms")[0],
            TokenKind::Duration {
                n: 500,
                unit: DurationUnit::Ms
            }
        );
    }

    /// Z-010: a date literal must name a day that exists. Month length and
    /// the Gregorian leap rule, not `1..=12` / `1..=31`.
    #[test]
    fn nonexistent_gregorian_dates_are_rejected() {
        for bad in [
            "2026-02-31", // February never has 31 days
            "2026-04-31", // 30-day month
            "2026-06-31",
            "2026-09-31",
            "2026-11-31",
            "2026-02-29", // 2026 is not a leap year
            "2025-02-29",
            "1900-02-29", // century, not a leap year
            "2026-01-00", // day zero is not a day
            "2026-00-10", // month zero is not a month
            "2026-13-01",
        ] {
            assert!(
                err(bad).contains("invalid date"),
                "`{bad}` must not lex as a date"
            );
        }
    }

    #[test]
    fn real_gregorian_dates_are_accepted() {
        for (src, y, m, d) in [
            ("2024-02-29", 2024, 2u8, 29u8), // leap year
            ("2000-02-29", 2000, 2, 29),     // fourth century, leap
            ("2026-02-28", 2026, 2, 28),
            ("2026-12-31", 2026, 12, 31),
            ("2026-04-30", 2026, 4, 30),
        ] {
            assert_eq!(
                kinds(src)[0],
                TokenKind::Date { y, m, d },
                "`{src}` is a real date"
            );
        }
    }

    /// The datetime form carries the same date, so it inherits the same
    /// calendar check — a bad day is rejected before the time is even read.
    #[test]
    fn datetime_inherits_the_calendar_check() {
        assert!(err("2026-02-30T00:00:00Z").contains("invalid date"));
        assert_eq!(
            kinds("2024-02-29T23:59:59Z")[0],
            TokenKind::Datetime {
                y: 2024,
                m: 2,
                d: 29,
                hh: 23,
                mm: 59,
                ss: 59
            }
        );
    }

    #[test]
    fn date_is_not_subtraction_but_spaced_minus_is() {
        use TokenKind::*;
        // Glued: one date literal. Spaced: arithmetic.
        assert_eq!(kinds("2026-07-01").len(), 3);
        assert_eq!(
            kinds("2026 - 7 - 1"),
            vec![Int(2026), Minus, Int(7), Minus, Int(1), Newline, Eof]
        );
    }

    #[test]
    fn decimals_are_exact_text() {
        assert_eq!(kinds("10.50")[0], TokenKind::Decimal("10.50".into()));
        assert!(err("10.50d").contains("decimal"));
    }

    #[test]
    fn newlines_only_at_depth_zero() {
        // The group brace spans lines: no Newline tokens inside it.
        let toks = kinds("group a {\n  n: count,\n  s: sum total }\nsort -s");
        let newlines = toks
            .iter()
            .filter(|k| matches!(k, TokenKind::Newline))
            .count();
        assert_eq!(newlines, 2); // after `}` line and after final line
    }

    #[test]
    fn comments_vanish() {
        assert_eq!(
            kinds("take 20  # the top 20 IS the result"),
            vec![
                TokenKind::Keyword(Keyword::Take),
                TokenKind::Int(20),
                TokenKind::Newline,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn string_escapes_closed_set() {
        assert_eq!(
            kinds(r#""a\"b\\c\nd""#)[0],
            TokenKind::Str("a\"b\\c\nd".into())
        );
        assert!(err(r#""bad \q""#).contains("unknown escape"));
        assert!(err("\"unterminated").contains("unterminated"));
        assert!(err("\"no\nlines\"").contains("span lines"));
    }

    #[test]
    fn loud_rejections() {
        assert!(err("Payees").contains("lowercase"));
        assert!(err("snake_case").contains("lowerCamel"));
        assert!(err("!ready").contains("`not`"));
        assert!(err("5q").contains("duration unit"));
        assert!(err("2026-13-01").contains("invalid date"));
        assert!(err("2026-07-01T14:30:00").contains("Z is mandatory"));
        assert!(err("$3").contains("parameter"));
    }

    #[test]
    fn arrow_and_comparisons() {
        use TokenKind::*;
        assert_eq!(
            kinds("payee -> payees?  sealed indexed"),
            vec![
                Ident("payee".into()),
                Arrow,
                Ident("payees".into()),
                Question,
                Keyword(super::Keyword::Sealed),
                Keyword(super::Keyword::Indexed),
                Newline,
                Eof
            ]
        );
        assert_eq!(kinds("a <= b")[1], LtEq);
        assert_eq!(kinds("a != b")[1], NotEq);
    }

    #[test]
    fn keywords_vs_contextual_names() {
        // `count` is contextual — an ordinary ident, usable as a field name.
        assert_eq!(kinds("count")[0], TokenKind::Ident("count".into()));
        assert_eq!(kinds("sort")[0], TokenKind::Keyword(Keyword::Sort));
    }

    /// Every fully-reserved word of engine-plan G3 lexes as a keyword. The
    /// authorization and platform words were the hole: without them
    /// `entity x { access: int }` looked like an ordinary shape.
    #[test]
    fn fully_reserved_words_lex_as_keywords() {
        for word in [
            // logic and pipeline
            "and",
            "or",
            "not",
            "in",
            "is",
            "null",
            "true",
            "false",
            "match",
            "any",
            "none",
            "on",
            "group",
            "reduce",
            "sort",
            "take",
            "append",
            "let",
            "distinct",
            "under",
            "above",
            "since",
            "contains",
            "starts",
            "matches",
            "set",
            "del",
            "subtree",
            "each",
            "cond", // declarations and clauses
            "entity",
            "op",
            "routine",
            "domain",
            "trigger",
            "when",
            "every",
            "check",
            "index",
            "unique",
            "sealed",
            "indexed",
            "owned",
            "tree",
            "normalize",
            "whole",
            // authorization and platform blocks
            "acl",
            "access",
            "app",
            "page",
            "principal",
            "client",
            "auth",
            "policy",
            "compliance",
            "channel",
            "space", // G9 registration flags
            "unguarded",
            "unbounded",
            "reproducible",
            "nopause",
            "public",
        ] {
            let kw = Keyword::parse(word)
                .unwrap_or_else(|| panic!("`{word}` is reserved in G3 but not in the lexer"));
            assert_eq!(kw.as_str(), word, "`{word}` round-trips through as_str");
            assert_eq!(
                kinds(word)[0],
                TokenKind::Keyword(kw),
                "`{word}` must lex as a keyword"
            );
        }
    }

    /// The contextual tier stays `Ident` — reserving these would steal the most
    /// natural field names in the language for no parsing benefit (G3).
    #[test]
    fn contextual_words_stay_idents() {
        for word in [
            // access modes: positional after `access` / inside `acl (…)`
            "read",
            "write",
            "admin", // block-internal words
            "run",
            "method",
            "methods",
            "share",
            "rollup",
            "live",
            "table",
            "metric",
            "detail",
            "route", // aggregates and functions
            "count",
            "sum",
            "avg",
            "min",
            "max",
            "median",
            "percentile",
            "joined",
            "coalesce",
            "round",
        ] {
            assert!(
                Keyword::parse(word).is_none(),
                "`{word}` is contextual, not a keyword"
            );
            assert_eq!(
                kinds(word)[0],
                TokenKind::Ident(word.into()),
                "`{word}` must lex as an ident"
            );
        }
    }

    /// The two tiers, where it bites: a contextual word is a usable field name,
    /// a fully-reserved word is not — the shape parser sees a keyword instead
    /// of a name and can say so, which is the whole point of reserving it.
    #[test]
    fn contextual_word_is_a_legal_field_name_reserved_word_is_not() {
        use TokenKind::*;
        assert_eq!(
            kinds("entity account {\n  read: int\n}"),
            vec![
                Keyword(super::Keyword::Entity),
                Ident("account".into()),
                LBrace,
                Ident("read".into()), // a name, not a keyword
                Colon,
                Ident("int".into()),
                RBrace,
                Newline,
                Eof
            ]
        );
        // Same shape, reserved word in the field-name slot: it lexes as a
        // keyword, so no name can ever be bound to it.
        let toks = kinds("entity account {\n  access: int\n}");
        assert_eq!(toks[3], Keyword(super::Keyword::Access));
        assert!(!matches!(toks[3], Ident(_)));
        // …and the same for a registration flag used as a field name.
        assert_eq!(
            kinds("entity job {\n  unbounded: bool\n}")[3],
            Keyword(super::Keyword::Unbounded)
        );
    }

    /// G9: `op name($p) unguarded = …` — the flags must not read as idents, or
    /// an op named `unbounded` parses.
    #[test]
    fn registration_flags_lex_in_the_wrapper() {
        use TokenKind::*;
        assert_eq!(
            kinds("op purge($id) unguarded unbounded reproducible = orders"),
            vec![
                Keyword(super::Keyword::Op),
                Ident("purge".into()),
                LParen,
                Param("id".into()),
                RParen,
                Keyword(super::Keyword::Unguarded),
                Keyword(super::Keyword::Unbounded),
                Keyword(super::Keyword::Reproducible),
                Eq,
                Ident("orders".into()),
                Newline,
                Eof
            ]
        );
    }
}
