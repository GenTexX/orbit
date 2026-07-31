//! comet typed IR: what the checker produces and codegen consumes.
//!
//! This mirrors the [`ast`](crate::ast) closely but with every question already
//! answered: each expression carries its [`Type`], and every name is resolved to
//! the slot it lives in rather than a string codegen would have to look up
//! again. Emitting a separate tree rather than annotating the AST in place means
//! codegen never re-derives a decision the checker already made, and cannot
//! silently disagree with it.
//!
//! A script that failed to check is still emitted as a tree, with the bad parts
//! typed [`Type::Error`] (ADR 0010) - the language service walks it for
//! completions even when the file does not compile. Codegen is the one consumer
//! that must refuse a tree containing errors.

use crate::span::Span;

/// The closed set of types a Comet value can have in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Type {
    F32,
    Bool,
    /// A two-component value type, copied on assignment.
    Vec2,
    /// The one heap-allocated, reference-counted type (ADR 0007).
    Str,
    /// What a function with no declared return type returns, and the type of a
    /// block with no tail expression - which is why it is the default.
    #[default]
    Unit,
    /// The poison type: something already reported as wrong. Anything built from
    /// an `Error` is `Error` and reports nothing further, so one mistake yields
    /// one diagnostic rather than a cascade.
    Error,
}

impl Type {
    /// The name to use in a diagnostic - what the user would have written.
    pub fn name(self) -> &'static str {
        match self {
            Type::F32 => "f32",
            Type::Bool => "bool",
            Type::Vec2 => "Vec2",
            Type::Str => "String",
            Type::Unit => "()",
            Type::Error => "<error>",
        }
    }

    /// Whether this type participates in checks at all. An `Error` operand
    /// suppresses further complaints about the expression built from it.
    pub fn is_error(self) -> bool {
        matches!(self, Type::Error)
    }

    /// The type a source type name denotes, or `None` if no such type exists.
    pub fn from_name(name: &str) -> Option<Type> {
        Some(match name {
            "f32" => Type::F32,
            "bool" => Type::Bool,
            "Vec2" => Type::Vec2,
            "String" => Type::Str,
            _ => return None,
        })
    }
}

/// A checked script: persistent state in declaration order, then functions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypedScript {
    pub state: Vec<TypedState>,
    pub functions: Vec<TypedFn>,
}

impl TypedScript {
    /// The function exported under `name`, if the script defines one.
    pub fn function(&self, name: &str) -> Option<&TypedFn> {
        self.functions.iter().find(|f| f.name == name)
    }
}

/// One piece of persistent script state - a top-level `let`, living in a global
/// slot that survives across calls.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedState {
    pub name: String,
    pub ty: Type,
    pub slot: u32,
    pub init: TypedExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedFn {
    pub name: String,
    pub ret: Type,
    /// Every local slot's type, indexed by slot. Parameters come first, so
    /// `locals[..param_count]` are the parameters in declaration order.
    pub locals: Vec<Type>,
    pub param_count: usize,
    pub body: TypedBlock,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TypedBlock {
    pub stmts: Vec<TypedStmt>,
    /// The block's value: a final expression written without a semicolon.
    pub tail: Option<TypedExpr>,
    /// The block's type - its tail's type, or `Unit`.
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedStmt {
    /// Bind a local slot to a value. The slot's type is in [`TypedFn::locals`].
    Let {
        slot: u32,
        init: TypedExpr,
    },
    /// Store a value into a place. Compound assignment is already desugared:
    /// `x += 1` arrives here as a plain store of `x + 1`.
    Assign {
        place: Place,
        value: TypedExpr,
    },
    If {
        cond: TypedExpr,
        then: TypedBlock,
        otherwise: Option<TypedBlock>,
    },
    While {
        cond: TypedExpr,
        body: TypedBlock,
    },
    Return {
        value: Option<TypedExpr>,
    },
    /// An expression evaluated for its effect; its value, if any, is discarded.
    Expr(TypedExpr),
}

/// Somewhere a value can be stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    Local(u32),
    /// One axis of a local `Vec2`. The other component is left as it was, which
    /// is what makes `v.x = 1.0` a partial write rather than a whole one.
    LocalField(u32, Axis),
    Global(u32),
    GlobalField(u32, Axis),
    /// The owning node's position, written back through the host.
    Pos,
    /// One axis of the owning node's position.
    PosField(Axis),
    /// An assignment whose target did not check. Codegen never sees this.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedExpr {
    pub kind: TypedExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedExprKind {
    /// All numbers are f32 - there is no integer type to coerce from.
    Number(f32),
    Bool(bool),
    /// A string literal, interned by codegen into the module's data segment.
    Str(String),
    Local(u32),
    Global(u32),
    /// The owning node's position, read through the host.
    Pos,
    /// One component of a `Vec2` value.
    Field {
        receiver: Box<TypedExpr>,
        axis: Axis,
    },
    /// `vec2(x, y)` - the two components, in order, on the stack.
    MakeVec2 {
        x: Box<TypedExpr>,
        y: Box<TypedExpr>,
    },
    /// A call to a function defined in this script. The index is into
    /// [`TypedScript::functions`].
    Call {
        index: usize,
        args: Vec<TypedExpr>,
    },
    /// A call to a host-provided function, e.g. `print`.
    HostCall {
        host: Host,
        args: Vec<TypedExpr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<TypedExpr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<TypedExpr>,
        rhs: Box<TypedExpr>,
    },
    /// Something that did not check. Always paired with [`Type::Error`].
    Error,
}

/// The functions the host must supply for a module to run. Kept as a closed enum
/// so adding one is a deliberate edit here and in the host, not an open registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// `print(String)` - the debug output call.
    Print,
    /// The transcendentals, which WebAssembly has no instructions for. Everything
    /// else - abs, sqrt, floor, ceil, min, max - is one opcode and never leaves
    /// the module.
    Sin,
    Cos,
    Atan2,
    Pow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Negate an f32.
    Neg,
    /// Invert a bool.
    Not,
    /// The f32 instructions that happen to be unary.
    Abs,
    Sqrt,
    Floor,
    Ceil,
}

/// A binary operation with its operand types already settled - `AddF32` rather
/// than a generic `Add` codegen would have to re-type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    AddF32,
    SubF32,
    MulF32,
    DivF32,
    /// Remainder. WebAssembly has no f32 rem, so codegen emits
    /// `a - trunc(a / b) * b`.
    RemF32,
    /// The f32 instructions that happen to be binary: min and max.
    MinF32,
    MaxF32,
    /// Equality on f32 or bool; the operand type decides the instruction.
    Eq(Type),
    NotEq(Type),
    LtF32,
    GtF32,
    LeF32,
    GeF32,
    And,
    Or,
}
