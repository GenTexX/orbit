//! comet type checker: the AST to a typed IR, plus every type error found on the
//! way.
//!
//! Like the parser, this never fails outright (ADR 0010). An expression it
//! cannot type becomes [`Type::Error`], which then absorbs anything built from
//! it - so a single mistake produces a single diagnostic instead of a cascade of
//! consequences, and the rest of the file still checks.
//!
//! Types are inferred only locally: a `let` takes its type from its initializer,
//! but a function must declare its parameters and return type (ADR 0006). That
//! is the whole inference story - there is no unification and no generics.

use std::collections::HashMap;

use crate::ast;
use crate::diagnostic::Diagnostic;
use crate::span::Span;
use crate::tir::*;

/// The magic identifier bound to the owning node's position.
const POS: &str = "pos";

/// Check `script`, returning the typed IR and any diagnostics. A script with
/// diagnostics still yields a tree - the bad parts are [`Type::Error`].
pub fn check(script: &ast::Script) -> (TypedScript, Vec<Diagnostic>) {
    let mut checker = Checker {
        diagnostics: Vec::new(),
        globals: HashMap::new(),
        global_types: Vec::new(),
        signatures: Vec::new(),
        by_name: HashMap::new(),
        locals: Vec::new(),
        scopes: Vec::new(),
        ret: Type::Unit,
    };
    let typed = checker.script(script);
    checker
        .diagnostics
        .sort_by_key(|d| (d.span.start, d.span.end));
    (typed, checker.diagnostics)
}

struct Signature {
    params: Vec<Type>,
    ret: Type,
}

struct Checker {
    diagnostics: Vec<Diagnostic>,
    /// Script state by name -> global slot.
    globals: HashMap<String, u32>,
    global_types: Vec<Type>,
    /// Every function's signature, indexed the same as `Script::functions`. Kept
    /// per index rather than per name so that two functions sharing a name (an
    /// error, but one the checker still has to survive) cannot end up checking
    /// one body against the other's parameters.
    signatures: Vec<Signature>,
    /// Function name -> index, for resolving calls. First definition wins; a
    /// later one is reported and ignored.
    by_name: HashMap<String, usize>,
    /// The current function's slot types; parameters occupy the first slots.
    locals: Vec<Type>,
    /// Lexical scopes, innermost last. Shadowing an outer name is allowed.
    scopes: Vec<HashMap<String, u32>>,
    ret: Type,
}

