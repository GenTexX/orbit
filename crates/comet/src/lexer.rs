//! comet lexer: source text to a flat token stream, byte-position tracked. Hand-
//! written (no lexer-generator dependency) since the whole pipeline is
//! deliberately simple and fast rather than optimized (ADR 0007).

use crate::diagnostic::Diagnostic;
use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// A whole number written without a decimal point: `5`. An `int`.
    Int(i64),
    /// A number written with a decimal point: `5.0`. An `f32`.
    ///
    /// The two are separate at the token level because the distinction is
    /// entirely in how it was written, and the parser should not have to
    /// re-read the source to recover it.
    Number(f64),
    Str(String),
    Ident(String),

    Func,
    Let,
    If,
    Else,
    While,
    For,
    In,
    Return,
    True,
    False,

    LBrace,
    RBrace,
    LParen,
    RParen,
    Comma,
    Dot,
    /// `@`, which starts an annotation.
    At,
    /// `..`, which only ever appears in a `for` range.
    DotDot,
    Semicolon,
    Colon,
    Arrow,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,

    Eq,
    EqEq,
    NotEq,
    Lt,
    Gt,
    Le,
    Ge,
    AndAnd,
    OrOr,
    Bang,

    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,

    /// A `//` line comment, including the slashes. Only [`lex_with_comments`]
    /// reports these - [`lex`] drops them, because nothing downstream of the
    /// parser has any use for one.
    Comment,

    /// The sentinel that always ends a token stream, so the parser never has to
    /// special-case running off the end - it just sees `Eof` forever.
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Lex `source` into a flat token stream (always ending in exactly one `Eof`
/// token) plus any diagnostics for characters or literals the lexer could not
/// make sense of. Never fails outright - an unrecognized character is skipped
/// and reported, not a hard error, so the parser downstream still sees
/// everything else (ADR 0010).
pub fn lex(source: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    let (mut tokens, diagnostics) = lex_with_comments(source);
    tokens.retain(|token| token.kind != TokenKind::Comment);
    (tokens, diagnostics)
}

/// The same, keeping comments in the stream.
///
/// Syntax highlighting is the one consumer that wants them: everything else
/// downstream reads code, and a comment is by definition not code. Keeping one
/// lexer rather than a second scanner in the highlighter is what stops `//`
/// inside a string literal from being colored as a comment.
pub fn lex_with_comments(source: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    Lexer::new(source).run()
}

struct Lexer<'src> {
    source: &'src str,
    bytes: &'src [u8],
    pos: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'src> Lexer<'src> {
    fn new(source: &'src str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> (Vec<Token>, Vec<Diagnostic>) {
        while let Some(c) = self.peek() {
            let start = self.pos;
            match c {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                b'/' if self.peek_at(1) == Some(b'/') => self.lex_line_comment(start),
                b'/' if self.peek_at(1) == Some(b'*') => self.lex_block_comment(start),
                b'"' => self.lex_string(start),
                b'0'..=b'9' => self.lex_number(start),
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.lex_ident_or_keyword(start),
                _ => self.lex_punct(start),
            }
        }
        self.push(TokenKind::Eof, self.pos, self.pos);
        (self.tokens, self.diagnostics)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start as u32, end as u32),
        });
    }

    /// A `/* ... */` comment. Unterminated is reported rather than swallowing
    /// the rest of the file in silence - while typing, the opening two
    /// characters exist long before the closing two do.
    fn lex_block_comment(&mut self, start: usize) {
        self.pos += 2;
        loop {
            match self.peek() {
                None => {
                    self.diagnostics.push(Diagnostic::error(
                        Span::new(start as u32, (start + 2) as u32),
                        "unterminated block comment",
                    ));
                    break;
                }
                Some(b'*') if self.peek_at(1) == Some(b'/') => {
                    self.pos += 2;
                    break;
                }
                Some(_) => self.pos += 1,
            }
        }
        self.push(TokenKind::Comment, start, self.pos);
    }

    fn lex_line_comment(&mut self, start: usize) {
        while let Some(c) = self.peek() {
            if c == b'\n' {
                break;
            }
            self.pos += 1;
        }
        self.push(TokenKind::Comment, start, self.pos);
    }

    fn lex_string(&mut self, start: usize) {
        self.pos += 1; // opening quote
        let mut value = String::new();
        loop {
            match self.peek() {
                None => {
                    self.diagnostics.push(Diagnostic::error(
                        Span::new(start as u32, self.pos as u32),
                        "unterminated string literal",
                    ));
                    break;
                }
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'n') => value.push('\n'),
                        Some(b't') => value.push('\t'),
                        Some(b'"') => value.push('"'),
                        Some(b'\\') => value.push('\\'),
                        Some(other) => value.push(other as char),
                        None => break,
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    let rest = &self.source[self.pos..];
                    let ch = rest.chars().next().expect("peek confirmed a byte exists");
                    value.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
        self.push(TokenKind::Str(value), start, self.pos);
    }

    fn lex_number(&mut self, start: usize) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.peek() == Some(b'.') && matches!(self.peek_at(1), Some(b'0'..=b'9')) {
            self.pos += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = &self.source[start..self.pos];
        let kind = if text.contains('.') {
            let value: f64 = text.parse().expect("lexed only digits and one dot");
            TokenKind::Number(value)
        } else {
            // Out of range is reported here rather than silently wrapping. i64
            // is the carrier so the checker can say what the limit is; the
            // language's `int` is 32-bit.
            match text.parse::<i64>() {
                Ok(value) => TokenKind::Int(value),
                Err(_) => {
                    self.diagnostics.push(Diagnostic::error(
                        Span::new(start as u32, self.pos as u32),
                        format!("`{text}` is too large for an int"),
                    ));
                    TokenKind::Int(0)
                }
            }
        };
        self.push(kind, start, self.pos);
    }

    fn lex_ident_or_keyword(&mut self, start: usize) {
        while matches!(
            self.peek(),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        ) {
            self.pos += 1;
        }
        let text = &self.source[start..self.pos];
        let kind = match text {
            "func" => TokenKind::Func,
            "let" => TokenKind::Let,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "return" => TokenKind::Return,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            _ => TokenKind::Ident(text.to_string()),
        };
        self.push(kind, start, self.pos);
    }

    fn lex_punct(&mut self, start: usize) {
        let two = self.peek_at(1);
        let (kind, len) = match (self.peek(), two) {
            (Some(b'@'), _) => (TokenKind::At, 1),
            (Some(b'.'), Some(b'.')) => (TokenKind::DotDot, 2),
            (Some(b'='), Some(b'=')) => (TokenKind::EqEq, 2),
            (Some(b'!'), Some(b'=')) => (TokenKind::NotEq, 2),
            (Some(b'<'), Some(b'=')) => (TokenKind::Le, 2),
            (Some(b'>'), Some(b'=')) => (TokenKind::Ge, 2),
            (Some(b'&'), Some(b'&')) => (TokenKind::AndAnd, 2),
            (Some(b'|'), Some(b'|')) => (TokenKind::OrOr, 2),
            (Some(b'+'), Some(b'=')) => (TokenKind::PlusEq, 2),
            (Some(b'-'), Some(b'=')) => (TokenKind::MinusEq, 2),
            (Some(b'*'), Some(b'=')) => (TokenKind::StarEq, 2),
            (Some(b'/'), Some(b'=')) => (TokenKind::SlashEq, 2),
            (Some(b'%'), Some(b'=')) => (TokenKind::PercentEq, 2),
            (Some(b'-'), Some(b'>')) => (TokenKind::Arrow, 2),
            (Some(b'{'), _) => (TokenKind::LBrace, 1),
            (Some(b'}'), _) => (TokenKind::RBrace, 1),
            (Some(b'('), _) => (TokenKind::LParen, 1),
            (Some(b')'), _) => (TokenKind::RParen, 1),
            (Some(b','), _) => (TokenKind::Comma, 1),
            (Some(b'.'), _) => (TokenKind::Dot, 1),
            (Some(b';'), _) => (TokenKind::Semicolon, 1),
            (Some(b':'), _) => (TokenKind::Colon, 1),
            (Some(b'+'), _) => (TokenKind::Plus, 1),
            (Some(b'-'), _) => (TokenKind::Minus, 1),
            (Some(b'*'), _) => (TokenKind::Star, 1),
            (Some(b'/'), _) => (TokenKind::Slash, 1),
            (Some(b'%'), _) => (TokenKind::Percent, 1),
            (Some(b'='), _) => (TokenKind::Eq, 1),
            (Some(b'<'), _) => (TokenKind::Lt, 1),
            (Some(b'>'), _) => (TokenKind::Gt, 1),
            (Some(b'!'), _) => (TokenKind::Bang, 1),
            (Some(_), _) => {
                // Advance by the whole character, not one byte. A multi-byte
                // character consumed a byte at a time yields one nonsense
                // diagnostic per byte, each spanning a cut through the middle of
                // it - and a span that is not a char boundary panics anything
                // downstream that slices the source with it, which an editor
                // drawing a squiggle does.
                let rest = &self.source[start..];
                let ch = rest.chars().next().expect("peek confirmed a byte exists");
                self.pos += ch.len_utf8();
                self.diagnostics.push(Diagnostic::error(
                    Span::new(start as u32, self.pos as u32),
                    format!("unexpected character {ch:?}"),
                ));
                return;
            }
            (None, _) => unreachable!("run() only calls lex_punct when peek() is Some"),
        };
        self.pos += len;
        self.push(kind, start, self.pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        let (tokens, diagnostics) = lex(source);
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );
        tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_a_let_statement() {
        assert_eq!(
            kinds("let speed = 120.0;"),
            vec![
                TokenKind::Let,
                TokenKind::Ident("speed".to_string()),
                TokenKind::Eq,
                TokenKind::Number(120.0),
                TokenKind::Semicolon,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn a_literal_too_large_to_carry_is_reported_rather_than_wrapped() {
        let (_, diagnostics) = lex("99999999999999999999999");
        assert_eq!(
            diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
            ["`99999999999999999999999` is too large for an int"]
        );
    }

    #[test]
    fn a_literal_without_a_decimal_point_is_an_int() {
        assert_eq!(kinds("200"), vec![TokenKind::Int(200), TokenKind::Eof]);
        assert_eq!(
            kinds("200.0"),
            vec![TokenKind::Number(200.0), TokenKind::Eof]
        );
        // How it is written is the whole distinction, so `200.` - a dot with no
        // digit after it - is still an int followed by a field access.
        assert_eq!(
            kinds("200.x"),
            vec![
                TokenKind::Int(200),
                TokenKind::Dot,
                TokenKind::Ident("x".into()),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn two_char_operators_win_over_their_one_char_prefix() {
        assert_eq!(
            kinds("+= -= *= /= == != <= >= && || ->"),
            vec![
                TokenKind::PlusEq,
                TokenKind::MinusEq,
                TokenKind::StarEq,
                TokenKind::SlashEq,
                TokenKind::EqEq,
                TokenKind::NotEq,
                TokenKind::Le,
                TokenKind::Ge,
                TokenKind::AndAnd,
                TokenKind::OrOr,
                TokenKind::Arrow,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn line_comments_are_skipped_entirely() {
        assert_eq!(
            kinds("let x = 1.0; // trailing note\nlet y = 2.0;"),
            vec![
                TokenKind::Let,
                TokenKind::Ident("x".to_string()),
                TokenKind::Eq,
                TokenKind::Number(1.0),
                TokenKind::Semicolon,
                TokenKind::Let,
                TokenKind::Ident("y".to_string()),
                TokenKind::Eq,
                TokenKind::Number(2.0),
                TokenKind::Semicolon,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn string_literals_decode_escapes() {
        let (tokens, diagnostics) = lex(r#""a\nb\"c""#);
        assert!(diagnostics.is_empty());
        assert_eq!(tokens[0].kind, TokenKind::Str("a\nb\"c".to_string()));
    }

    #[test]
    fn keywords_are_not_identifiers() {
        assert_eq!(
            kinds("func let if else while for in return true false"),
            vec![
                TokenKind::Func,
                TokenKind::Let,
                TokenKind::If,
                TokenKind::Else,
                TokenKind::While,
                TokenKind::For,
                TokenKind::In,
                TokenKind::Return,
                TokenKind::True,
                TokenKind::False,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn an_unterminated_string_is_reported_not_panicked() {
        let (_, diagnostics) = lex("\"never closed");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "unterminated string literal");
    }

    #[test]
    fn an_unknown_character_is_reported_and_skipped() {
        let (tokens, diagnostics) = lex("let x = 1.0 # 2.0;");
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains('#'));
        // Lexing continued past the bad character instead of stopping.
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Number(2.0)));
    }

    #[test]
    fn a_stray_multibyte_character_is_one_error_spanning_the_whole_character() {
        // A byte-at-a-time skip reported one error per byte, each spanning a cut
        // through the middle of the character - and a span that is not a char
        // boundary panics anything that slices the source with it, which an
        // editor drawing a squiggle does.
        let source = "let x = \u{fc};";
        let (_, diagnostics) = lex(source);
        assert_eq!(
            diagnostics.len(),
            1,
            "one character, one error: {diagnostics:?}"
        );
        assert_eq!(diagnostics[0].span.text(source), "\u{fc}");
        assert!(diagnostics[0].message.contains('\u{fc}'), "and names it");
        assert!(source.is_char_boundary(diagnostics[0].span.start as usize));
        assert!(source.is_char_boundary(diagnostics[0].span.end as usize));
    }

    #[test]
    fn lexing_continues_past_a_multibyte_character() {
        let (tokens, _) = lex("\u{1f600} 42");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Int(42)));
    }

    #[test]
    fn every_token_carries_the_span_it_covers() {
        let (tokens, _) = lex("speed");
        assert_eq!(tokens[0].span.text("speed"), "speed");
    }
}
