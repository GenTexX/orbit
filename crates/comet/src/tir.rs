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
    /// A 32-bit signed integer: indices, counters, anything countable. Widens to
    /// `f32` implicitly; going the other way needs `int(x)`.
    Int,
    Bool,
    /// A two-component value type, copied on assignment.
    Vec2,
    /// The one heap-allocated, reference-counted type (ADR 0007).
    Str,
    /// What a function with no declared return type returns, and the type of a
    /// block with no tail expression - which is why it is the default.
    #[default]
    Unit,
    /// A user-defined enum, by its index in [`TypedScript::enums`]. Carrying an
    /// index rather than a name keeps `Type` `Copy` and keeps every comparison
    /// a integer one.
    Enum(u32),
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
            Type::Int => "int",
            Type::Bool => "bool",
            Type::Vec2 => "Vec2",
            Type::Str => "String",
            Type::Enum(_) => "enum",
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
            "int" => Type::Int,
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
    pub enums: Vec<TypedEnum>,
    pub state: Vec<TypedState>,
    pub functions: Vec<TypedFn>,
}

impl TypedScript {
    /// The function exported under `name`, if the script defines one.
    pub fn function(&self, name: &str) -> Option<&TypedFn> {
        self.functions.iter().find(|f| f.name == name)
    }
}

/// A checked enum: its variants in declaration order, and the layout every
/// value of it occupies.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedEnum {
    pub name: String,
    pub variants: Vec<TypedVariant>,
    /// How many payload slots a value needs, beyond the tag: the widest
    /// variant's. Every value of the enum is this size whichever variant is
    /// live, which is what keeps an enum a stack value rather than an
    /// allocation.
    pub payload_slots: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedVariant {
    pub name: String,
    pub payload: Vec<Type>,
}

/// One piece of persistent script state - a top-level `let`, living in a global
/// slot that survives across calls.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedState {
    pub name: String,
    pub ty: Type,
    pub slot: u32,
    /// Always present, even when the source wrote none: the checker fills in
    /// the type's default, so codegen never learns that an initializer can be
    /// left out.
    pub init: TypedExpr,
    /// Whether `@export` marked it inspector-editable, in which case the stored
    /// value the host writes after instantiation is what actually wins.
    pub exported: bool,
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
    /// A host property, by its schema id. One variant for every property the
    /// engine exposes, rather than one variant per property - which is the
    /// whole point: adding `transform.rotation` costs nothing here.
    ///
    /// `axis` writes one component of a `Vec2` property and leaves the other as
    /// it was, which is what makes `transform.position.x = 1.0` a partial write.
    Host {
        field: u32,
        ty: Type,
        axis: Option<Axis>,
    },
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
    Number(f32),
    Int(i32),
    /// An `int` used where an `f32` is wanted. Inserted by the checker, never
    /// written - which is what makes the widening implicit without codegen
    /// having to guess where it belongs.
    Widen(Box<TypedExpr>),
    /// `int(x)`: an `f32` truncated toward zero. Saturating rather than
    /// trapping, so a script cannot crash the editor by dividing badly.
    Narrow(Box<TypedExpr>),
    Bool(bool),
    /// A string literal, interned by codegen into the module's data segment.
    Str(String),
    Local(u32),
    Global(u32),
    /// A host property, read through the host by its schema id.
    HostField {
        field: u32,
        ty: Type,
    },
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
    /// Build an enum value: its tag, then its payload.
    MakeVariant {
        enum_index: u32,
        variant: u32,
        args: Vec<TypedExpr>,
    },
    /// `match`: evaluate the scrutinee once, then whichever arm's tag matches.
    /// An expression, so every arm has the same type - which is the type of the
    /// whole thing.
    Match {
        scrutinee: Box<TypedExpr>,
        enum_index: u32,
        /// One per variant, in the enum's declaration order. Exhaustiveness is
        /// checked, so there is always exactly one arm per variant and codegen
        /// never has to consider a fallthrough.
        arms: Vec<TypedArm>,
    },
    /// Something that did not check. Always paired with [`Type::Error`].
    Error,
}

/// One arm: where its payload bindings live, and what it evaluates to.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedArm {
    /// The local slots the variant's payload is unpacked into, in order.
    pub bindings: Vec<u32>,
    pub body: TypedExpr,
}

/// The functions the host must supply for a module to run. Kept as a closed enum
/// so adding one is a deliberate edit here and in the host, not an open registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// `print(String)` - the debug output call.
    Print,
    /// `str(int) -> String`. A separate call rather than widening to `f32`
    /// first: past 2^24 that would print a rounded number, and silently at
    /// that. `str` is the one function a beginner uses to see a value, so it
    /// must not be the one that lies.
    StrInt,
    /// `str(f32) -> String` - number formatting. Done by the host because
    /// float-to-decimal is a page of code nobody should emit into every module.
    Str,
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
    /// Negate an int.
    NegInt,
    /// Invert a bool.
    Not,
    /// Negate both components of a `Vec2`.
    NegVec2,
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
    /// The same five on `int`. Division truncates toward zero, and both it and
    /// the remainder trap on a zero divisor - the one place an int is sharper
    /// than an f32, which quietly gives infinity.
    AddInt,
    SubInt,
    MulInt,
    DivInt,
    RemInt,
    /// Remainder. WebAssembly has no f32 rem, so codegen emits
    /// `a - trunc(a / b) * b`.
    RemF32,
    /// The f32 instructions that happen to be binary: min and max.
    MinF32,
    MaxF32,
    /// String concatenation. Allocates - the first thing in the language that
    /// does - and yields an owned reference like any other String expression.
    ConcatStr,
    /// The additive core of `Vec2`, plus scaling by a number. Exactly enough to
    /// make `pos += vel * dt` compile.
    ///
    /// Componentwise `Vec2 * Vec2` is deliberately absent: `a * b` on two
    /// vectors reads as a dot product to enough people that it is a hazard in a
    /// language meant to be learned from.
    AddVec2,
    SubVec2,
    /// Scaling. Both orders are kept rather than normalized to one, because
    /// swapping the operands would swap the order they are evaluated in, and an
    /// operand can be a call.
    MulVec2F32,
    MulF32Vec2,
    DivVec2F32,
    /// Equality on f32 or bool; the operand type decides the instruction.
    Eq(Type),
    NotEq(Type),
    LtF32,
    GtF32,
    LeF32,
    GeF32,
    LtInt,
    GtInt,
    LeInt,
    GeInt,
    And,
    Or,
}
