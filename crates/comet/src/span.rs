//! comet source spans: byte-offset ranges into the original source text, carried
//! by every token and AST node so diagnostics can point at exactly what is wrong.

/// A byte-offset range `[start, end)` into a script's source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// A span covering both `self` and `other`, from the earlier start to the
    /// later end - for building a parent node's span from its children's.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }

    /// The source text this span covers.
    pub fn text(self, source: &str) -> &str {
        &source[self.start as usize..self.end as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_covers_both_spans_regardless_of_order() {
        let a = Span::new(5, 10);
        let b = Span::new(2, 7);
        assert_eq!(a.to(b), Span::new(2, 10));
        assert_eq!(b.to(a), Span::new(2, 10));
    }

    #[test]
    fn text_slices_the_source() {
        let source = "let speed = 1.0;";
        assert_eq!(Span::new(4, 9).text(source), "speed");
    }
}