impl Checker {
    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(span, message));
    }

    /// Report a type mismatch, unless either side is already poisoned.
    fn expect_type(&mut self, expected: Type, found: Type, span: Span) {
        if expected == found || expected.is_error() || found.is_error() {
            return;
        }
        self.error(
            span,
            format!("expected `{}`, found `{}`", expected.name(), found.name()),
        );
    }

    fn resolve_type(&mut self, name: &ast::TypeName) -> Type {
        match Type::from_name(&name.name) {
            Some(ty) => ty,
            None => {
                // An empty name means the parser already reported a missing
                // type name; do not pile on.
                if !name.name.is_empty() {
                    self.error(name.span, format!("unknown type `{}`", name.name));
                }
                Type::Error
            }
        }
    }

    // --- top level ---

    fn script(&mut self, script: &ast::Script) -> TypedScript {
        // Signatures first, so a function can call one defined later and a
        // global initializer can call any of them.
        for (index, f) in script.functions.iter().enumerate() {
            let params: Vec<Type> = f
                .params
                .iter()
                .map(|p| {
                    let ty = self.resolve_type(&p.ty);
                    if ty == Type::Unit {
                        self.error(p.ty.span, "a parameter cannot have type `()`");
                    }
                    ty
                })
                .collect();
            let ret = match &f.ret {
                Some(t) => self.resolve_type(t),
                None => Type::Unit,
            };
            self.signatures.push(Signature { params, ret });
            if self.by_name.contains_key(&f.name) {
                if !f.name.is_empty() {
                    self.error(
                        f.name_span,
                        format!("`{}` is already defined in this script", f.name),
                    );
                }
            } else {
                self.by_name.insert(f.name.clone(), index);
            }
            if is_host_name(&f.name) {
                self.error(
                    f.name_span,
                    format!(
                        "`{}` is provided by the engine and cannot be redefined",
                        f.name
                    ),
                );
            } else if is_reserved_name(&f.name) {
                self.error(
                    f.name_span,
                    format!("`{}` is reserved for the runtime", f.name),
                );
            }
        }

        // Then script state, in order: an initializer sees the globals declared
        // above it, which keeps the whole thing acyclic without a dependency
        // graph.
        let mut state = Vec::new();
        for decl in &script.state {
            let init = self.expr(&decl.init);
            let declared = decl.ty.as_ref().map(|t| self.resolve_type(t));
            let ty = match declared {
                Some(declared) => {
                    self.expect_type(declared, init.ty, init.span);
                    declared
                }
                None => init.ty,
            };
            if ty == Type::Unit {
                self.error(
                    decl.span,
                    "a `let` needs a value, but this produces nothing",
                );
            }
            let slot = self.global_types.len() as u32;
            self.global_types.push(ty);
            if self.globals.insert(decl.name.clone(), slot).is_some() && !decl.name.is_empty() {
                self.error(
                    decl.name_span,
                    format!("`{}` is already defined in this script", decl.name),
                );
            }
            state.push(TypedState {
                name: decl.name.clone(),
                ty,
                slot,
                init,
            });
        }

        let functions = script
            .functions
            .iter()
            .enumerate()
            .map(|(index, f)| self.function(index, f))
            .collect();
        TypedScript { state, functions }
    }

    fn function(&mut self, index: usize, f: &ast::Function) -> TypedFn {
        let signature = &self.signatures[index];
        let ret = signature.ret;
        let param_types = signature.params.clone();

        self.locals = param_types;
        self.ret = ret;
        self.scopes = vec![HashMap::new()];
        for (i, p) in f.params.iter().enumerate() {
            self.scopes[0].insert(p.name.clone(), i as u32);
        }

        let body = self.block(&f.body);

        // The body's tail is the function's value when it has one.
        if let Some(tail) = &body.tail {
            self.expect_type(ret, tail.ty, tail.ty_span(f.body.span));
        } else if ret != Type::Unit && !ret.is_error() && !always_returns(&body) {
            self.error(
                f.body.span,
                format!(
                    "this function must return `{}`, but some paths reach the end without one",
                    ret.name()
                ),
            );
        }

        TypedFn {
            name: f.name.clone(),
            ret,
            locals: std::mem::take(&mut self.locals),
            param_count: f.params.len(),
            body,
        }
    }

    // --- statements ---

    fn block(&mut self, block: &ast::Block) -> TypedBlock {
        self.scopes.push(HashMap::new());
        let stmts = block.stmts.iter().map(|s| self.stmt(s)).collect();
        let tail = block.tail.as_ref().map(|e| self.expr(e));
        self.scopes.pop();
        let ty = tail.as_ref().map_or(Type::Unit, |t| t.ty);
        TypedBlock { stmts, tail, ty }
    }

    fn declare_local(&mut self, name: &str, ty: Type) -> u32 {
        let slot = self.locals.len() as u32;
        self.locals.push(ty);
        self.scopes
            .last_mut()
            .expect("a scope is always open while checking a body")
            .insert(name.to_string(), slot);
        slot
    }

    fn stmt(&mut self, stmt: &ast::Stmt) -> TypedStmt {
        match stmt {
            ast::Stmt::Let {
                name,
                ty,
                init,
                span,
                ..
            } => {
                let init = self.expr(init);
                let declared = ty.as_ref().map(|t| self.resolve_type(t));
                let ty = match declared {
                    Some(declared) => {
                        self.expect_type(declared, init.ty, init.span);
                        declared
                    }
                    None => init.ty,
                };
                if ty == Type::Unit {
                    self.error(*span, "a `let` needs a value, but this produces nothing");
                }
                let slot = self.declare_local(name, ty);
                TypedStmt::Let { slot, init }
            }

            ast::Stmt::Assign {
                target, op, value, ..
            } => {
                let (place, target_ty) = self.place(target);
                let value = self.expr(value);
                match op {
                    ast::AssignOp::Set => {
                        self.expect_type(target_ty, value.ty, value.span);
                        TypedStmt::Assign { place, value }
                    }
                    _ => {
                        // Compound assignment is arithmetic, so both sides must
                        // be f32; desugar to a plain store of the operation.
                        let op = match op {
                            ast::AssignOp::Add => BinaryOp::AddF32,
                            ast::AssignOp::Sub => BinaryOp::SubF32,
                            ast::AssignOp::Mul => BinaryOp::MulF32,
                            ast::AssignOp::Div => BinaryOp::DivF32,
                            ast::AssignOp::Rem => BinaryOp::RemF32,
                            ast::AssignOp::Set => unreachable!("handled above"),
                        };
                        self.expect_type(Type::F32, target_ty, target.span());
                        self.expect_type(Type::F32, value.ty, value.span);
                        let ok = target_ty == Type::F32 && value.ty == Type::F32;
                        let span = value.span;
                        let read = self.place_read(&place, target_ty, target.span());
                        TypedStmt::Assign {
                            place,
                            value: TypedExpr {
                                ty: if ok { Type::F32 } else { Type::Error },
                                kind: if ok {
                                    TypedExprKind::Binary {
                                        op,
                                        lhs: Box::new(read),
                                        rhs: Box::new(value),
                                    }
                                } else {
                                    TypedExprKind::Error
                                },
                                span,
                            },
                        }
                    }
                }
            }

            ast::Stmt::If(if_stmt) => self.if_stmt(if_stmt),

            ast::Stmt::While { cond, body, .. } => {
                let cond = self.expr(cond);
                self.expect_type(Type::Bool, cond.ty, cond.span);
                TypedStmt::While {
                    cond,
                    body: self.block(body),
                }
            }

            ast::Stmt::Return { value, span } => {
                let value = value.as_ref().map(|v| self.expr(v));
                match (&value, self.ret) {
                    (Some(v), ret) => self.expect_type(ret, v.ty, v.span),
                    (None, Type::Unit) | (None, Type::Error) => {}
                    (None, ret) => self.error(
                        *span,
                        format!(
                            "this function returns `{}`, so `return` needs a value",
                            ret.name()
                        ),
                    ),
                }
                TypedStmt::Return { value }
            }

            ast::Stmt::Expr { expr, .. } => TypedStmt::Expr(self.expr(expr)),
        }
    }

    fn if_stmt(&mut self, if_stmt: &ast::IfStmt) -> TypedStmt {
        let cond = self.expr(&if_stmt.cond);
        self.expect_type(Type::Bool, cond.ty, cond.span);
        let then = self.block(&if_stmt.then);
        let otherwise = if_stmt.otherwise.as_deref().map(|e| match e {
            ast::Else::Block(b) => self.block(b),
            ast::Else::If(nested) => {
                // `else if` becomes a block holding the nested if, so the IR has
                // one shape for both spellings.
                let stmt = self.if_stmt(nested);
                TypedBlock {
                    stmts: vec![stmt],
                    tail: None,
                    ty: Type::Unit,
                }
            }
        });
        TypedStmt::If {
            cond,
            then,
            otherwise,
        }
    }

    // --- places ---

    /// Resolve an assignment target to a [`Place`] and the type stored there.
    fn place(&mut self, target: &ast::Expr) -> (Place, Type) {
        match target {
            ast::Expr::Ident { name, span } => {
                if name == POS {
                    return (Place::Pos, Type::Vec2);
                }
                if let Some(slot) = self.lookup_local(name) {
                    return (Place::Local(slot), self.locals[slot as usize]);
                }
                if let Some(&slot) = self.globals.get(name) {
                    return (Place::Global(slot), self.global_types[slot as usize]);
                }
                if !name.is_empty() {
                    self.error(*span, format!("cannot find `{name}` in this scope"));
                }
                (Place::Error, Type::Error)
            }
            ast::Expr::Field {
                receiver,
                field,
                field_span,
                ..
            } => {
                if let ast::Expr::Ident { name, .. } = receiver.as_ref()
                    && name == POS
                {
                    return match axis(field) {
                        Some(axis) => (Place::PosField(axis), Type::F32),
                        None => {
                            self.error(*field_span, format!("`Vec2` has no field `{field}`"));
                            (Place::Error, Type::Error)
                        }
                    };
                }
                // A named `Vec2` - a local, a parameter, or script state - can
                // have one axis written. Anything else cannot: there is nowhere
                // to put the result of assigning into a temporary.
                if let ast::Expr::Ident { name, span } = receiver.as_ref() {
                    let axis = match axis(field) {
                        Some(axis) => axis,
                        None => {
                            self.error(*field_span, format!("`Vec2` has no field `{field}`"));
                            return (Place::Error, Type::Error);
                        }
                    };
                    if let Some(slot) = self.lookup_local(name) {
                        return match self.locals[slot as usize] {
                            Type::Vec2 => (Place::LocalField(slot, axis), Type::F32),
                            other => {
                                self.no_fields(*span, other);
                                (Place::Error, Type::Error)
                            }
                        };
                    }
                    if let Some(&slot) = self.globals.get(name) {
                        return match self.global_types[slot as usize] {
                            Type::Vec2 => (Place::GlobalField(slot, axis), Type::F32),
                            other => {
                                self.no_fields(*span, other);
                                (Place::Error, Type::Error)
                            }
                        };
                    }
                    if !name.is_empty() {
                        self.error(*span, format!("cannot find `{name}` in this scope"));
                    }
                    return (Place::Error, Type::Error);
                }
                let receiver = self.expr(receiver);
                if !receiver.ty.is_error() {
                    self.error(
                        target.span(),
                        "only a named `Vec2` can be assigned through a field",
                    );
                }
                (Place::Error, Type::Error)
            }
            other => {
                if !matches!(other, ast::Expr::Error { .. }) {
                    self.error(other.span(), "this cannot be assigned to");
                }
                (Place::Error, Type::Error)
            }
        }
    }

    /// Report that a type has no fields, unless it is already poisoned.
    fn no_fields(&mut self, span: Span, ty: Type) {
        if !ty.is_error() {
            self.error(span, format!("`{}` has no fields", ty.name()));
        }
    }

    /// The expression that reads a place - what compound assignment needs for
    /// its left operand.
    fn place_read(&mut self, place: &Place, ty: Type, span: Span) -> TypedExpr {
        let kind = match place {
            Place::Local(slot) => TypedExprKind::Local(*slot),
            Place::Global(slot) => TypedExprKind::Global(*slot),
            Place::LocalField(slot, axis) => TypedExprKind::Field {
                receiver: Box::new(TypedExpr {
                    kind: TypedExprKind::Local(*slot),
                    ty: Type::Vec2,
                    span,
                }),
                axis: *axis,
            },
            Place::GlobalField(slot, axis) => TypedExprKind::Field {
                receiver: Box::new(TypedExpr {
                    kind: TypedExprKind::Global(*slot),
                    ty: Type::Vec2,
                    span,
                }),
                axis: *axis,
            },
            Place::Pos => TypedExprKind::Pos,
            Place::PosField(axis) => TypedExprKind::Field {
                receiver: Box::new(TypedExpr {
                    kind: TypedExprKind::Pos,
                    ty: Type::Vec2,
                    span,
                }),
                axis: *axis,
            },
            Place::Error => TypedExprKind::Error,
        };
        TypedExpr { kind, ty, span }
    }

    fn lookup_local(&self, name: &str) -> Option<u32> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    // --- expressions ---

    fn expr(&mut self, expr: &ast::Expr) -> TypedExpr {
        let span = expr.span();
        let (kind, ty) = match expr {
            ast::Expr::Number { value, .. } => (TypedExprKind::Number(*value as f32), Type::F32),
            ast::Expr::Bool { value, .. } => (TypedExprKind::Bool(*value), Type::Bool),
            ast::Expr::Str { value, .. } => (TypedExprKind::Str(value.clone()), Type::Str),

            ast::Expr::Ident { name, .. } => {
                if name == POS {
                    (TypedExprKind::Pos, Type::Vec2)
                } else if let Some(slot) = self.lookup_local(name) {
                    (TypedExprKind::Local(slot), self.locals[slot as usize])
                } else if let Some(&slot) = self.globals.get(name) {
                    (
                        TypedExprKind::Global(slot),
                        self.global_types[slot as usize],
                    )
                } else {
                    if !name.is_empty() {
                        self.error(span, format!("cannot find `{name}` in this scope"));
                    }
                    (TypedExprKind::Error, Type::Error)
                }
            }

            ast::Expr::Field {
                receiver,
                field,
                field_span,
                ..
            } => {
                let receiver = self.expr(receiver);
                match receiver.ty {
                    Type::Vec2 => match axis(field) {
                        Some(axis) => (
                            TypedExprKind::Field {
                                receiver: Box::new(receiver),
                                axis,
                            },
                            Type::F32,
                        ),
                        None => {
                            self.error(*field_span, format!("`Vec2` has no field `{field}`"));
                            (TypedExprKind::Error, Type::Error)
                        }
                    },
                    Type::Error => (TypedExprKind::Error, Type::Error),
                    other => {
                        self.error(*field_span, format!("`{}` has no fields", other.name()));
                        (TypedExprKind::Error, Type::Error)
                    }
                }
            }

            ast::Expr::Call {
                callee,
                callee_span,
                args,
                ..
            } => self.call(callee, *callee_span, args, span),

            ast::Expr::Unary { op, operand, .. } => {
                let operand = self.expr(operand);
                let (op, want) = match op {
                    ast::UnaryOp::Neg => (UnaryOp::Neg, Type::F32),
                    ast::UnaryOp::Not => (UnaryOp::Not, Type::Bool),
                };
                self.expect_type(want, operand.ty, operand.span);
                let ok = operand.ty == want;
                (
                    if ok {
                        TypedExprKind::Unary {
                            op,
                            operand: Box::new(operand),
                        }
                    } else {
                        TypedExprKind::Error
                    },
                    if ok { want } else { Type::Error },
                )
            }

            ast::Expr::Binary { op, lhs, rhs, .. } => {
                let lhs = self.expr(lhs);
                let rhs = self.expr(rhs);
                self.binary(*op, lhs, rhs)
            }

            ast::Expr::Error { .. } => (TypedExprKind::Error, Type::Error),
        };
        TypedExpr { kind, ty, span }
    }

    fn call(
        &mut self,
        callee: &str,
        callee_span: Span,
        args: &[ast::Expr],
        span: Span,
    ) -> (TypedExprKind, Type) {
        let args: Vec<TypedExpr> = args.iter().map(|a| self.expr(a)).collect();

        // `vec2(x, y)` is a constructor rather than a call: it becomes two
        // values on the stack, which is what a Vec2 already is.
        if callee == "vec2" {
            self.check_args(callee, &args, &[Type::F32, Type::F32], span);
            let ok = args.len() == 2 && args.iter().all(|a| a.ty == Type::F32);
            if !ok {
                return (TypedExprKind::Error, Type::Error);
            }
            let mut args = args.into_iter();
            return (
                TypedExprKind::MakeVec2 {
                    x: Box::new(args.next().expect("arity checked")),
                    y: Box::new(args.next().expect("arity checked")),
                },
                Type::Vec2,
            );
        }
        if let Some((builtin, params, ret)) = builtin(callee) {
            self.check_args(callee, &args, params, span);
            let ok = args.len() == params.len() && args.iter().zip(params).all(|(a, p)| a.ty == *p);
            if !ok {
                return (TypedExprKind::Error, Type::Error);
            }
            let mut args = args.into_iter();
            return match builtin {
                Builtin::Host(host) => (
                    TypedExprKind::HostCall {
                        host,
                        args: args.collect(),
                    },
                    ret,
                ),
                // One WebAssembly instruction, so it lowers inline rather than
                // costing a call out of the module.
                Builtin::Unary(op) => (
                    TypedExprKind::Unary {
                        op,
                        operand: Box::new(args.next().expect("arity checked")),
                    },
                    ret,
                ),
                Builtin::Binary(op) => {
                    let lhs = args.next().expect("arity checked");
                    let rhs = args.next().expect("arity checked");
                    (
                        TypedExprKind::Binary {
                            op,
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        },
                        ret,
                    )
                }
            };
        }

        let Some(&index) = self.by_name.get(callee) else {
            if !callee.is_empty() {
                self.error(callee_span, format!("cannot find function `{callee}`"));
            }
            return (TypedExprKind::Error, Type::Error);
        };
        let signature = &self.signatures[index];
        let (params, ret) = (signature.params.clone(), signature.ret);
        self.check_args(callee, &args, &params, span);
        let ok = args.len() == params.len() && args.iter().zip(&params).all(|(a, p)| a.ty == *p);
        if ok {
            (TypedExprKind::Call { index, args }, ret)
        } else {
            (TypedExprKind::Error, Type::Error)
        }
    }

    fn check_args(&mut self, callee: &str, args: &[TypedExpr], params: &[Type], span: Span) {
        if args.len() != params.len() {
            self.error(
                span,
                format!(
                    "`{callee}` takes {} argument{}, but {} {} given",
                    params.len(),
                    if params.len() == 1 { "" } else { "s" },
                    args.len(),
                    if args.len() == 1 { "was" } else { "were" }
                ),
            );
            return;
        }
        for (arg, param) in args.iter().zip(params) {
            self.expect_type(*param, arg.ty, arg.span);
        }
    }

    fn binary(
        &mut self,
        op: ast::BinaryOp,
        lhs: TypedExpr,
        rhs: TypedExpr,
    ) -> (TypedExprKind, Type) {
        use ast::BinaryOp as B;

        // An operand that already failed poisons the result silently.
        if lhs.ty.is_error() || rhs.ty.is_error() {
            return (TypedExprKind::Error, Type::Error);
        }

        let (typed_op, result) = match op {
            B::Add | B::Sub | B::Mul | B::Div | B::Rem => {
                self.expect_type(Type::F32, lhs.ty, lhs.span);
                self.expect_type(Type::F32, rhs.ty, rhs.span);
                if lhs.ty != Type::F32 || rhs.ty != Type::F32 {
                    return (TypedExprKind::Error, Type::Error);
                }
                let op = match op {
                    B::Add => BinaryOp::AddF32,
                    B::Sub => BinaryOp::SubF32,
                    B::Mul => BinaryOp::MulF32,
                    B::Div => BinaryOp::DivF32,
                    B::Rem => BinaryOp::RemF32,
                    _ => unreachable!("matched on the arithmetic operators"),
                };
                (op, Type::F32)
            }

            B::Lt | B::Gt | B::Le | B::Ge => {
                self.expect_type(Type::F32, lhs.ty, lhs.span);
                self.expect_type(Type::F32, rhs.ty, rhs.span);
                if lhs.ty != Type::F32 || rhs.ty != Type::F32 {
                    return (TypedExprKind::Error, Type::Error);
                }
                let op = match op {
                    B::Lt => BinaryOp::LtF32,
                    B::Gt => BinaryOp::GtF32,
                    B::Le => BinaryOp::LeF32,
                    B::Ge => BinaryOp::GeF32,
                    _ => unreachable!("matched on the comparison operators"),
                };
                (op, Type::Bool)
            }

            B::Eq | B::NotEq => {
                if lhs.ty != rhs.ty {
                    self.expect_type(lhs.ty, rhs.ty, rhs.span);
                    return (TypedExprKind::Error, Type::Error);
                }
                // Comparing strings needs a host call or a memcmp helper, which
                // v1 does not emit; keep it an error rather than a silent
                // pointer comparison that would look like it worked.
                if !matches!(lhs.ty, Type::F32 | Type::Bool) {
                    self.error(
                        lhs.span.to(rhs.span),
                        format!(
                            "`{}` values cannot be compared in this version",
                            lhs.ty.name()
                        ),
                    );
                    return (TypedExprKind::Error, Type::Error);
                }
                let op = if op == B::Eq {
                    BinaryOp::Eq(lhs.ty)
                } else {
                    BinaryOp::NotEq(lhs.ty)
                };
                (op, Type::Bool)
            }

            B::And | B::Or => {
                self.expect_type(Type::Bool, lhs.ty, lhs.span);
                self.expect_type(Type::Bool, rhs.ty, rhs.span);
                if lhs.ty != Type::Bool || rhs.ty != Type::Bool {
                    return (TypedExprKind::Error, Type::Error);
                }
                let op = if op == B::And {
                    BinaryOp::And
                } else {
                    BinaryOp::Or
                };
                (op, Type::Bool)
            }
        };

        (
            TypedExprKind::Binary {
                op: typed_op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            result,
        )
    }
}

