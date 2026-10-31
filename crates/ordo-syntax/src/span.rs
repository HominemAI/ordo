//! Source positions. Every token and every diagnostic carries one.

/// Byte range into the source, plus 1-based line/column of its start —
/// errors always name a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub col: u32,
}

impl Span {
    pub fn new(start: usize, end: usize, line: u32, col: u32) -> Self {
        Span {
            start,
            end,
            line,
            col,
        }
    }

    /// Widen to cover both spans (diagnostics over multi-token constructs).
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
            line: self.line.min(other.line),
            col: if other.line < self.line {
                other.col
            } else {
                self.col
            },
        }
    }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}
