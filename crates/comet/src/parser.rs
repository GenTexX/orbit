//! comet parser: tokens to an [`ast::Script`], hand-written recursive descent
//! with precedence climbing for binary operators.
//!
//! **Error tolerance is the design constraint, not a nicety** (ADR 0010). The
//! editor re-parses on every keystroke, so source is malformed most of the time
//! it is seen. The parser therefore never stops at the first problem: it records
//! a diagnostic, substitutes [`Expr::Error`] or skips to a recovery point, and
//! keeps going, so a typo on line 2 still leaves lines 3 onward parsed and
//! checkable.

use crate::ast::*;
use crate::diagnostic::Diagnostic;
use crate::lexer::{Token, TokenKind, lex};
use crate::span::Span;

/// Parse `source` into a script plus every diagnostic found lexing or parsing it.
/// Always returns a script - a wholly unparseable file yields an empty one, not
/// an error.
pub fn parse(source: &str) -> (Script, Vec<Diagnostic>) {
    let (tokens, mut diagnostics) = lex(source);
    let mut parser = Parser {
        tokens,
        pos: 0,
        diagnostics: Vec::new(),
        depth: 0,
        depth_reported: false,
    };
    let script = parser.script();
    diagnostics.append(&mut parser.diagnostics);
    diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
    (script, diagnostics)
}

/// How deeply expressions and blocks may nest before the parser gives up.
///
/// A recursive-descent parser uses stack proportional to nesting, and nothing
/// stops a file from containing a hundred thousand open parentheses - typed by
/// hand, no, but pasted or generated, yes. Overflowing the stack aborts the
/// process, which in an editor means the file being edited is gone. Reporting
/// the depth and stopping is the only outcome that keeps the editor alive.
///
/// The limit is far above anything a person writes: real code nests single
/// digits deep, and the deepest expression in comet's own fixtures is four.
const MAX_DEPTH: u32 = 128;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    /// How deep the current descent is. See [`MAX_DEPTH`].
    depth: u32,
    /// Whether the depth limit has already been reported, so one runaway file
    /// yields one diagnostic rather than one per level on the way back out.
    depth_reported: bool,
}