impl TypedExpr {
    /// The span to blame for a tail expression's type. Falls back to the block's
    /// span when the tail came from recovered source with no useful span.
    fn ty_span(&self, fallback: Span) -> Span {
        if self.span.start == self.span.end {
            fallback
        } else {
            self.span
        }
    }
}

fn axis(field: &str) -> Option<Axis> {
    match field {
        "x" => Some(Axis::X),
        "y" => Some(Axis::Y),
        _ => None,
    }
}

/// What a builtin call lowers to once its arguments have checked.
#[derive(Debug, Clone, Copy)]
enum Builtin {
    Host(Host),
    Unary(UnaryOp),
    Binary(BinaryOp),
}

/// The functions the engine provides, with their signatures.
///
/// Most of the maths is one WebAssembly instruction, so it lowers inline and
/// never leaves the module; only the transcendentals, which wasm has no opcodes
/// for, cost a host call.
fn builtin(name: &str) -> Option<(Builtin, &'static [Type], Type)> {
    use Type::{F32, Str, Unit};
    Some(match name {
        "print" => (Builtin::Host(Host::Print), &[Str], Unit),
        "abs" => (Builtin::Unary(UnaryOp::Abs), &[F32], F32),
        "sqrt" => (Builtin::Unary(UnaryOp::Sqrt), &[F32], F32),
        "floor" => (Builtin::Unary(UnaryOp::Floor), &[F32], F32),
        "ceil" => (Builtin::Unary(UnaryOp::Ceil), &[F32], F32),
        "min" => (Builtin::Binary(BinaryOp::MinF32), &[F32, F32], F32),
        "max" => (Builtin::Binary(BinaryOp::MaxF32), &[F32, F32], F32),
        "sin" => (Builtin::Host(Host::Sin), &[F32], F32),
        "cos" => (Builtin::Host(Host::Cos), &[F32], F32),
        "atan2" => (Builtin::Host(Host::Atan2), &[F32, F32], F32),
        "pow" => (Builtin::Host(Host::Pow), &[F32, F32], F32),
        _ => return None,
    })
}

