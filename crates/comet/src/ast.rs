//! comet AST: what the parser produces and the type checker walks. Every node
//! carries the [`Span`] it came from, so a diagnostic can always point at source.

use crate::span::Span;

/// A whole script: persistent state declarations and function definitions, in
/// source order.
///
/// A script has no "main" - the host calls an exported function by name (`update`
/// for now). Top-level `let`s are persistent state that outlives a call, which is
/// why they are a separate list from anything inside a function body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Script {
    pub state: Vec<StateDecl>,
    pub functions: Vec<Function>,
}

/// A top-level `let` - persistent script state, initialized once and surviving
/// across calls, as opposed to a `let` inside a body, which is an ordinary local.
#[derive(Debug, Clone, PartialEq)]
pub struct StateDecl {
    pub name: String,
    pub name_span: Span,
    /// The declared type, when written (`let speed: f32 = ...`). `None` means
    /// infer it from `init`.
    pub ty: Option<TypeName>,
    pub init: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub name_span: Span,
    pub params: Vec<Param>,
    /// The declared return type, or `None` for a function returning nothing.
    pub ret: Option<TypeName>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    /// The span of the name alone. `span` covers `name: Type`, which is the
    /// wrong thing to select when jumping to, or renaming, a parameter.
    pub name_span: Span,
    pub ty: TypeName,
    pub span: Span,
}

/// A type as written in source. Resolving it to a real type is the checker's job -
/// the parser does not know which names are valid, so an unknown one is a type
/// error with a span rather than a parse failure.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeName {
    pub name: String,
    pub span: Span,
}

/// A braced block. `tail` is the block's value: a final expression written
/// without a semicolon (Rust's rule). `None` means the block evaluates to nothing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// A local binding: `let name = expr;`
    Let {
        name: String,
        name_span: Span,
        ty: Option<TypeName>,
        init: Expr,
        span: Span,
    },
    /// An assignment to a place: `pos.x = expr;` or `speed += expr;`
    Assign {
        target: Expr,
        op: AssignOp,
        value: Expr,
        span: Span,
    },
    If(IfStmt),
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    /// `for i in a..b { ... }` - the counted loop, half-open like Rust's.
    For {
        name: String,
        name_span: Span,
        start: Expr,
        end: Expr,
        body: Block,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    /// An expression evaluated for its effect, e.g. `print("hi");`
    Expr {
        expr: Expr,
        span: Span,
    },
}

/// `if` is a statement rather than an expression in v1: nothing needs its value,
/// and keeping it a statement means a block's tail expression stays unambiguous.
#[derive(Debug, Clone, PartialEq)]
pub struct IfStmt {
    pub cond: Expr,
    pub then: Block,
    /// `else if` parses as an `Else::If`, so a chain stays a chain rather than
    /// nesting an extra block per link.
    pub otherwise: Option<Box<Else>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Else {
    Block(Block),
    If(IfStmt),
}

#[derive(Debug, Clone, PartialEq, Copy, Eq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// `5` - a whole number written without a decimal point.
    Int {
        value: i64,
        span: Span,
    },
    /// `5.0` - written with one.
    Number {
        value: f64,
        span: Span,
    },
    Bool {
        value: bool,
        span: Span,
    },
    Str {
        value: String,
        span: Span,
    },
    /// A bare name: a local, a parameter, a piece of script state, or the
    /// name of a host object like `transform`. Which one it is, is the checker's call.
    Ident {
        name: String,
        span: Span,
    },
    /// Field access: `pos.x`.
    Field {
        receiver: Box<Expr>,
        field: String,
        field_span: Span,
        span: Span,
    },
    Call {
        callee: String,
        callee_span: Span,
        args: Vec<Expr>,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    /// A parse error stood in for an expression here. Carrying it in the tree
    /// rather than bailing is what lets the checker keep going and report about
    /// the rest of the file (ADR 0010).
    Error {
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int { span, .. }
            | Expr::Number { span, .. }
            | Expr::Bool { span, .. }
            | Expr::Str { span, .. }
            | Expr::Ident { span, .. }
            | Expr::Field { span, .. }
            | Expr::Call { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Error { span } => *span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    NotEq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}