impl Parser {
    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos.min(self.tokens.len() - 1)].kind
    }

    fn peek_span(&self) -> Span {
        self.tokens[self.pos.min(self.tokens.len() - 1)].span
    }

    fn at_end(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos.min(self.tokens.len() - 1)].clone();
        if !self.at_end() {
            self.pos += 1;
        }
        token
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        // Past the depth limit the parser is unwinding through a hundred levels
        // of half-parsed groups and then recovering into the wreckage. None of
        // what it finds there is a fact about the file, and a hundred thousand
        // diagnostics is its own kind of crash.
        if self.depth_reported {
            return;
        }
        self.diagnostics.push(Diagnostic::error(span, message));
    }

    /// Consume `kind` or report that it was missing. Returns whether it was
    /// there - callers use that to decide whether to recover, never to bail.
    fn expect(&mut self, kind: &TokenKind, what: &str) -> bool {
        if self.eat(kind) {
            return true;
        }
        let span = self.peek_span();
        self.error(span, format!("expected {what}"));
        false
    }

    fn ident(&mut self, what: &str) -> (String, Span) {
        if let TokenKind::Ident(name) = self.peek().clone() {
            let span = self.peek_span();
            self.advance();
            (name, span)
        } else {
            let span = self.peek_span();
            self.error(span, format!("expected {what}"));
            (String::new(), span)
        }
    }

    fn type_name(&mut self) -> TypeName {
        let (name, span) = self.ident("a type name");
        TypeName { name, span }
    }

    // --- top level ---

    fn script(&mut self) -> Script {
        let mut script = Script::default();
        while !self.at_end() {
            // Annotations attach to whatever declaration follows them.
            let annotations = self.annotations();
            match self.peek() {
                TokenKind::Func => {
                    if let Some(f) = self.function() {
                        script.functions.push(f);
                    }
                }
                TokenKind::Let => {
                    if let Some(s) = self.state_decl(annotations) {
                        script.state.push(s);
                    }
                }
                _ => {
                    let span = self.peek_span();
                    self.error(span, "expected `func` or `let` at the top level");
                    self.recover_to_top_level();
                }
            }
        }
        script
    }

    /// Skip forward to the next thing that can start a top-level item, so one
    /// piece of garbage costs one diagnostic rather than one per token after it.
    fn recover_to_top_level(&mut self) {
        while !self.at_end()
            && !matches!(
                self.peek(),
                TokenKind::Func | TokenKind::Let | TokenKind::At
            )
        {
            self.advance();
        }
    }

    /// Any run of `@name` or `@name(a, b)` before a declaration.
    ///
    /// Parsed generally even though `@export` is the only one the checker knows:
    /// the whole reason for the `@` prefix is that the ones taking arguments
    /// need no second syntax when they arrive.
    fn annotations(&mut self) -> Vec<Annotation> {
        let mut found = Vec::new();
        while matches!(self.peek(), TokenKind::At) {
            let start = self.peek_span();
            self.advance();
            let (name, name_span) = self.ident("an annotation name");
            let mut args = Vec::new();
            if self.eat(&TokenKind::LParen) {
                while !self.at_end() && !matches!(self.peek(), TokenKind::RParen) {
                    args.push(self.expr());
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "`)` after an annotation's arguments");
            }
            let end = self.peek_span();
            found.push(Annotation {
                name,
                name_span,
                args,
                span: start.to(end),
            });
        }
        found
    }

    fn state_decl(&mut self, annotations: Vec<Annotation>) -> Option<StateDecl> {
        let start = annotations
            .first()
            .map_or_else(|| self.peek_span(), |a| a.span);
        self.advance(); // `let`
        let (name, name_span) = self.ident("a variable name");
        let ty = self.eat(&TokenKind::Colon).then(|| self.type_name());
        // The initializer is optional, which is what lets an exported variable
        // carry only a type. Whether it may be left out at all is the checker's
        // call, since it depends on there being a type to default from.
        let init = self.eat(&TokenKind::Eq).then(|| self.expr());
        let end = self.peek_span();
        self.expect(&TokenKind::Semicolon, "`;` after a `let` declaration");
        Some(StateDecl {
            annotations,
            name,
            name_span,
            ty,
            init,
            span: start.to(end),
        })
    }

    fn function(&mut self) -> Option<Function> {
        let start = self.peek_span();
        self.advance(); // `func`
        let (name, name_span) = self.ident("a function name");

        let mut params = Vec::new();
        if self.expect(&TokenKind::LParen, "`(` after a function name") {
            while !self.at_end() && !matches!(self.peek(), TokenKind::RParen) {
                let p_start = self.peek_span();
                let (p_name, p_name_span) = self.ident("a parameter name");
                self.expect(&TokenKind::Colon, "`:` after a parameter name");
                let ty = self.type_name();
                params.push(Param {
                    name: p_name,
                    name_span: p_name_span,
                    span: p_start.to(ty.span),
                    ty,
                });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "`)` after the parameter list");
        }

        let ret = self.eat(&TokenKind::Arrow).then(|| self.type_name());

        if !matches!(self.peek(), TokenKind::LBrace) {
            let span = self.peek_span();
            self.error(span, "expected `{` to open the function body");
            self.recover_to_top_level();
            return None;
        }
        let body = self.block();
        Some(Function {
            name,
            name_span,
            params,
            ret,
            span: start.to(body.span),
            body,
        })
    }

    // --- statements ---

    fn block(&mut self) -> Block {
        let start = self.peek_span();
        // Blocks nest through `if`/`while`/`for` bodies, and each level is
        // another frame - so they are counted against the same budget as
        // expressions. An empty block with the tokens left alone is enough:
        // the enclosing statement's recovery takes it from here.
        if self.depth >= MAX_DEPTH {
            self.too_deep();
            return Block {
                stmts: Vec::new(),
                tail: None,
                span: start,
            };
        }
        self.depth += 1;
        let block = self.block_inner(start);
        self.depth -= 1;
        block
    }

    fn block_inner(&mut self, start: Span) -> Block {
        self.expect(&TokenKind::LBrace, "`{`");
        let mut stmts = Vec::new();
        let mut tail = None;

        while !self.at_end() && !matches!(self.peek(), TokenKind::RBrace) {
            // A nested `func` means the enclosing block was never closed. Stop
            // here and let the top level resume, rather than swallowing the
            // whole rest of the file into this block (fixture 5's shape).
            if matches!(self.peek(), TokenKind::Func) {
                break;
            }
            let before = self.pos;
            match self.stmt_or_tail() {
                StmtOrTail::Stmt(s) => stmts.push(s),
                StmtOrTail::Tail(e) => {
                    tail = Some(Box::new(e));
                    break;
                }
            }
            // A statement that consumed nothing would spin forever; force progress.
            if self.pos == before {
                self.advance();
            }
        }

        let end = self.peek_span();
        self.expect(&TokenKind::RBrace, "`}` to close this block");
        Block {
            stmts,
            tail,
            span: start.to(end),
        }
    }

    fn stmt_or_tail(&mut self) -> StmtOrTail {
        let start = self.peek_span();
        match self.peek() {
            TokenKind::Let => {
                self.advance();
                let (name, name_span) = self.ident("a variable name");
                let ty = self.eat(&TokenKind::Colon).then(|| self.type_name());
                self.expect(&TokenKind::Eq, "`=` after a `let` declaration");
                let init = self.expr();
                let end = self.peek_span();
                self.expect(&TokenKind::Semicolon, "`;` after a `let` declaration");
                StmtOrTail::Stmt(Stmt::Let {
                    name,
                    name_span,
                    ty,
                    init,
                    span: start.to(end),
                })
            }
            TokenKind::If => StmtOrTail::Stmt(Stmt::If(self.if_stmt())),
            TokenKind::While => {
                self.advance();
                let cond = self.expr();
                let body = self.block();
                StmtOrTail::Stmt(Stmt::While {
                    cond,
                    span: start.to(body.span),
                    body,
                })
            }
            TokenKind::For => {
                self.advance();
                let (name, name_span) = self.ident("a loop variable");
                self.expect(&TokenKind::In, "`in`");
                // The bounds are parsed without the range operator in the
                // expression grammar, so `..` cannot appear anywhere else and
                // does not need a precedence level of its own.
                let start_bound = self.expr();
                self.expect(&TokenKind::DotDot, "`..`");
                let end_bound = self.expr();
                let body = self.block();
                StmtOrTail::Stmt(Stmt::For {
                    name,
                    name_span,
                    start: start_bound,
                    end: end_bound,
                    span: start.to(body.span),
                    body,
                })
            }
            TokenKind::Return => {
                self.advance();
                let value = if matches!(self.peek(), TokenKind::Semicolon) {
                    None
                } else {
                    Some(self.expr())
                };
                let end = self.peek_span();
                self.expect(&TokenKind::Semicolon, "`;` after `return`");
                StmtOrTail::Stmt(Stmt::Return {
                    value,
                    span: start.to(end),
                })
            }
            _ => {
                let expr = self.expr();
                // An assignment operator here makes this a statement, not a tail.
                let op = match self.peek() {
                    TokenKind::Eq => Some(AssignOp::Set),
                    TokenKind::PlusEq => Some(AssignOp::Add),
                    TokenKind::MinusEq => Some(AssignOp::Sub),
                    TokenKind::StarEq => Some(AssignOp::Mul),
                    TokenKind::SlashEq => Some(AssignOp::Div),
                    TokenKind::PercentEq => Some(AssignOp::Rem),
                    _ => None,
                };
                if let Some(op) = op {
                    self.advance();
                    let value = self.expr();
                    let end = self.peek_span();
                    self.expect(&TokenKind::Semicolon, "`;` after an assignment");
                    return StmtOrTail::Stmt(Stmt::Assign {
                        target: expr,
                        op,
                        value,
                        span: start.to(end),
                    });
                }
                // No semicolon and the block ends here: this is the tail
                // expression - the block's value.
                if matches!(self.peek(), TokenKind::RBrace) {
                    return StmtOrTail::Tail(expr);
                }
                let end = self.peek_span();
                self.expect(&TokenKind::Semicolon, "`;` after an expression statement");
                StmtOrTail::Stmt(Stmt::Expr {
                    span: start.to(end),
                    expr,
                })
            }
        }
    }

    fn if_stmt(&mut self) -> IfStmt {
        let start = self.peek_span();
        self.advance(); // `if`
        let cond = self.expr();
        let then = self.block();
        let mut otherwise = None;
        let mut end = then.span;
        if self.eat(&TokenKind::Else) {
            if matches!(self.peek(), TokenKind::If) {
                let nested = self.if_stmt();
                end = nested.span;
                otherwise = Some(Box::new(Else::If(nested)));
            } else {
                let block = self.block();
                end = block.span;
                otherwise = Some(Box::new(Else::Block(block)));
            }
        }
        IfStmt {
            cond,
            then,
            otherwise,
            span: start.to(end),
        }
    }

    // --- expressions (precedence climbing) ---

    fn expr(&mut self) -> Expr {
        // The one place expressions are entered from, so this is the one place
        // the depth has to be counted.
        if self.depth >= MAX_DEPTH {
            return self.too_deep();
        }
        self.depth += 1;
        let expr = self.binary(0);
        self.depth -= 1;
        expr
    }

    /// Give up on a construct that nests past [`MAX_DEPTH`], reporting it once.
    ///
    /// The tokens are left where they are. Recovery at the statement level will
    /// skip to the next `;` or `}`, which is the same thing it does for any
    /// other malformed expression.
    fn too_deep(&mut self) -> Expr {
        let span = self.peek_span();
        if !self.depth_reported {
            self.error(span, format!("nested more than {MAX_DEPTH} levels deep"));
            self.depth_reported = true;
        }
        Expr::Number { value: 0.0, span }
    }

    fn binary(&mut self, min_prec: u8) -> Expr {
        let mut lhs = self.unary();
        while let Some((op, prec)) = binary_op(self.peek()) {
            if prec < min_prec {
                break;
            }
            self.advance();
            // All binary operators here are left-associative, so the right side
            // binds only tighter-or-equal operators.
            let rhs = self.binary(prec + 1);
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        lhs
    }

    fn unary(&mut self) -> Expr {
        let start = self.peek_span();
        let op = match self.peek() {
            TokenKind::Minus => Some(UnaryOp::Neg),
            TokenKind::Bang => Some(UnaryOp::Not),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let operand = self.unary();
            let span = start.to(operand.span());
            return Expr::Unary {
                op,
                operand: Box::new(operand),
                span,
            };
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Expr {
        let mut expr = self.primary();
        while self.eat(&TokenKind::Dot) {
            let (field, field_span) = self.ident("a field name");
            let span = expr.span().to(field_span);
            expr = Expr::Field {
                receiver: Box::new(expr),
                field,
                field_span,
                span,
            };
        }
        expr
    }

    fn primary(&mut self) -> Expr {
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::Int(value) => {
                self.advance();
                Expr::Int { value, span }
            }
            TokenKind::Number(value) => {
                self.advance();
                Expr::Number { value, span }
            }
            TokenKind::True => {
                self.advance();
                Expr::Bool { value: true, span }
            }
            TokenKind::False => {
                self.advance();
                Expr::Bool { value: false, span }
            }
            TokenKind::Str(value) => {
                self.advance();
                Expr::Str { value, span }
            }
            TokenKind::Ident(name) => {
                self.advance();
                if self.eat(&TokenKind::LParen) {
                    let mut args = Vec::new();
                    while !self.at_end() && !matches!(self.peek(), TokenKind::RParen) {
                        args.push(self.expr());
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self.peek_span();
                    self.expect(&TokenKind::RParen, "`)` after the arguments");
                    return Expr::Call {
                        callee: name,
                        callee_span: span,
                        args,
                        span: span.to(end),
                    };
                }
                Expr::Ident { name, span }
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.expr();
                self.expect(&TokenKind::RParen, "`)` to close the group");
                inner
            }
            _ => {
                self.error(span, "expected an expression");
                Expr::Error { span }
            }
        }
    }
}

enum StmtOrTail {
    Stmt(Stmt),
    Tail(Expr),
}

/// The binary operator a token starts, and its binding power (higher binds
/// tighter). Mirrors the usual C/Rust precedence so nothing surprises anyone.
fn binary_op(kind: &TokenKind) -> Option<(BinaryOp, u8)> {
    Some(match kind {
        TokenKind::OrOr => (BinaryOp::Or, 1),
        TokenKind::AndAnd => (BinaryOp::And, 2),
        TokenKind::EqEq => (BinaryOp::Eq, 3),
        TokenKind::NotEq => (BinaryOp::NotEq, 3),
        TokenKind::Lt => (BinaryOp::Lt, 4),
        TokenKind::Gt => (BinaryOp::Gt, 4),
        TokenKind::Le => (BinaryOp::Le, 4),
        TokenKind::Ge => (BinaryOp::Ge, 4),
        TokenKind::Plus => (BinaryOp::Add, 5),
        TokenKind::Minus => (BinaryOp::Sub, 5),
        TokenKind::Star => (BinaryOp::Mul, 6),
        TokenKind::Slash => (BinaryOp::Div, 6),
        TokenKind::Percent => (BinaryOp::Rem, 6),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(source: &str) -> Script {
        let (script, diagnostics) = parse(source);
        assert!(
            diagnostics.is_empty(),
            "expected a clean parse, got: {diagnostics:?}"
        );
        script
    }

    // --- the plan's three valid fixtures, verbatim ---

    #[test]
    fn fixture_1_bouncing_node_parses_clean() {
        let script = parse_ok(include_str!("../tests/fixtures/bounce.cmt"));
        assert_eq!(
            script.state.len(),
            2,
            "speed and direction are script state"
        );
        assert_eq!(script.state[0].name, "speed");
        assert_eq!(script.functions.len(), 1);
        assert_eq!(script.functions[0].name, "update");
        assert_eq!(script.functions[0].params[0].name, "dt");
        assert_eq!(script.functions[0].params[0].ty.name, "f32");
        // `pos.x += ...;` then two `if`s.
        assert_eq!(script.functions[0].body.stmts.len(), 3);
        assert!(matches!(
            script.functions[0].body.stmts[0],
            Stmt::Assign {
                op: AssignOp::Add,
                ..
            }
        ));
    }

    #[test]
    fn fixture_2_string_and_print_parses_clean() {
        let script = parse_ok(include_str!("../tests/fixtures/ticker.cmt"));
        assert_eq!(script.state.len(), 1);
        let Stmt::If(if_stmt) = &script.functions[0].body.stmts[1] else {
            panic!("expected an if statement");
        };
        let Stmt::Expr {
            expr: Expr::Call { callee, args, .. },
            ..
        } = &if_stmt.then.stmts[0]
        else {
            panic!("expected a print call");
        };
        assert_eq!(callee, "print");
        assert!(matches!(args[0], Expr::Str { .. }));
    }

    #[test]
    fn fixture_3_tail_expression_is_the_blocks_value() {
        let script = parse_ok(include_str!("../tests/fixtures/clamp.cmt"));
        let clamp = &script.functions[0];
        assert_eq!(clamp.name, "clamp");
        assert_eq!(clamp.params.len(), 3);
        assert_eq!(
            clamp.ret.as_ref().expect("a declared return type").name,
            "f32"
        );
        // Two explicit early returns as statements...
        assert_eq!(clamp.body.stmts.len(), 2);
        assert!(matches!(clamp.body.stmts[0], Stmt::If(_)));
        // ...and a bare trailing `value` as the block's tail, not a statement.
        let tail = clamp.body.tail.as_ref().expect("a tail expression");
        assert!(matches!(**tail, Expr::Ident { ref name, .. } if name == "value"));
    }

    // --- the plan's two broken fixtures ---

    #[test]
    fn fixture_4_type_error_still_parses_fine() {
        // A type error is not a parse error: this file is syntactically perfect.
        let script = parse_ok(include_str!("../tests/fixtures/type_error.cmt"));
        assert_eq!(script.functions.len(), 1);
        assert_eq!(script.functions[0].body.stmts.len(), 2);
    }

    #[test]
    fn fixture_5_missing_braces_report_but_do_not_cascade() {
        let (script, diagnostics) = parse(include_str!("../tests/fixtures/unclosed.cmt"));
        assert!(
            !diagnostics.is_empty(),
            "the missing braces must be reported"
        );
        // Recovery worked: the function and the statements before the break were
        // still parsed, rather than the whole file being lost.
        assert_eq!(script.functions.len(), 1);
        assert_eq!(script.functions[0].name, "update");
        assert!(
            script.functions[0].body.stmts.len() >= 3,
            "the let, the assignment, and the if should all have parsed"
        );
    }

    // --- recovery and structure ---

    #[test]
    fn a_broken_function_does_not_hide_the_next_one() {
        let (script, diagnostics) = parse(
            "func broken( { }\n\
             func fine(dt: f32) { let x = 1.0; }",
        );
        assert!(!diagnostics.is_empty());
        assert!(
            script.functions.iter().any(|f| f.name == "fine"),
            "recovery must reach the next top-level item; got {:?}",
            script.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn garbage_at_the_top_level_costs_one_diagnostic_not_one_per_token() {
        // Well-formed tokens in a position nothing accepts, so the run reaches
        // the parser (unknown *characters* never do - the lexer drops them).
        let (script, diagnostics) = parse("1.0 2.0 3.0 4.0 5.0\nfunc update(dt: f32) { }");
        let parse_errors = diagnostics
            .iter()
            .filter(|d| d.message.starts_with("expected `func`"))
            .count();
        assert_eq!(
            parse_errors, 1,
            "the whole run should collapse into one diagnostic; got: {diagnostics:?}"
        );
        assert_eq!(
            script.functions.len(),
            1,
            "and recovery should still reach the function after it"
        );
    }

    #[test]
    fn unknown_characters_are_dropped_by_the_lexer_before_the_parser_sees_them() {
        // The division of labour: junk characters are the lexer's to report, so
        // the parser is handed a clean stream and stays quiet about them.
        let (script, diagnostics) = parse("# # #\nfunc update(dt: f32) { }");
        assert_eq!(
            diagnostics.len(),
            3,
            "one per bad character: {diagnostics:?}"
        );
        assert!(
            diagnostics
                .iter()
                .all(|d| d.message.contains("unexpected character"))
        );
        assert_eq!(script.functions.len(), 1);
    }

    #[test]
    fn else_if_chains_stay_flat() {
        let script = parse_ok(
            "func update(dt: f32) {\n\
                 if dt > 1.0 { } else if dt > 0.5 { } else { }\n\
             }",
        );
        let Stmt::If(outer) = &script.functions[0].body.stmts[0] else {
            panic!("expected an if");
        };
        let Some(otherwise) = &outer.otherwise else {
            panic!("expected an else");
        };
        let Else::If(inner) = otherwise.as_ref() else {
            panic!("`else if` should parse as Else::If, not a nested block");
        };
        assert!(matches!(inner.otherwise.as_deref(), Some(Else::Block(_))));
    }

    #[test]
    fn while_loops_parse() {
        let script = parse_ok("func update(dt: f32) { while dt > 0.0 { dt -= 1.0; } }");
        assert!(matches!(
            script.functions[0].body.stmts[0],
            Stmt::While { .. }
        ));
    }

    #[test]
    fn for_loops_parse_their_range() {
        let script = parse_ok("func update(dt: f32) { for i in 0.0..count { } }");
        let Stmt::For {
            name, start, end, ..
        } = &script.functions[0].body.stmts[0]
        else {
            panic!("expected a for");
        };
        assert_eq!(name, "i");
        assert!(matches!(start, Expr::Number { .. }));
        assert!(matches!(end, Expr::Ident { .. }));
    }

    #[test]
    fn a_for_loop_missing_its_range_is_reported_not_panicked() {
        let (_, diagnostics) = parse("func update(dt: f32) { for i in 0.0 { } }");
        assert!(
            diagnostics.iter().any(|d| d.message.contains("`..`")),
            "the missing range operator is what to report: {diagnostics:?}"
        );
    }

    #[test]
    fn precedence_binds_multiplication_tighter_than_addition() {
        let script = parse_ok("func update(dt: f32) { let x = 1.0 + 2.0 * 3.0; }");
        let Stmt::Let { init, .. } = &script.functions[0].body.stmts[0] else {
            panic!("expected a let");
        };
        let Expr::Binary { op, rhs, .. } = init else {
            panic!("expected a binary expression");
        };
        assert_eq!(*op, BinaryOp::Add, "the outer node must be the looser `+`");
        assert!(matches!(
            **rhs,
            Expr::Binary {
                op: BinaryOp::Mul,
                ..
            }
        ));
    }

    #[test]
    fn parentheses_override_precedence() {
        let script = parse_ok("func update(dt: f32) { let x = (1.0 + 2.0) * 3.0; }");
        let Stmt::Let { init, .. } = &script.functions[0].body.stmts[0] else {
            panic!("expected a let");
        };
        assert!(matches!(
            init,
            Expr::Binary {
                op: BinaryOp::Mul,
                ..
            }
        ));
    }

    #[test]
    fn comparison_binds_looser_than_arithmetic() {
        let script = parse_ok("func update(dt: f32) { let ok = 1.0 + 2.0 > 2.0; }");
        let Stmt::Let { init, .. } = &script.functions[0].body.stmts[0] else {
            panic!("expected a let");
        };
        assert!(matches!(
            init,
            Expr::Binary {
                op: BinaryOp::Gt,
                ..
            }
        ));
    }

    #[test]
    fn a_typed_state_declaration_records_its_type() {
        let script = parse_ok("let speed: f32 = 1.0;");
        assert_eq!(
            script.state[0].ty.as_ref().expect("a declared type").name,
            "f32"
        );
    }

    #[test]
    fn field_access_chains_left_to_right() {
        let script = parse_ok("func update(dt: f32) { let x = transform.position.x; }");
        let Stmt::Let { init, .. } = &script.functions[0].body.stmts[0] else {
            panic!("expected a let");
        };
        let Expr::Field {
            receiver, field, ..
        } = init
        else {
            panic!("expected a field access");
        };
        assert_eq!(field, "x");
        assert!(matches!(**receiver, Expr::Field { ref field, .. } if field == "position"));
    }

    #[test]
    fn diagnostics_come_back_sorted_by_position() {
        let (_, diagnostics) = parse("func a( { }\nfunc b( { }");
        let starts: Vec<u32> = diagnostics.iter().map(|d| d.span.start).collect();
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        assert_eq!(starts, sorted, "an editor renders these in order");
    }

    #[test]
    fn an_empty_script_is_not_an_error() {
        let script = parse_ok("");
        assert!(script.state.is_empty() && script.functions.is_empty());
    }

    #[test]
    fn parsing_always_terminates_on_pathological_input() {
        // Guards the forced-progress rule in block(): a statement that consumes
        // no tokens must not spin forever.
        for source in ["func f() { ", "func f() { ) ) ) }", "{{{{", "let", "func"] {
            let _ = parse(source);
        }
    }

    #[test]
    fn nesting_past_the_limit_is_reported_rather_than_overflowing_the_stack() {
        // A recursive-descent parser uses stack proportional to nesting, and a
        // hundred thousand open parentheses is a file, not an impossibility.
        // Overflowing aborts the process, which in an editor loses the file.
        let depth = MAX_DEPTH as usize + 50;
        let source = format!(
            "func f() {{ let x = {}1.0{}; }}",
            "(".repeat(depth),
            ")".repeat(depth)
        );
        let (_, diagnostics) = parse(&source);
        assert_eq!(
            diagnostics.len(),
            1,
            "one message, not one per level unwound: {diagnostics:?}"
        );
        assert!(diagnostics[0].message.contains("nested more than"));
    }

    #[test]
    fn nested_blocks_count_against_the_same_budget() {
        let depth = MAX_DEPTH as usize + 50;
        let source = format!(
            "func f() {{ {}{} }}",
            "if true { ".repeat(depth),
            "}".repeat(depth)
        );
        let (_, diagnostics) = parse(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    }

    #[test]
    fn nesting_a_person_would_write_is_nowhere_near_the_limit() {
        // The limit only ever fires on generated or pasted input. Real code
        // nests single digits deep.
        let source = "func f(a: f32) -> f32 {\n\
             if a > 0.0 {\n\
                 while a > 1.0 {\n\
                     if a > 2.0 { return max(a, min(a, abs(a - 1.0))); }\n\
                 }\n\
             }\n\
             0.0\n\
         }";
        let (_, diagnostics) = parse(source);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}