fn is_host_name(name: &str) -> bool {
    builtin(name).is_some() || name == "vec2"
}

/// Names the compiled module already uses for something. Every script function
/// is exported under its own name, and wasm export names have to be unique, so
/// a script calling a function `memory` would emit an invalid module. Reserving
/// the `comet_` prefix and `memory` up front turns that into a diagnostic with a
/// span, which is the only form of it a person can act on.
fn is_reserved_name(name: &str) -> bool {
    name.starts_with("comet_") || name == "memory"
}

/// Whether every path through this block ends in a `return`. Used only to report
/// a missing return, so it is deliberately conservative: it understands `return`
/// and `if`/`else` where both sides return, and assumes nothing about loops.
fn always_returns(block: &TypedBlock) -> bool {
    if block.tail.is_some() {
        return true;
    }
    block.stmts.iter().any(|stmt| match stmt {
        TypedStmt::Return { .. } => true,
        TypedStmt::If {
            then,
            otherwise: Some(otherwise),
            ..
        } => always_returns(then) && always_returns(otherwise),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    /// Check a source string, asserting it parsed cleanly first so a test about
    /// types never accidentally passes because of a syntax error.
    fn check_src(source: &str) -> (TypedScript, Vec<Diagnostic>) {
        let (script, parse_diagnostics) = parse(source);
        assert!(
            parse_diagnostics.is_empty(),
            "fixture should parse clean: {parse_diagnostics:?}"
        );
        check(&script)
    }

    fn messages(source: &str) -> Vec<String> {
        check_src(source).1.into_iter().map(|d| d.message).collect()
    }

    fn check_clean(source: &str) -> TypedScript {
        let (typed, diagnostics) = check_src(source);
        assert!(
            diagnostics.is_empty(),
            "expected no errors, got {diagnostics:?}"
        );
        typed
    }

    // --- the plan's valid fixtures must check clean ---

    #[test]
    fn fixture_1_bouncing_node_checks_clean() {
        let typed = check_clean(include_str!("../tests/fixtures/bounce.cmt"));
        assert_eq!(typed.state.len(), 2);
        assert_eq!(typed.state[0].ty, Type::F32, "speed is inferred f32");
        let update = typed.function("update").expect("update is defined");
        assert_eq!(update.ret, Type::Unit);
        assert_eq!(update.param_count, 1);
        assert_eq!(update.locals[0], Type::F32, "dt");
    }

    #[test]
    fn fixture_2_string_and_print_checks_clean() {
        let typed = check_clean(include_str!("../tests/fixtures/ticker.cmt"));
        let update = typed.function("update").expect("update is defined");
        let TypedStmt::If { then, .. } = &update.body.stmts[1] else {
            panic!("expected an if");
        };
        let TypedStmt::Expr(call) = &then.stmts[0] else {
            panic!("expected the print call");
        };
        assert!(matches!(
            call.kind,
            TypedExprKind::HostCall {
                host: Host::Print,
                ..
            }
        ));
        assert_eq!(call.ty, Type::Unit);
    }

    #[test]
    fn fixture_3_clamp_checks_clean_and_returns_via_its_tail() {
        let typed = check_clean(include_str!("../tests/fixtures/clamp.cmt"));
        let clamp = typed.function("clamp").expect("clamp is defined");
        assert_eq!(clamp.ret, Type::F32);
        assert_eq!(clamp.param_count, 3);
        let tail = clamp.body.tail.as_ref().expect("a tail expression");
        assert_eq!(tail.ty, Type::F32);
        assert!(
            matches!(tail.kind, TypedExprKind::Local(0)),
            "`value` is param 0"
        );
    }

    // --- the plan's type-error fixture: exactly one error, at the right span ---

    #[test]
    fn fixture_4_reports_exactly_one_error_at_the_bool_operand() {
        let source = include_str!("../tests/fixtures/type_error.cmt");
        let (_, diagnostics) = check_src(source);
        assert_eq!(
            diagnostics.len(),
            1,
            "one mistake, one diagnostic: {diagnostics:?}"
        );
        assert_eq!(diagnostics[0].message, "expected `f32`, found `bool`");
        assert_eq!(
            diagnostics[0].span.text(source),
            "ready",
            "the squiggle belongs under the offending operand"
        );
    }

    // --- error tolerance: one mistake does not cascade ---

    #[test]
    fn an_unknown_name_produces_one_error_not_one_per_use() {
        let messages = messages(
            "func update(dt: f32) {\n\
                 let a = nope * 2.0;\n\
                 let b = a + 1.0;\n\
                 let c = b + a;\n\
             }",
        );
        assert_eq!(
            messages,
            vec!["cannot find `nope` in this scope"],
            "the poisoned value must not re-report at every later use"
        );
    }

    #[test]
    fn a_bad_statement_does_not_stop_later_ones_being_checked() {
        let messages = messages(
            "func update(dt: f32) {\n\
                 let a = true + 1.0;\n\
                 let b = false * 2.0;\n\
             }",
        );
        assert_eq!(
            messages.len(),
            2,
            "both lines are still reported: {messages:?}"
        );
    }

    // --- inference and scoping ---

    #[test]
    fn a_let_takes_its_type_from_its_initializer() {
        let typed = check_clean("func update(dt: f32) { let flag = true; let n = 1.0; }");
        let update = typed.function("update").unwrap();
        assert_eq!(update.locals[1], Type::Bool);
        assert_eq!(update.locals[2], Type::F32);
    }

    #[test]
    fn a_declared_type_must_match_the_initializer() {
        assert_eq!(
            messages("func update(dt: f32) { let n: f32 = true; }"),
            vec!["expected `f32`, found `bool`"]
        );
    }

    #[test]
    fn an_unknown_type_name_is_reported() {
        assert_eq!(
            messages("func update(dt: Vector) { }"),
            vec!["unknown type `Vector`"]
        );
    }

    #[test]
    fn an_inner_scope_can_shadow_an_outer_name() {
        let typed = check_clean(
            "func update(dt: f32) {\n\
                 let x = 1.0;\n\
                 if dt > 0.0 { let x = true; }\n\
             }",
        );
        let update = typed.function("update").unwrap();
        assert_eq!(update.locals[1], Type::F32, "the outer x");
        assert_eq!(update.locals[2], Type::Bool, "the shadowing inner x");
    }

    #[test]
    fn a_local_does_not_escape_the_block_that_declared_it() {
        assert_eq!(
            messages(
                "func update(dt: f32) {\n\
                     if dt > 0.0 { let inner = 1.0; }\n\
                     let after = inner;\n\
                 }"
            ),
            vec!["cannot find `inner` in this scope"]
        );
    }

    #[test]
    fn script_state_is_visible_to_a_function_defined_before_it() {
        // clamp.cmt relies on this: `let speed` sits between the two functions.
        check_clean("func update(dt: f32) { pos.x += speed; }\nlet speed = 1.0;");
    }

    #[test]
    fn a_global_initializer_cannot_see_a_global_declared_after_it() {
        assert_eq!(
            messages("let a = b;\nlet b = 1.0;"),
            vec!["cannot find `b` in this scope"]
        );
    }

    // --- the magic `pos` binding ---

    #[test]
    fn pos_is_a_vec2_and_its_axes_are_f32() {
        let typed = check_clean("func update(dt: f32) { let p = pos; let x = pos.x; }");
        let update = typed.function("update").unwrap();
        assert_eq!(update.locals[1], Type::Vec2);
        assert_eq!(update.locals[2], Type::F32);
    }

    #[test]
    fn assigning_a_pos_axis_resolves_to_a_pos_field_place() {
        let typed = check_clean("func update(dt: f32) { pos.y = 3.0; }");
        let update = typed.function("update").unwrap();
        assert!(matches!(
            update.body.stmts[0],
            TypedStmt::Assign {
                place: Place::PosField(Axis::Y),
                ..
            }
        ));
    }

    #[test]
    fn vec2_has_only_x_and_y() {
        assert_eq!(
            messages("func update(dt: f32) { let z = pos.z; }"),
            vec!["`Vec2` has no field `z`"]
        );
    }

    #[test]
    fn a_number_has_no_fields() {
        assert_eq!(
            messages("func update(dt: f32) { let bad = dt.x; }"),
            vec!["`f32` has no fields"]
        );
    }

    // --- assignment ---

    #[test]
    fn compound_assignment_desugars_to_a_read_and_a_binary_op() {
        let typed = check_clean("let speed = 1.0;\nfunc update(dt: f32) { speed += 2.0; }");
        let update = typed.function("update").unwrap();
        let TypedStmt::Assign { place, value } = &update.body.stmts[0] else {
            panic!("expected an assignment");
        };
        assert_eq!(*place, Place::Global(0));
        let TypedExprKind::Binary { op, lhs, .. } = &value.kind else {
            panic!("compound assignment should become a binary op");
        };
        assert_eq!(*op, BinaryOp::AddF32);
        assert!(
            matches!(lhs.kind, TypedExprKind::Global(0)),
            "reads the same place"
        );
    }

    #[test]
    fn a_literal_cannot_be_assigned_to() {
        assert_eq!(
            messages("func update(dt: f32) { 1.0 = 2.0; }"),
            vec!["this cannot be assigned to"]
        );
    }

    #[test]
    fn assigning_an_unknown_name_reports_once() {
        assert_eq!(
            messages("func update(dt: f32) { missing = 1.0; }"),
            vec!["cannot find `missing` in this scope"]
        );
    }

    // --- calls ---

    #[test]
    fn calling_with_the_wrong_arity_is_reported() {
        assert_eq!(
            messages("func f(a: f32) -> f32 { a }\nfunc update(dt: f32) { let x = f(1.0, 2.0); }"),
            vec!["`f` takes 1 argument, but 2 were given"]
        );
    }

    #[test]
    fn calling_with_the_wrong_argument_type_is_reported() {
        assert_eq!(
            messages("func f(a: f32) -> f32 { a }\nfunc update(dt: f32) { let x = f(true); }"),
            vec!["expected `f32`, found `bool`"]
        );
    }

    #[test]
    fn an_unknown_function_is_reported() {
        assert_eq!(
            messages("func update(dt: f32) { nope(1.0); }"),
            vec!["cannot find function `nope`"]
        );
    }

    #[test]
    fn print_takes_a_string_and_nothing_else() {
        check_clean(r#"func update(dt: f32) { print("hi"); }"#);
        assert_eq!(
            messages("func update(dt: f32) { print(1.0); }"),
            vec!["expected `String`, found `f32`"]
        );
    }

    #[test]
    fn a_function_may_call_one_defined_later() {
        check_clean(
            "func update(dt: f32) { pos.x = later(dt); }\n\
             func later(v: f32) -> f32 { v }",
        );
    }

    // --- returns ---

    #[test]
    fn a_missing_return_is_reported() {
        let messages = messages("func f(a: f32) -> f32 { let b = a; }");
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].contains("must return `f32`"),
            "got: {messages:?}"
        );
    }

    #[test]
    fn returning_from_every_branch_satisfies_the_check() {
        check_clean(
            "func f(a: f32) -> f32 {\n\
                 if a > 0.0 { return 1.0; } else { return 2.0; }\n\
             }",
        );
    }

    #[test]
    fn a_return_value_must_match_the_declared_type() {
        assert_eq!(
            messages("func f(a: f32) -> f32 { return true; }"),
            vec!["expected `f32`, found `bool`"]
        );
    }

    #[test]
    fn a_bare_return_needs_a_value_when_the_function_returns_one() {
        let messages = messages("func f(a: f32) -> f32 { if a > 0.0 { return; } 1.0 }");
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("needs a value"), "got: {messages:?}");
    }

    #[test]
    fn a_tail_expression_must_match_the_declared_return_type() {
        assert_eq!(
            messages("func f(a: f32) -> f32 { true }"),
            vec!["expected `f32`, found `bool`"]
        );
    }

    // --- conditions and operators ---

    #[test]
    fn an_if_condition_must_be_a_bool() {
        assert_eq!(
            messages("func update(dt: f32) { if dt { } }"),
            vec!["expected `bool`, found `f32`"]
        );
    }

    #[test]
    fn a_while_condition_must_be_a_bool() {
        assert_eq!(
            messages("func update(dt: f32) { while dt { } }"),
            vec!["expected `bool`, found `f32`"]
        );
    }

    #[test]
    fn comparison_yields_a_bool_and_arithmetic_an_f32() {
        let typed = check_clean("func update(dt: f32) { let a = dt * 2.0; let b = dt > 1.0; }");
        let update = typed.function("update").unwrap();
        assert_eq!(update.locals[1], Type::F32);
        assert_eq!(update.locals[2], Type::Bool);
    }

    #[test]
    fn logical_operators_require_bools() {
        check_clean("func update(dt: f32) { let ok = dt > 0.0 && dt < 1.0; }");
        assert_eq!(
            messages("func update(dt: f32) { let bad = dt && true; }"),
            vec!["expected `bool`, found `f32`"]
        );
    }

    #[test]
    fn equality_needs_matching_operand_types() {
        check_clean("func update(dt: f32) { let same = dt == 1.0; }");
        assert_eq!(
            messages("func update(dt: f32) { let bad = dt == true; }"),
            vec!["expected `f32`, found `bool`"]
        );
    }

    #[test]
    fn strings_cannot_be_compared_yet_and_say_so() {
        let messages = messages(r#"func update(dt: f32) { let bad = "a" == "b"; }"#);
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].contains("cannot be compared"),
            "an honest limit, not a silent pointer comparison: {messages:?}"
        );
    }

    #[test]
    fn negation_needs_a_number_and_not_needs_a_bool() {
        check_clean("func update(dt: f32) { let a = -dt; let b = !(dt > 0.0); }");
        assert_eq!(
            messages("func update(dt: f32) { let bad = !dt; }"),
            vec!["expected `bool`, found `f32`"]
        );
    }

    // --- duplicate and reserved names ---

    #[test]
    fn defining_the_same_function_twice_is_reported() {
        let messages = messages("func f() { }\nfunc f() { }");
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("already defined"), "got: {messages:?}");
    }

    #[test]
    fn the_names_the_emitted_module_uses_are_reserved() {
        // Not pedantry: these become wasm export names, which must be unique, so
        // without this the module would simply be invalid.
        for source in ["func memory() { }", "func comet_alloc() { }"] {
            let messages = messages(source);
            assert_eq!(messages.len(), 1, "{source}: {messages:?}");
            assert!(messages[0].contains("reserved"), "{source}: {messages:?}");
        }
        check_clean("func comet() { }\nfunc memory_used() { }");
    }

    #[test]
    fn a_duplicate_definition_checks_each_body_against_its_own_parameters() {
        // Regression: with signatures keyed by name, the second `f`'s (empty)
        // parameter list was used to check the first `f`'s body, and reading `a`
        // indexed past the end of its slot table.
        let messages = messages("func f(a: f32) { let x = a; }\nfunc f() { }");
        assert_eq!(messages.len(), 1, "only the duplicate name: {messages:?}");
        assert!(messages[0].contains("already defined"), "got: {messages:?}");
    }

    #[test]
    fn a_call_resolves_to_the_first_definition_of_a_duplicated_name() {
        // The duplicate is reported and then ignored, rather than silently
        // rebinding every call in the file to the later definition.
        let messages = messages(
            "func f(a: f32) -> f32 { a }\n\
             func f() -> f32 { 1.0 }\n\
             func update(dt: f32) { pos.x = f(dt); }",
        );
        assert_eq!(messages.len(), 1, "the call is fine: {messages:?}");
        assert!(messages[0].contains("already defined"), "got: {messages:?}");
    }

    #[test]
    fn a_script_cannot_redefine_a_host_function() {
        let messages = messages("func print(s: String) { }");
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].contains("provided by the engine"),
            "got: {messages:?}"
        );
    }

    // --- a syntactically broken file still checks what parsed ---

    #[test]
    fn checking_survives_a_file_that_did_not_fully_parse() {
        let (script, parse_diagnostics) = parse(include_str!("../tests/fixtures/unclosed.cmt"));
        assert!(!parse_diagnostics.is_empty());
        // The point: this does not panic, and the statements that did parse are
        // still typed - which is what keeps squiggles alive while you type.
        let (typed, _) = check(&script);
        let update = typed.function("update").expect("update still parsed");
        assert_eq!(update.locals[1], Type::F32, "`let speed` was still typed");
    }
}
