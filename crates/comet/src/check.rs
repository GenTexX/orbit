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
use crate::schema::HostSchema;
use crate::span::Span;
use crate::tir::*;

/// Check `script`, returning the typed IR and any diagnostics. A script with
/// diagnostics still yields a tree - the bad parts are [`Type::Error`].
pub fn check(script: &ast::Script, schema: &HostSchema) -> (TypedScript, Vec<Diagnostic>) {
    let mut checker = Checker {
        enums: Vec::new(),
        schema,
        diagnostics: Vec::new(),
        globals: HashMap::new(),
        global_types: Vec::new(),
        signatures: Vec::new(),
        by_name: HashMap::new(),
        locals: Vec::new(),
        scopes: Vec::new(),
        bindings: Vec::new(),
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

struct Checker<'a> {
    /// The script's enums, in declaration order. `Type::Enum` indexes this.
    enums: Vec<TypedEnum>,
    /// What this script can reach outside itself. comet has no built-in idea of
    /// what a node is; the engine says.
    schema: &'a HostSchema,
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
    /// One entry per local slot: what a `let` bound, and whether anything has
    /// read it. `None` for parameters and for slots the checker introduced.
    bindings: Vec<Option<Binding>>,
    ret: Type,
}

impl Checker<'_> {
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

    /// `State::Idle`, or `Hit::Wall(3.0)`.
    fn make_variant(
        &mut self,
        enum_name: &str,
        enum_span: Span,
        variant: &str,
        variant_span: Span,
        args: &[ast::Expr],
    ) -> (TypedExprKind, Type) {
        let Some(index) = self.enum_index(enum_name) else {
            if !enum_name.is_empty() {
                let candidates = self.enums.iter().map(|e| e.name.as_str());
                let message = match nearest(enum_name, candidates) {
                    Some(suggestion) => {
                        format!("cannot find enum `{enum_name}` - did you mean `{suggestion}`?")
                    }
                    None => format!("cannot find enum `{enum_name}`"),
                };
                self.error(enum_span, message);
            }
            return (TypedExprKind::Error, Type::Error);
        };
        let found = self.enums[index as usize]
            .variants
            .iter()
            .position(|v| v.name == variant);
        let Some(position) = found else {
            let candidates = self.enums[index as usize]
                .variants
                .iter()
                .map(|v| v.name.as_str());
            let message = match nearest(variant, candidates) {
                Some(suggestion) => format!(
                    "`{enum_name}` has no variant `{variant}` - did you mean `{suggestion}`?"
                ),
                None => format!("`{enum_name}` has no variant `{variant}`"),
            };
            self.error(variant_span, message);
            return (TypedExprKind::Error, Type::Error);
        };

        let payload = self.enums[index as usize].variants[position]
            .payload
            .clone();
        let mut checked: Vec<TypedExpr> = args.iter().map(|a| self.expr(a)).collect();
        if checked.len() != payload.len() {
            self.error(
                variant_span,
                format!(
                    "`{variant}` carries {} value{}, but {} {} given",
                    payload.len(),
                    if payload.len() == 1 { "" } else { "s" },
                    checked.len(),
                    if checked.len() == 1 { "was" } else { "were" }
                ),
            );
            return (TypedExprKind::Error, Type::Error);
        }
        for (arg, want) in checked.iter_mut().zip(&payload) {
            let taken = std::mem::replace(
                arg,
                TypedExpr {
                    kind: TypedExprKind::Error,
                    ty: Type::Error,
                    span: variant_span,
                },
            );
            *arg = self.coerce(taken, *want);
        }
        (
            TypedExprKind::MakeVariant {
                enum_index: index,
                variant: position as u32,
                args: checked,
            },
            Type::Enum(index),
        )
    }

    /// `match x { ... }`, checked for exhaustiveness.
    ///
    /// Every variant must have an arm. There is no wildcard on purpose: a
    /// compiler that tells a learner they forgot a case is the point of the
    /// construct, and `_` is how that goes quiet.
    fn match_expr(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        span: Span,
    ) -> (TypedExprKind, Type) {
        let scrutinee = self.expr(scrutinee);
        let Type::Enum(index) = scrutinee.ty else {
            if !scrutinee.ty.is_error() {
                self.error(
                    scrutinee.span,
                    format!(
                        "`match` works on an enum, and this is a {}",
                        scrutinee.ty.name()
                    ),
                );
            }
            return (TypedExprKind::Error, Type::Error);
        };

        let variants = self.enums[index as usize].variants.clone();
        let mut checked: Vec<Option<TypedArm>> = vec![None; variants.len()];
        let mut result = Type::Error;
        let mut first_span = span;
        for arm in arms {
            let Some(position) = variants.iter().position(|v| v.name == arm.variant) else {
                let message = match nearest(&arm.variant, variants.iter().map(|v| v.name.as_str()))
                {
                    Some(suggestion) => format!(
                        "`{}` has no variant `{}` - did you mean `{suggestion}`?",
                        self.enums[index as usize].name, arm.variant
                    ),
                    None => format!(
                        "`{}` has no variant `{}`",
                        self.enums[index as usize].name, arm.variant
                    ),
                };
                self.error(arm.variant_span, message);
                continue;
            };
            if checked[position].is_some() {
                self.error(
                    arm.variant_span,
                    format!("`{}` already has an arm", arm.variant),
                );
                continue;
            }

            // The payload's names are bound for this arm's body only.
            let payload = variants[position].payload.clone();
            if arm.bindings.len() != payload.len() {
                self.error(
                    arm.variant_span,
                    format!(
                        "`{}` carries {} value{}, but this pattern names {}",
                        arm.variant,
                        payload.len(),
                        if payload.len() == 1 { "" } else { "s" },
                        arm.bindings.len()
                    ),
                );
                continue;
            }
            self.scopes.push(HashMap::new());
            let bindings: Vec<u32> = arm
                .bindings
                .iter()
                .zip(&payload)
                .map(|((name, name_span), ty)| self.declare_local(name, *ty, Some(*name_span)))
                .collect();
            let body = self.expr(&arm.body);
            self.scopes.pop();

            // Every arm has the same type: it is one expression with one value.
            if result.is_error() {
                result = body.ty;
                first_span = body.span;
            } else if body.ty != result && !body.ty.is_error() {
                self.error(
                    body.span,
                    format!(
                        "every arm of a `match` has to produce the same type - this one is {}, \
                         and the first is {}",
                        body.ty.name(),
                        result.name()
                    ),
                );
            }
            let _ = first_span;
            checked[position] = Some(TypedArm { bindings, body });
        }

        let missing: Vec<&str> = checked
            .iter()
            .zip(&variants)
            .filter(|(arm, _)| arm.is_none())
            .map(|(_, variant)| variant.name.as_str())
            .collect();
        if !missing.is_empty() {
            let list = missing.join("`, `");
            self.error(
                span,
                format!(
                    "this `match` does not cover `{list}` - every variant needs an arm, and \
                     there is no wildcard"
                ),
            );
            return (TypedExprKind::Error, Type::Error);
        }

        (
            TypedExprKind::Match {
                scrutinee: Box::new(scrutinee),
                enum_index: index,
                arms: checked
                    .into_iter()
                    .map(|a| a.expect("all present"))
                    .collect(),
            },
            result,
        )
    }

    /// Whether releasing a value of this type has anything to do.
    fn owns_str(&self, ty: Type) -> bool {
        match ty {
            Type::Str => true,
            Type::Enum(index) => self.enums[index as usize].holds_str,
            _ => false,
        }
    }

    /// Whether a value of this type can be stored on the component and written
    /// back into a running module.
    ///
    /// A `String` is a pointer into the module's own memory, so handing one
    /// across needs an ownership rule that is its own decision. An enum is one
    /// number - its tag - as long as no variant carries anything, which is also
    /// exactly when it has a default to fall back to.
    fn is_exportable(&self, ty: Type) -> bool {
        match ty {
            Type::F32 | Type::Int | Type::Bool | Type::Vec2 => true,
            Type::Enum(index) => self.enums[index as usize]
                .variants
                .iter()
                .all(|v| v.payload.is_empty()),
            _ => false,
        }
    }

    /// The index of the enum called `name`, if the script declares one.
    fn enum_index(&self, name: &str) -> Option<u32> {
        self.enums
            .iter()
            .position(|e| e.name == name)
            .map(|i| i as u32)
    }

    /// How many wasm slots a value of this type occupies. The checker needs it
    /// only to size an enum's payload; codegen has the authoritative version.
    fn slots(&self, ty: Type) -> usize {
        match ty {
            Type::Vec2 => 2,
            Type::Unit | Type::Error => 0,
            // The tag plus the widest payload. An enum inside an enum is
            // flattened rather than boxed, so it stays a stack value.
            Type::Enum(index) => 1 + self.enums[index as usize].payload_slots,
            _ => 1,
        }
    }

    /// Check a declaration's annotations, returning whether it is exported.
    ///
    /// Only `@export` exists in v1. The others - `@range`, `@tooltip` and the
    /// rest - are a decision that has not been taken, so an unknown one is
    /// reported rather than ignored: a typo that silently does nothing is the
    /// wrong failure mode for a language meant to be learned from, and is the
    /// reason annotations are grammar rather than a doc-comment convention.
    fn annotations(&mut self, decl: &ast::StateDecl) -> bool {
        let mut exported = false;
        for annotation in &decl.annotations {
            if annotation.name != "export" {
                let message = match nearest(&annotation.name, ["export"].into_iter()) {
                    Some(suggestion) => format!(
                        "unknown annotation `@{}` - did you mean `@{suggestion}`?",
                        annotation.name
                    ),
                    None => format!("unknown annotation `@{}`", annotation.name),
                };
                self.error(annotation.name_span, message);
                continue;
            }
            if !annotation.args.is_empty() {
                self.error(annotation.span, "`@export` takes no arguments");
            }
            if exported {
                self.error(annotation.span, "`@export` is already on this declaration");
            }
            exported = true;
        }
        exported
    }

    /// The value a declaration with no initializer starts with.
    ///
    /// Every built-in type has an obvious answer. A user-defined enum does not,
    /// so its first variant is it: an arbitrary rule, chosen because the
    /// alternatives were a warning with an exception in it, or refusing to
    /// export enums at all. A variant carrying a payload cannot be a default -
    /// there is nothing to fill the payload with - so that is an error.
    fn default_value(&mut self, ty: Type, span: Span) -> TypedExpr {
        let kind = match ty {
            Type::F32 => TypedExprKind::Number(0.0),
            Type::Int => TypedExprKind::Int(0),
            Type::Bool => TypedExprKind::Bool(false),
            Type::Vec2 => TypedExprKind::MakeVec2 {
                x: Box::new(TypedExpr {
                    kind: TypedExprKind::Number(0.0),
                    ty: Type::F32,
                    span,
                }),
                y: Box::new(TypedExpr {
                    kind: TypedExprKind::Number(0.0),
                    ty: Type::F32,
                    span,
                }),
            },
            Type::Str => TypedExprKind::Str(String::new()),
            Type::Enum(index) => {
                let first = self.enums[index as usize].variants.first();
                match first {
                    Some(variant) if variant.payload.is_empty() => TypedExprKind::MakeVariant {
                        enum_index: index,
                        variant: 0,
                        args: Vec::new(),
                    },
                    Some(variant) => {
                        let name = variant.name.clone();
                        self.error(
                            span,
                            format!(
                                "`{name}` is this enum's first variant and so its default, but it \
                                 carries a value - give the declaration an initializer, or put a \
                                 variant with no payload first"
                            ),
                        );
                        TypedExprKind::Error
                    }
                    None => TypedExprKind::Error,
                }
            }
            Type::Unit | Type::Error => TypedExprKind::Error,
        };
        TypedExpr { kind, ty, span }
    }

    /// Make `expr` fit `want`, widening an `int` where an `f32` is asked for,
    /// and reporting a mismatch otherwise.
    ///
    /// Everything that checks an expression against an expected type goes
    /// through here - a `let` with an annotation, an argument, a return, an
    /// assignment - so the widening rule lives in one place instead of at each
    /// of them, where one would eventually be missed.
    fn coerce(&mut self, expr: TypedExpr, want: Type) -> TypedExpr {
        if want == Type::F32 && expr.ty == Type::Int {
            let span = expr.span;
            return TypedExpr {
                kind: TypedExprKind::Widen(Box::new(expr)),
                ty: Type::F32,
                span,
            };
        }
        self.expect_type(want, expr.ty, expr.span);
        expr
    }

    /// Widen both sides of a mixed-numeric operation to `f32`, or leave two
    /// ints alone. Returns the type the operation works in.
    ///
    /// The widening is one-directional on purpose: there is no precision-loss
    /// surprise, and a script written before `int` existed keeps compiling
    /// because a bare `5` still fits everywhere an `f32` is wanted.
    fn unify_numeric(&mut self, lhs: &mut TypedExpr, rhs: &mut TypedExpr) -> Type {
        if lhs.ty == Type::Int && rhs.ty == Type::Int {
            return Type::Int;
        }
        let widen = |expr: &mut TypedExpr| {
            if expr.ty == Type::Int {
                let taken = std::mem::replace(
                    expr,
                    TypedExpr {
                        kind: TypedExprKind::Error,
                        ty: Type::Error,
                        span: Span::new(0, 0),
                    },
                );
                let span = taken.span;
                *expr = TypedExpr {
                    kind: TypedExprKind::Widen(Box::new(taken)),
                    ty: Type::F32,
                    span,
                };
            }
        };
        widen(lhs);
        widen(rhs);
        Type::F32
    }

    fn resolve_type(&mut self, name: &ast::TypeName) -> Type {
        match Type::from_name(&name.name) {
            Some(ty) => ty,
            None if self.enum_index(&name.name).is_some() => {
                Type::Enum(self.enum_index(&name.name).expect("just checked"))
            }
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
        // Enums first: a state initializer, a signature, or another enum's
        // payload can name one, and none of them can be checked until the set
        // of type names is known.
        for (index, decl) in script.enums.iter().enumerate() {
            if self.enum_index(&decl.name).is_some() && !decl.name.is_empty() {
                self.error(
                    decl.name_span,
                    format!("`{}` is already defined in this script", decl.name),
                );
            }
            self.enums.push(TypedEnum {
                name: decl.name.clone(),
                variants: Vec::new(),
                holds_str: false,
                payload_slots: 0,
            });
            let _ = index;
        }
        // Then their payloads, in a second pass, so one enum may carry another
        // regardless of which was written first.
        for (index, decl) in script.enums.iter().enumerate() {
            let mut variants = Vec::new();
            let mut widest = 0;
            for variant in &decl.variants {
                if variants
                    .iter()
                    .any(|v: &TypedVariant| v.name == variant.name)
                {
                    self.error(
                        variant.name_span,
                        format!("`{}` is already a variant of this enum", variant.name),
                    );
                }
                let payload: Vec<Type> = variant
                    .payload
                    .iter()
                    .map(|t| self.resolve_type(t))
                    .collect();
                widest = widest.max(payload.iter().map(|t| self.slots(*t)).sum::<usize>());
                variants.push(TypedVariant {
                    name: variant.name.clone(),
                    payload,
                });
            }
            if variants.is_empty() && !decl.name.is_empty() {
                self.error(decl.span, "an enum needs at least one variant");
            }
            self.enums[index].variants = variants;
            self.enums[index].payload_slots = widest;
        }
        // Whether an enum can hold a String is a property of the whole graph -
        // one enum's payload may be another - so it settles by repetition
        // rather than in one pass. The set only grows, so this terminates.
        loop {
            let mut changed = false;
            for index in 0..self.enums.len() {
                if self.enums[index].holds_str {
                    continue;
                }
                let holds = self.enums[index]
                    .variants
                    .iter()
                    .flat_map(|v| &v.payload)
                    .any(|ty| self.owns_str(*ty));
                if holds {
                    self.enums[index].holds_str = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

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
            self.check_hook(index, f);
        }

        // Then script state, in order: an initializer sees the globals declared
        // above it, which keeps the whole thing acyclic without a dependency
        // graph.
        let mut state = Vec::new();
        for decl in &script.state {
            let exported = self.annotations(decl);
            let declared = decl.ty.as_ref().map(|t| self.resolve_type(t));
            let (init, ty) = match (&decl.init, declared) {
                (Some(written), Some(declared)) => {
                    let init = self.expr(written);
                    (self.coerce(init, declared), declared)
                }
                (Some(written), None) => {
                    let init = self.expr(written);
                    let ty = init.ty;
                    (init, ty)
                }
                // No initializer: the type's default. This is what an exported
                // variable should look like - the inspector owns the value, so
                // a number in the source would be a second answer to the same
                // question, and the one a reader sees first is the one that
                // loses.
                (None, Some(declared)) => (self.default_value(declared, decl.span), declared),
                (None, None) => {
                    self.error(
                        decl.span,
                        format!(
                            "`{}` needs a type or a value - with neither there is nothing to \
                             work out what it holds",
                            decl.name
                        ),
                    );
                    (
                        TypedExpr {
                            kind: TypedExprKind::Error,
                            ty: Type::Error,
                            span: decl.span,
                        },
                        Type::Error,
                    )
                }
            };
            if exported && decl.init.is_some() {
                self.diagnostics.push(Diagnostic::warning(
                    decl.span,
                    format!(
                        "`{}` is exported, so the inspector owns its value - what is written \
                         here is only the default and is not what runs",
                        decl.name
                    ),
                ));
            }
            if exported && !self.is_exportable(ty) && !ty.is_error() {
                let detail = match ty {
                    // The tag is one number; a payload is not, and there is no
                    // way to put one in the inspector yet.
                    Type::Enum(_) => "an enum whose variants carry values cannot be exported yet",
                    _ => {
                        "only f32, int, bool, Vec2 and payload-free enums can be stored on the component"
                    }
                };
                self.error(
                    decl.span,
                    format!("a `{}` cannot be exported yet - {detail}", ty.name()),
                );
            }
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
                exported,
            });
        }

        let functions = script
            .functions
            .iter()
            .enumerate()
            .map(|(index, f)| self.function(index, f))
            .collect();
        TypedScript {
            enums: std::mem::take(&mut self.enums),
            state,
            functions,
        }
    }

    fn function(&mut self, index: usize, f: &ast::Function) -> TypedFn {
        let signature = &self.signatures[index];
        let ret = signature.ret;
        let param_types = signature.params.clone();

        self.locals = param_types;
        self.ret = ret;
        self.scopes = vec![HashMap::new()];
        // Parameters get no binding entry: an unused parameter is normal - a
        // handler takes `dt` whether or not it needs it - and warning about one
        // would train people to ignore warnings.
        self.bindings = f.params.iter().map(|_| None).collect();
        for (i, p) in f.params.iter().enumerate() {
            self.scopes[0].insert(p.name.clone(), i as u32);
        }

        let mut body = self.block(&f.body);
        self.report_unused();

        // The body's tail is the function's value when it has one, so it is
        // coerced like any other place a type is expected - `-> f32 { 1 }`
        // widens rather than being refused.
        if let Some(mut tail) = body.tail.take() {
            // A tail from recovered source can have an empty span; blame the
            // block instead, so the squiggle lands somewhere a reader can see.
            tail.span = tail.ty_span(f.body.span);
            let coerced = self.coerce(tail, ret);
            body.ty = coerced.ty;
            body.tail = Some(coerced);
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

    /// Reject a function that takes one of the engine's hook names but does not
    /// have that hook's signature.
    ///
    /// The host looks the name up at exactly one signature and quietly gets
    /// nothing back when it does not match, so a script that declares
    /// `func update()` compiles to a perfectly good module that is then never
    /// called. From inside the editor that is indistinguishable from the script
    /// not running, with nothing anywhere saying why. This is the only place a
    /// person finds out before wondering why their node does not move.
    ///
    /// An error rather than a warning: the module would be valid WebAssembly,
    /// but it is not a valid script for this engine, and a diagnostic nobody
    /// can miss is the entire point of the check.
    fn check_hook(&mut self, index: usize, f: &ast::Function) {
        let Some(hook) = HOOKS.iter().find(|hook| hook.name == f.name) else {
            return;
        };
        let signature = &self.signatures[index];
        // A signature with a type that did not resolve has already been
        // reported; one mistake yields one diagnostic.
        if signature.ret.is_error() || signature.params.iter().any(|ty| ty.is_error()) {
            return;
        }
        if signature.params != hook.params || signature.ret != hook.ret {
            let written = hook.written;
            self.error(
                f.name_span,
                format!(
                    "`{}` is called by the engine and must be written `{written}`",
                    f.name
                ),
            );
        }
    }

    /// Warn about every `let` in the function just checked that nothing read.
    ///
    /// The first thing in comet to emit a warning rather than an error: the
    /// script still compiles and still runs. It is here because an unused
    /// binding is usually a typo one line later - the value went into `speed`
    /// and the line below reads `sped`, which is already an error, and this is
    /// the other half of the same story.
    fn report_unused(&mut self) {
        let unused: Vec<(Span, String)> = std::mem::take(&mut self.bindings)
            .into_iter()
            .flatten()
            .filter(|b| !b.used)
            .map(|b| (b.span, b.name))
            .collect();
        for (span, name) in unused {
            self.diagnostics.push(Diagnostic::warning(
                span,
                format!("`{name}` is never used - prefix it with `_` if that is deliberate"),
            ));
        }
    }

    /// Report a property the host schema does not have, suggesting the nearest
    /// one it does - the engine's property names are exactly the kind of thing
    /// nobody remembers exactly.
    fn unknown_property(&mut self, object: &str, field: &str, span: Span) {
        let names = self
            .schema
            .fields_of(object)
            .iter()
            .map(|f| f.name.as_str());
        let message = match nearest(field, names) {
            Some(suggestion) => {
                format!("`{object}` has no property `{field}` - did you mean `{suggestion}`?")
            }
            None => format!("`{object}` has no property `{field}`"),
        };
        self.error(span, message);
    }

    /// Note that something read the local in `slot`.
    fn mark_used(&mut self, slot: u32) {
        if let Some(Some(binding)) = self.bindings.get_mut(slot as usize) {
            binding.used = true;
        }
    }

    // --- statements ---

    fn block(&mut self, block: &ast::Block) -> TypedBlock {
        self.scopes.push(HashMap::new());
        let mut stmts = Vec::with_capacity(block.stmts.len());
        for stmt in &block.stmts {
            // One source statement can become several typed ones: `for` is
            // checked by lowering it to the `while` a reader would have written
            // by hand.
            match stmt {
                ast::Stmt::For { .. } => self.for_stmt(stmt, &mut stmts),
                other => stmts.push(self.stmt(other)),
            }
        }
        let tail = block.tail.as_ref().map(|e| self.expr(e));
        self.scopes.pop();
        let ty = tail.as_ref().map_or(Type::Unit, |t| t.ty);
        TypedBlock { stmts, tail, ty }
    }

    /// Lower `for i in a..b { body }` into the three statements that mean the
    /// same thing:
    ///
    /// ```text
    /// let i = a;
    /// let <end> = b;
    /// while i < <end> { body; i = i + 1.0; }
    /// ```
    ///
    /// Doing it here rather than in codegen means `for` cannot drift from the
    /// `while` it claims to be, and codegen never learns that `for` exists. The
    /// upper bound is hoisted into a local of its own so it is evaluated once,
    /// which is what makes `for i in 0..count()` call `count` a single time.
    fn for_stmt(&mut self, stmt: &ast::Stmt, out: &mut Vec<TypedStmt>) {
        let ast::Stmt::For {
            name,
            name_span,
            start,
            end,
            body,
            ..
        } = stmt
        else {
            unreachable!("only called for a `for`");
        };

        // The counter is an `int`, which is the case the type was argued for:
        // a counted loop over whole numbers, with no fencepost weirdness and no
        // `for i in 0.0..3.5` to wonder about.
        let start = self.expr(start);
        let end = self.expr(end);
        self.expect_type(Type::Int, start.ty, start.span);
        self.expect_type(Type::Int, end.ty, end.span);

        // The loop variable and the bound live in a scope of their own, so
        // neither is in scope after the loop and neither shadows anything for
        // longer than the loop lasts.
        self.scopes.push(HashMap::new());
        // The counter is declared without a span: a `for` that does not use its
        // own variable is an ordinary way to repeat something N times.
        let slot = self.declare_local(name, Type::Int, None);
        // A name with a space in it: no source text can lex to this, so the
        // bound cannot be read or written by the script that produced it.
        let end_slot = self.declare_local("for end", Type::Int, None);
        out.push(TypedStmt::Let { slot, init: start });
        out.push(TypedStmt::Let {
            slot: end_slot,
            init: end,
        });

        let counter = |kind| TypedExpr {
            kind,
            ty: Type::Int,
            span: *name_span,
        };
        let cond = TypedExpr {
            kind: TypedExprKind::Binary {
                op: BinaryOp::LtInt,
                lhs: Box::new(counter(TypedExprKind::Local(slot))),
                rhs: Box::new(counter(TypedExprKind::Local(end_slot))),
            },
            ty: Type::Bool,
            span: *name_span,
        };

        let mut body = self.block(body);
        body.stmts.push(TypedStmt::Assign {
            place: Place::Local(slot),
            value: TypedExpr {
                kind: TypedExprKind::Binary {
                    op: BinaryOp::AddInt,
                    lhs: Box::new(counter(TypedExprKind::Local(slot))),
                    rhs: Box::new(counter(TypedExprKind::Int(1))),
                },
                ty: Type::Int,
                span: *name_span,
            },
        });
        self.scopes.pop();

        out.push(TypedStmt::While { cond, body });
    }

    /// Declare a local, recording where its name was written so an unused one
    /// can be pointed at. `span` is `None` for a slot the checker introduced
    /// itself, which nobody can use and nobody should be warned about.
    fn declare_local(&mut self, name: &str, ty: Type, span: Option<Span>) -> u32 {
        let slot = self.locals.len() as u32;
        self.locals.push(ty);
        // `_` is the way to say "I know, and I mean it" - for a binding kept
        // for its shape, or a loop counter the body does not care about.
        self.bindings
            .push(span.filter(|_| !name.starts_with('_')).map(|span| Binding {
                name: name.to_string(),
                span,
                used: false,
            }));
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
                name_span,
                ty,
                init,
                span,
            } => {
                let mut init = self.expr(init);
                let declared = ty.as_ref().map(|t| self.resolve_type(t));
                let ty = match declared {
                    Some(declared) => {
                        init = self.coerce(init, declared);
                        declared
                    }
                    None => init.ty,
                };
                if ty == Type::Unit {
                    self.error(*span, "a `let` needs a value, but this produces nothing");
                }
                let slot = self.declare_local(name, ty, Some(*name_span));
                TypedStmt::Let { slot, init }
            }

            ast::Stmt::Assign {
                target, op, value, ..
            } => {
                let (place, target_ty) = self.place(target);
                let value = self.expr(value);
                match op {
                    ast::AssignOp::Set => {
                        let value = self.coerce(value, target_ty);
                        TypedStmt::Assign { place, value }
                    }
                    _ => {
                        // Desugar to a plain store of the operation, and check
                        // it the way the operator itself is checked - so
                        // `pos += vel * dt` works exactly when `pos + vel * dt`
                        // does, and there is one definition of what `+` means
                        // rather than a second one that has to be kept in step.
                        let op = match op {
                            ast::AssignOp::Add => ast::BinaryOp::Add,
                            ast::AssignOp::Sub => ast::BinaryOp::Sub,
                            ast::AssignOp::Mul => ast::BinaryOp::Mul,
                            ast::AssignOp::Div => ast::BinaryOp::Div,
                            ast::AssignOp::Rem => ast::BinaryOp::Rem,
                            ast::AssignOp::Set => unreachable!("handled above"),
                        };
                        let span = value.span;
                        let read = self.place_read(&place, target_ty, target.span());
                        let (kind, ty) = self.binary(op, read, value);
                        // The result has to fit back where it came from. Every
                        // operator reachable here returns one of its operand
                        // types, so this only fires on a mismatch the operator
                        // itself was happy with.
                        if !ty.is_error() && !target_ty.is_error() && ty != target_ty {
                            self.error(
                                span,
                                format!(
                                    "`{}=` on {} produces {}, which cannot be stored back",
                                    symbol(op),
                                    target_ty.name(),
                                    ty.name()
                                ),
                            );
                            return TypedStmt::Assign {
                                place,
                                value: TypedExpr {
                                    kind: TypedExprKind::Error,
                                    ty: Type::Error,
                                    span,
                                },
                            };
                        }
                        TypedStmt::Assign {
                            place,
                            value: TypedExpr { kind, ty, span },
                        }
                    }
                }
            }

            ast::Stmt::If(if_stmt) => self.if_stmt(if_stmt),

            ast::Stmt::For { .. } => {
                unreachable!("`for` is lowered by for_stmt, which block calls directly")
            }

            ast::Stmt::While { cond, body, .. } => {
                let cond = self.expr(cond);
                self.expect_type(Type::Bool, cond.ty, cond.span);
                TypedStmt::While {
                    cond,
                    body: self.block(body),
                }
            }

            ast::Stmt::Return { value, span } => {
                let mut value = value.as_ref().map(|v| self.expr(v));
                match (&value, self.ret) {
                    (Some(_), ret) => value = value.map(|v| self.coerce(v, ret)),
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
                if let Some(slot) = self.lookup_local(name) {
                    return (Place::Local(slot), self.locals[slot as usize]);
                }
                if let Some(&slot) = self.globals.get(name) {
                    return (Place::Global(slot), self.global_types[slot as usize]);
                }
                self.unresolved(*span, name);
                (Place::Error, Type::Error)
            }
            ast::Expr::Field {
                receiver,
                field,
                field_span,
                ..
            } => {
                // `transform.position = v`, or one axis of it.
                if let ast::Expr::Ident { name, .. } = receiver.as_ref()
                    && self.lookup_local(name).is_none()
                    && !self.globals.contains_key(name)
                    && self.schema.has_object(name)
                {
                    return match self.schema.resolve(name, field) {
                        Some(found) => (
                            Place::Host {
                                field: found.id,
                                ty: found.ty.ty(),
                                axis: None,
                            },
                            found.ty.ty(),
                        ),
                        None => {
                            self.unknown_property(name, field, *field_span);
                            (Place::Error, Type::Error)
                        }
                    };
                }
                // `transform.position.x = v`: one axis of a Vec2 property,
                // leaving the other as it was.
                if let ast::Expr::Field {
                    receiver: outer,
                    field: property,
                    field_span: property_span,
                    ..
                } = receiver.as_ref()
                    && let ast::Expr::Ident { name, .. } = outer.as_ref()
                    && self.lookup_local(name).is_none()
                    && !self.globals.contains_key(name)
                    && self.schema.has_object(name)
                {
                    let Some(found) = self.schema.resolve(name, property) else {
                        self.unknown_property(name, property, *property_span);
                        return (Place::Error, Type::Error);
                    };
                    if found.ty.ty() != Type::Vec2 {
                        self.error(
                            *field_span,
                            format!("`{}` has no fields", found.ty.ty().name()),
                        );
                        return (Place::Error, Type::Error);
                    }
                    let Some(axis) = axis(field) else {
                        self.error(*field_span, format!("`Vec2` has no field `{field}`"));
                        return (Place::Error, Type::Error);
                    };
                    return (
                        Place::Host {
                            field: found.id,
                            ty: Type::Vec2,
                            axis: Some(axis),
                        },
                        Type::F32,
                    );
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
                    self.unresolved(*span, name);
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
        // Reading a place is what compound assignment does: `x += 1` uses `x`,
        // where a plain `x = 1` does not.
        if let Place::Local(slot) | Place::LocalField(slot, _) = place {
            self.mark_used(*slot);
        }
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
            // Reading a host property back for a compound assignment. With an
            // axis, that means reading the whole Vec2 and taking one component -
            // the property has no per-axis accessor, and inventing one here
            // would be codegen's decision rather than the checker's.
            Place::Host {
                field,
                ty: field_ty,
                axis,
            } => {
                let read = TypedExprKind::HostField {
                    field: *field,
                    ty: *field_ty,
                };
                match axis {
                    Some(axis) => TypedExprKind::Field {
                        receiver: Box::new(TypedExpr {
                            kind: read,
                            ty: *field_ty,
                            span,
                        }),
                        axis: *axis,
                    },
                    None => read,
                }
            }
            Place::Error => TypedExprKind::Error,
        };
        TypedExpr { kind, ty, span }
    }

    fn lookup_local(&self, name: &str) -> Option<u32> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// Report a name nothing resolved, with the nearest thing that would have.
    ///
    /// A typo is the most common error a beginner makes and the least
    /// informative to be told about: "cannot find `spede`" sends you reading,
    /// "did you mean `speed`?" does not. Everything visible from here is a
    /// candidate - locals, script state, functions, builtins - because a typo
    /// does not know what kind of thing it was aiming at.
    fn unresolved(&mut self, span: Span, name: &str) {
        if name.is_empty() {
            return;
        }
        let candidates = self
            .scopes
            .iter()
            .flat_map(|scope| scope.keys())
            .chain(self.globals.keys())
            .chain(self.by_name.keys())
            .map(String::as_str)
            .chain(BUILTIN_NAMES.iter().copied());
        let message = match nearest(name, candidates) {
            Some(suggestion) => {
                format!("cannot find `{name}` in this scope - did you mean `{suggestion}`?")
            }
            None => format!("cannot find `{name}` in this scope"),
        };
        self.error(span, message);
    }

    // --- expressions ---

    fn expr(&mut self, expr: &ast::Expr) -> TypedExpr {
        let span = expr.span();
        let (kind, ty) = match expr {
            ast::Expr::Number { value, .. } => (TypedExprKind::Number(*value as f32), Type::F32),
            ast::Expr::Int { value, span } => match i32::try_from(*value) {
                Ok(value) => (TypedExprKind::Int(value), Type::Int),
                Err(_) => {
                    self.error(*span, format!("`{value}` does not fit in an int"));
                    (TypedExprKind::Error, Type::Error)
                }
            },
            ast::Expr::Bool { value, .. } => (TypedExprKind::Bool(*value), Type::Bool),
            ast::Expr::Str { value, .. } => (TypedExprKind::Str(value.clone()), Type::Str),

            ast::Expr::Ident { name, .. } => {
                if let Some(slot) = self.lookup_local(name) {
                    self.mark_used(slot);
                    (TypedExprKind::Local(slot), self.locals[slot as usize])
                } else if let Some(&slot) = self.globals.get(name) {
                    (
                        TypedExprKind::Global(slot),
                        self.global_types[slot as usize],
                    )
                } else if self.schema.has_object(name) {
                    // A group of properties is not a value. Saying which one to
                    // reach for is more use than "expected a value".
                    let example = self
                        .schema
                        .fields_of(name)
                        .first()
                        .map(|f| format!(" - write `{name}.{}`", f.name))
                        .unwrap_or_default();
                    self.error(
                        span,
                        format!("`{name}` is a group of properties, not a value{example}"),
                    );
                    (TypedExprKind::Error, Type::Error)
                } else {
                    self.unresolved(span, name);
                    (TypedExprKind::Error, Type::Error)
                }
            }

            ast::Expr::Field {
                receiver,
                field,
                field_span,
                ..
            } => {
                // A host property, if the receiver names an object nothing else
                // has shadowed. Checked before the receiver is evaluated as an
                // expression, because as an expression it is an error.
                if let ast::Expr::Ident { name, .. } = receiver.as_ref()
                    && self.lookup_local(name).is_none()
                    && !self.globals.contains_key(name)
                    && self.schema.has_object(name)
                {
                    let (kind, ty) = match self.schema.resolve(name, field) {
                        Some(found) => (
                            TypedExprKind::HostField {
                                field: found.id,
                                ty: found.ty.ty(),
                            },
                            found.ty.ty(),
                        ),
                        None => {
                            self.unknown_property(name, field, *field_span);
                            (TypedExprKind::Error, Type::Error)
                        }
                    };
                    return TypedExpr { kind, ty, span };
                }
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

            ast::Expr::Variant {
                enum_name,
                enum_span,
                variant,
                variant_span,
                args,
                ..
            } => self.make_variant(enum_name, *enum_span, variant, *variant_span, args),

            ast::Expr::Match {
                scrutinee, arms, ..
            } => self.match_expr(scrutinee, arms, span),

            ast::Expr::Unary { op, operand, .. } => {
                let operand = self.expr(operand);
                let (op, want) = match op {
                    // Negating a Vec2 negates both components. Chosen by the
                    // operand's type rather than by a second operator, which is
                    // how `-v` stays the thing a reader expects it to be.
                    ast::UnaryOp::Neg if operand.ty == Type::Vec2 => (UnaryOp::NegVec2, Type::Vec2),
                    ast::UnaryOp::Neg if operand.ty == Type::Int => (UnaryOp::NegInt, Type::Int),
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
        let mut args: Vec<TypedExpr> = args.iter().map(|a| self.expr(a)).collect();

        // `vec2(x, y)` is a constructor rather than a call: it becomes two
        // values on the stack, which is what a Vec2 already is.
        if callee == "vec2" {
            self.check_args(callee, &mut args, &[Type::F32, Type::F32], span);
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
        // `int(x)` truncates toward zero; on something already an int it is a
        // no-op rather than an error, so a script can be explicit without being
        // punished for it.
        if callee == "int" {
            let mut args = args;
            self.check_args(callee, &mut args, &[Type::F32], span);
            let Some(arg) = args.pop() else {
                return (TypedExprKind::Error, Type::Error);
            };
            return match arg.ty {
                Type::Int => (arg.kind, Type::Int),
                Type::F32 => (TypedExprKind::Narrow(Box::new(arg)), Type::Int),
                _ => (TypedExprKind::Error, Type::Error),
            };
        }
        // `str` of a whole number keeps it whole. Widening to f32 first would
        // print a rounded value past 2^24, which is exactly the case `int`
        // exists to be honest about.
        if callee == "str" && args.len() == 1 && args[0].ty == Type::Int {
            return (
                TypedExprKind::HostCall {
                    host: Host::StrInt,
                    args,
                },
                Type::Str,
            );
        }
        if let Some((builtin, params, ret)) = builtin(callee) {
            self.check_args(callee, &mut args, params, span);
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
                // Only callable names are candidates here: what follows the
                // typo is a `(`, so a nearby local would not have helped.
                let candidates = self
                    .by_name
                    .keys()
                    .map(String::as_str)
                    .chain(BUILTIN_NAMES.iter().copied());
                let message = match nearest(callee, candidates) {
                    Some(suggestion) => {
                        format!("cannot find function `{callee}` - did you mean `{suggestion}`?")
                    }
                    None => format!("cannot find function `{callee}`"),
                };
                self.error(callee_span, message);
            }
            return (TypedExprKind::Error, Type::Error);
        };
        let signature = &self.signatures[index];
        let (params, ret) = (signature.params.clone(), signature.ret);
        self.check_args(callee, &mut args, &params, span);
        let ok = args.len() == params.len() && args.iter().zip(&params).all(|(a, p)| a.ty == *p);
        if ok {
            (TypedExprKind::Call { index, args }, ret)
        } else {
            (TypedExprKind::Error, Type::Error)
        }
    }

    /// Check each argument against its parameter, widening an `int` passed
    /// where an `f32` is wanted. Takes the arguments by mutable reference
    /// because the widening has to reach codegen, not merely be permitted.
    fn check_args(&mut self, callee: &str, args: &mut [TypedExpr], params: &[Type], span: Span) {
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
        for (arg, param) in args.iter_mut().zip(params) {
            let taken = std::mem::replace(
                arg,
                TypedExpr {
                    kind: TypedExprKind::Error,
                    ty: Type::Error,
                    span: Span::new(0, 0),
                },
            );
            *arg = self.coerce(taken, *param);
        }
    }

    fn binary(
        &mut self,
        op: ast::BinaryOp,
        mut lhs: TypedExpr,
        mut rhs: TypedExpr,
    ) -> (TypedExprKind, Type) {
        use ast::BinaryOp as B;

        // An operand that already failed poisons the result silently.
        if lhs.ty.is_error() || rhs.ty.is_error() {
            return (TypedExprKind::Error, Type::Error);
        }

        let (typed_op, result) = match op {
            // `+` on two strings joins them. Every other arithmetic operator,
            // and every other type, stays f32-only.
            B::Add if lhs.ty == Type::Str && rhs.ty == Type::Str => {
                (BinaryOp::ConcatStr, Type::Str)
            }
            // Joining a string to a number is the mistake everyone makes on
            // their first debug line. There is no coercion in the language, so
            // this stays an error - but it says what to write instead rather
            // than reporting a bare type mismatch.
            B::Add if lhs.ty == Type::Str || rhs.ty == Type::Str => {
                let (number, span) = if lhs.ty == Type::Str {
                    (rhs.ty, rhs.span)
                } else {
                    (lhs.ty, lhs.span)
                };
                let hint = if number == Type::F32 {
                    " - use `str(...)` to turn a number into a String"
                } else {
                    ""
                };
                let name = number.name();
                let a = if name.starts_with(['f', 'a', 'e', 'i', 'o', 'u']) {
                    "an"
                } else {
                    "a"
                };
                self.error(span, format!("cannot join a String and {a} {name}{hint}"));
                return (TypedExprKind::Error, Type::Error);
            }
            // The additive core of Vec2 plus scaling, which is exactly enough
            // to make `pos += vel * dt` compile. Geometry - dot, length,
            // normalize, distance - is a small further step and is not here.
            B::Add if lhs.ty == Type::Vec2 && rhs.ty == Type::Vec2 => {
                (BinaryOp::AddVec2, Type::Vec2)
            }
            B::Sub if lhs.ty == Type::Vec2 && rhs.ty == Type::Vec2 => {
                (BinaryOp::SubVec2, Type::Vec2)
            }
            B::Mul if lhs.ty == Type::Vec2 && is_numeric(rhs.ty) => {
                rhs = self.coerce(rhs, Type::F32);
                (BinaryOp::MulVec2F32, Type::Vec2)
            }
            B::Mul if is_numeric(lhs.ty) && rhs.ty == Type::Vec2 => {
                lhs = self.coerce(lhs, Type::F32);
                (BinaryOp::MulF32Vec2, Type::Vec2)
            }
            B::Div if lhs.ty == Type::Vec2 && is_numeric(rhs.ty) => {
                rhs = self.coerce(rhs, Type::F32);
                (BinaryOp::DivVec2F32, Type::Vec2)
            }
            // Any other arithmetic involving a Vec2 is unsupported on purpose,
            // and says so rather than reporting "expected f32" once per side.
            B::Add | B::Sub | B::Mul | B::Div | B::Rem
                if lhs.ty == Type::Vec2 || rhs.ty == Type::Vec2 =>
            {
                let message = if op == B::Mul && lhs.ty == rhs.ty {
                    "`*` on two Vec2 values is not defined: it reads as a dot \
                     product to some people and as componentwise to others. \
                     Multiply by a number, or write the components you want"
                        .to_string()
                } else {
                    format!(
                        "cannot apply `{}` to {} and {} - a Vec2 supports `+`, `-`, \
                         unary `-`, and `*` or `/` by a number",
                        symbol(op),
                        lhs.ty.name(),
                        rhs.ty.name()
                    )
                };
                self.error(lhs.span.to(rhs.span), message);
                return (TypedExprKind::Error, Type::Error);
            }
            B::Add | B::Sub | B::Mul | B::Div | B::Rem => {
                if !is_numeric(lhs.ty) || !is_numeric(rhs.ty) {
                    self.expect_type(Type::F32, lhs.ty, lhs.span);
                    self.expect_type(Type::F32, rhs.ty, rhs.span);
                    return (TypedExprKind::Error, Type::Error);
                }
                let ty = self.unify_numeric(&mut lhs, &mut rhs);
                let op = match (op, ty) {
                    (B::Add, Type::Int) => BinaryOp::AddInt,
                    (B::Sub, Type::Int) => BinaryOp::SubInt,
                    (B::Mul, Type::Int) => BinaryOp::MulInt,
                    (B::Div, Type::Int) => BinaryOp::DivInt,
                    (B::Rem, Type::Int) => BinaryOp::RemInt,
                    (B::Add, _) => BinaryOp::AddF32,
                    (B::Sub, _) => BinaryOp::SubF32,
                    (B::Mul, _) => BinaryOp::MulF32,
                    (B::Div, _) => BinaryOp::DivF32,
                    (B::Rem, _) => BinaryOp::RemF32,
                    _ => unreachable!("matched on the arithmetic operators"),
                };
                (op, ty)
            }

            B::Lt | B::Gt | B::Le | B::Ge => {
                if !is_numeric(lhs.ty) || !is_numeric(rhs.ty) {
                    self.expect_type(Type::F32, lhs.ty, lhs.span);
                    self.expect_type(Type::F32, rhs.ty, rhs.span);
                    return (TypedExprKind::Error, Type::Error);
                }
                let op = match (op, self.unify_numeric(&mut lhs, &mut rhs)) {
                    (B::Lt, Type::Int) => BinaryOp::LtInt,
                    (B::Gt, Type::Int) => BinaryOp::GtInt,
                    (B::Le, Type::Int) => BinaryOp::LeInt,
                    (B::Ge, Type::Int) => BinaryOp::GeInt,
                    (B::Lt, _) => BinaryOp::LtF32,
                    (B::Gt, _) => BinaryOp::GtF32,
                    (B::Le, _) => BinaryOp::LeF32,
                    (B::Ge, _) => BinaryOp::GeF32,
                    _ => unreachable!("matched on the comparison operators"),
                };
                (op, Type::Bool)
            }

            B::Eq | B::NotEq => {
                // A mixed comparison widens, the same as arithmetic does.
                if is_numeric(lhs.ty) && is_numeric(rhs.ty) {
                    let ty = self.unify_numeric(&mut lhs, &mut rhs);
                    let op = if op == B::Eq {
                        BinaryOp::Eq(ty)
                    } else {
                        BinaryOp::NotEq(ty)
                    };
                    return (
                        TypedExprKind::Binary {
                            op,
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        },
                        Type::Bool,
                    );
                }
                if lhs.ty != rhs.ty {
                    self.expect_type(lhs.ty, rhs.ty, rhs.span);
                    return (TypedExprKind::Error, Type::Error);
                }
                // Comparing strings needs a host call or a memcmp helper, which
                // v1 does not emit; keep it an error rather than a silent
                // pointer comparison that would look like it worked.
                if !matches!(lhs.ty, Type::Bool) {
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
/// The names a suggestion may propose that are not in any table the checker
/// builds: the host functions, which live in [`builtin`] and `vec2`.
const BUILTIN_NAMES: &[&str] = &[
    "print", "str", "vec2", "abs", "sqrt", "floor", "ceil", "min", "max", "sin", "cos", "atan2",
    "pow", "int",
];

/// The candidate closest to `name`, if one is close enough to be worth saying.
///
/// "Close enough" is an edit distance of at most a third of the name's length,
/// with a floor of one - and nothing under three characters gets a guess at
/// all. Every one-letter name is one edit from every other, so `i` was cheerily
/// suggesting `f`. Suggesting something unrelated is worse than suggesting
/// nothing, because it reads as an answer.
fn nearest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let len = name.chars().count();
    if len < 3 {
        return None;
    }
    let budget = (len / 3).max(1);
    candidates
        .filter(|candidate| *candidate != name)
        .map(|candidate| (edit_distance(name, candidate), candidate))
        .filter(|(distance, _)| *distance <= budget)
        // Ties go to the alphabetically first, so the message does not depend on
        // the iteration order of a HashMap.
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, candidate)| candidate)
}

/// Levenshtein distance, one row at a time.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            let next = (row[j] + 1).min(row[j + 1] + 1).min(previous + cost);
            previous = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// A function the engine looks up by name, and the shape it must have.
struct Hook {
    name: &'static str,
    params: &'static [Type],
    ret: Type,
    /// The declaration as it should be written, for the diagnostic. Held rather
    /// than rendered from the types above because a parameter name is part of
    /// what a person needs to be shown and is not in the signature.
    written: &'static str,
}

/// The three names helios looks up. A name is listed here only once the engine
/// actually calls it - rejecting a signature nothing looks up would invent a
/// contract the engine does not have.
const HOOKS: &[Hook] = &[
    Hook {
        name: "update",
        params: &[Type::F32],
        ret: Type::Unit,
        written: "func update(dt: f32)",
    },
    Hook {
        name: "start",
        params: &[],
        ret: Type::Unit,
        written: "func start()",
    },
    Hook {
        name: "on_destroy",
        params: &[],
        ret: Type::Unit,
        written: "func on_destroy()",
    },
];

/// A `let` the checker is watching, so it can say when nothing ever read it.
struct Binding {
    name: String,
    span: Span,
    used: bool,
}

fn builtin(name: &str) -> Option<(Builtin, &'static [Type], Type)> {
    use Type::{F32, Str, Unit};
    Some(match name {
        "print" => (Builtin::Host(Host::Print), &[Str], Unit),
        "str" => (Builtin::Host(Host::Str), &[F32], Str),
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
    builtin(name).is_some() || name == "vec2" || name == "int"
}

/// Names the compiled module already uses for something. Every script function
/// is exported under its own name, and wasm export names have to be unique, so
/// a script calling a function `memory` would emit an invalid module. Reserving
/// the `comet_` prefix and `memory` up front turns that into a diagnostic with a
/// span, which is the only form of it a person can act on.
/// How an operator is written, for a diagnostic that has to name it.
///
fn symbol(op: ast::BinaryOp) -> &'static str {
    use ast::BinaryOp as B;
    match op {
        B::Add => "+",
        B::Sub => "-",
        B::Mul => "*",
        B::Div => "/",
        B::Rem => "%",
        B::Eq => "==",
        B::NotEq => "!=",
        B::Lt => "<",
        B::Gt => ">",
        B::Le => "<=",
        B::Ge => ">=",
        B::And => "&&",
        B::Or => "||",
    }
}

/// Whether a type takes part in arithmetic. The two numeric types are the
/// whole set, and mixing them widens.
fn is_numeric(ty: Type) -> bool {
    matches!(ty, Type::F32 | Type::Int)
}

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
    use crate::diagnostic::Severity;
    use crate::parser::parse;

    /// Check a source string, asserting it parsed cleanly first so a test about
    /// types never accidentally passes because of a syntax error.
    fn check_src(source: &str) -> (TypedScript, Vec<Diagnostic>) {
        let (script, parse_diagnostics) = parse(source);
        assert!(
            parse_diagnostics.is_empty(),
            "fixture should parse clean: {parse_diagnostics:?}"
        );
        check(&script, &crate::schema::example_schema())
    }

    /// The error messages a script produces. Warnings are advisory and have
    /// their own tests; a test about a type rule should not have to keep every
    /// fixture's bindings used to stay passing.
    fn messages(source: &str) -> Vec<String> {
        of_severity(source, Severity::Error)
    }

    fn warnings(source: &str) -> Vec<String> {
        of_severity(source, Severity::Warning)
    }

    fn of_severity(source: &str, severity: Severity) -> Vec<String> {
        check_src(source)
            .1
            .into_iter()
            .filter(|d| d.severity == severity)
            .map(|d| d.message)
            .collect()
    }

    fn check_clean(source: &str) -> TypedScript {
        let (typed, diagnostics) = check_src(source);
        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
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
        check_clean("func update(dt: f32) { transform.position.x += speed; }\nlet speed = 1.0;");
    }

    #[test]
    fn a_global_initializer_cannot_see_a_global_declared_after_it() {
        assert_eq!(
            messages("let a = b;\nlet b = 1.0;"),
            vec!["cannot find `b` in this scope"]
        );
    }

    // --- host properties ---

    #[test]
    fn a_host_property_has_the_type_the_schema_gave_it() {
        let typed = check_clean(
            "func update(dt: f32) { let p = transform.position; let x = transform.position.x; }",
        );
        let update = typed.function("update").unwrap();
        assert_eq!(update.locals[1], Type::Vec2);
        assert_eq!(update.locals[2], Type::F32);
    }

    #[test]
    fn assigning_one_axis_of_a_property_is_a_partial_write() {
        let typed = check_clean("func update(dt: f32) { transform.position.y = 3.0; }");
        let update = typed.function("update").unwrap();
        assert!(matches!(
            update.body.stmts[0],
            TypedStmt::Assign {
                place: Place::Host {
                    ty: Type::Vec2,
                    axis: Some(Axis::Y),
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn an_object_is_not_a_value_and_says_what_to_write_instead() {
        assert_eq!(
            messages("func update(dt: f32) { let p = transform; }"),
            ["`transform` is a group of properties, not a value - write `transform.position`"]
        );
    }

    #[test]
    fn an_unknown_property_suggests_the_nearest_one() {
        assert_eq!(
            messages("func update(dt: f32) { let r = transform.rotaton; }"),
            ["`transform` has no property `rotaton` - did you mean `rotation`?"]
        );
    }

    #[test]
    fn a_local_may_shadow_an_object_name_because_nothing_is_magic_now() {
        // `transform` is not a keyword. A local wins, which is ordinary lexical
        // scoping - and the reason a script can still call something `pos`.
        check_clean("func update(dt: f32) { let pos = 1.0; print(str(pos)); }");
    }

    #[test]
    fn vec2_has_only_x_and_y() {
        assert_eq!(
            messages("func update(dt: f32) { let z = transform.position.z; }"),
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
            "func update(dt: f32) { transform.position.x = later(dt); }\n\
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
             func update(dt: f32) { transform.position.x = f(dt); }",
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
        let (typed, _) = check(&script, &crate::schema::example_schema());
        let update = typed.function("update").expect("update still parsed");
        assert_eq!(update.locals[1], Type::F32, "`let speed` was still typed");
    }

    #[test]
    fn plus_joins_two_strings_but_nothing_else_does() {
        check_clean(r#"func f() -> String { "a" + "b" }"#);

        // Every other operator stays f32-only, and so does a mixed `+`. The
        // message names both sides so it is clear which one is the surprise.
        assert_eq!(
            messages(r#"func f() { let s: String = "a" + 1.0; }"#),
            ["cannot join a String and an f32 - use `str(...)` to turn a number into a String"]
        );
        assert_eq!(
            messages(r#"func f() { let s: String = 1.0 + "a"; }"#),
            ["cannot join a String and an f32 - use `str(...)` to turn a number into a String"]
        );
        // Only `+` gets the hint; the rest are plain f32 operators and say so.
        assert_eq!(
            messages(r#"func f() { let s: String = "a" - "b"; }"#),
            [
                "expected `f32`, found `String`",
                "expected `f32`, found `String`"
            ]
        );
    }

    #[test]
    fn str_turns_a_number_into_a_string() {
        check_clean(r#"func f(x: f32) -> String { "x: " + str(x) }"#);
        assert_eq!(
            messages(r#"func f() { let s: String = str("already"); }"#),
            ["expected `f32`, found `String`"]
        );
    }

    #[test]
    fn a_for_loop_is_the_while_loop_a_reader_would_have_written() {
        let typed = check_clean("func f() { for i in 0..3 { print(str(i)); } }");
        let body = &typed.functions[0].body.stmts;

        // let i = 0; let <end> = 3; while i < <end> { ...; i = i + 1 }, all int
        assert_eq!(body.len(), 3, "three statements, not one loop node");
        assert!(matches!(body[0], TypedStmt::Let { slot: 0, .. }));
        assert!(matches!(body[1], TypedStmt::Let { slot: 1, .. }));
        let TypedStmt::While { cond, body } = &body[2] else {
            panic!("expected a while");
        };
        assert!(matches!(
            cond.kind,
            TypedExprKind::Binary {
                op: BinaryOp::LtInt,
                ..
            }
        ));
        assert!(
            matches!(
                body.stmts.last(),
                Some(TypedStmt::Assign {
                    place: Place::Local(0),
                    ..
                })
            ),
            "the increment is the last thing in the body"
        );
    }

    #[test]
    fn the_loop_variable_leaves_scope_with_the_loop() {
        assert_eq!(
            messages("func f() { for i in 0..3 { } print(str(i)); }"),
            ["cannot find `i` in this scope"]
        );
        // And the hidden bound is not a name any script can reach, because no
        // source text lexes to it.
        check_clean("func f() { for i in 0..3 { } for i in 0..3 { } }");
    }

    #[test]
    fn for_bounds_must_be_whole_numbers() {
        assert_eq!(
            messages(r#"func f() { for i in "a"..3 { } }"#),
            ["expected `int`, found `String`"]
        );
        // An f32 bound is refused rather than truncated: `for i in 0.0..3.5`
        // has no obvious meaning, and the counter is an int.
        assert_eq!(
            messages("func f() { for i in 0..3.5 { } }"),
            ["expected `int`, found `f32`"]
        );
    }

    #[test]
    fn a_typo_suggests_the_name_it_almost_matched() {
        assert_eq!(
            messages("func f() { let speed = 1.0; let x = sped; }"),
            ["cannot find `sped` in this scope - did you mean `speed`?"]
        );
        // Builtins and functions are candidates too - a typo does not know what
        // kind of thing it was aiming at.
        assert_eq!(
            messages(r#"func f() { print(strr(1.0)); }"#),
            ["cannot find function `strr` - did you mean `str`?"]
        );
    }

    #[test]
    fn a_name_with_nothing_near_it_gets_no_guess() {
        assert_eq!(
            messages("func f() { let speed = 1.0; let x = wombat; }"),
            ["cannot find `wombat` in this scope"]
        );
        // Every one-letter name is one edit from every other, so short names
        // get no guess at all: a wrong suggestion reads as an answer.
        assert_eq!(
            messages("func f() { let a = 1.0; let b = q; }"),
            ["cannot find `q` in this scope"]
        );
    }

    #[test]
    fn a_binding_nothing_reads_is_a_warning_not_an_error() {
        let source = "func f() { let unused = 1.0; }";
        assert_eq!(
            warnings(source),
            ["`unused` is never used - prefix it with `_` if that is deliberate"]
        );
        assert!(messages(source).is_empty(), "the script still compiles");
    }

    #[test]
    fn writing_to_a_binding_is_not_reading_it() {
        // `x = 2.0` stores; nothing has looked at what is in there.
        assert_eq!(warnings("func f() { let x = 1.0; x = 2.0; }").len(), 1);
        // `x += 1.0` reads it first, so it counts.
        assert!(warnings("func f() { let x = 1.0; x += 1.0; }").is_empty());
        // And so does reading one field of it.
        assert!(warnings("func f() { let v = pos; let y = v.x; print(str(y)); }").is_empty());
    }

    #[test]
    fn the_things_that_should_not_warn_do_not() {
        // An unused parameter is normal - a handler takes `dt` whether or not
        // it needs it.
        assert!(warnings("func update(dt: f32) { }").is_empty());
        // A `for` that repeats something N times need not name its counter.
        assert!(warnings(r#"func f() { for i in 0.0..3.0 { print("x"); } }"#).is_empty());
        // And an underscore says "I know".
        assert!(warnings("func f() { let _spare = 1.0; }").is_empty());
    }

    #[test]
    fn an_update_with_the_wrong_signature_is_reported_rather_than_ignored() {
        // The host looks `update` up at one signature and silently gets nothing
        // back otherwise, so without this the script compiles, never runs, and
        // says nothing about why.
        let expected =
            ["`update` is called by the engine and must be written `func update(dt: f32)`"];
        assert_eq!(messages("func update() { }"), expected, "no parameter");
        assert_eq!(
            messages("func update(dt: f32) -> f32 { 1.0 }"),
            expected,
            "returns something"
        );
        assert_eq!(
            messages("func update(dt: Vec2) { }"),
            expected,
            "wrong parameter type"
        );
        assert_eq!(
            messages("func update(a: f32, b: f32) { }"),
            expected,
            "too many parameters"
        );
    }

    #[test]
    fn the_other_two_hooks_are_checked_the_same_way() {
        check_clean("func start() { }");
        check_clean("func on_destroy() { }");
        assert_eq!(
            messages("func start(dt: f32) { }"),
            ["`start` is called by the engine and must be written `func start()`"]
        );
        assert_eq!(
            messages("func on_destroy() -> f32 { 1.0 }"),
            ["`on_destroy` is called by the engine and must be written `func on_destroy()`"]
        );
    }

    #[test]
    fn the_parameter_name_of_update_is_the_authors_to_choose() {
        check_clean("func update(elapsed: f32) { }");
        // And a function that is not a hook can have any shape at all.
        check_clean("func tick() -> Vec2 { transform.position }");
    }

    #[test]
    fn an_update_whose_type_did_not_resolve_reports_only_that() {
        // One mistake, one diagnostic: the unresolved type is the thing to fix,
        // and adding "must be written func update(dt: f32)" on top of it would
        // be a second complaint about the same character.
        assert_eq!(
            messages("func update(dt: Vector) { }"),
            ["unknown type `Vector`"]
        );
    }

    #[test]
    fn how_a_literal_is_written_decides_its_type() {
        let typed = check_clean("func f() { let whole = 5; let fraction = 5.0; }");
        let f = typed.function("f").unwrap();
        assert_eq!(f.locals[0], Type::Int);
        assert_eq!(f.locals[1], Type::F32);
    }

    #[test]
    fn an_int_widens_to_f32_but_never_the_other_way() {
        // One-directional, so there is no precision-loss surprise and every
        // script written before `int` existed keeps compiling.
        check_clean("func f() { let x: f32 = 5; }");
        check_clean("func f(a: f32) { let x = a + 1; }");
        check_clean("func f() -> f32 { 1 }");
        assert_eq!(
            messages("func f() { let x: int = 5.0; }"),
            ["expected `int`, found `f32`"]
        );
        assert_eq!(
            messages("func f() -> int { 1.0 }"),
            ["expected `int`, found `f32`"]
        );
    }

    #[test]
    fn a_mixed_expression_is_an_f32_and_two_ints_stay_an_int() {
        let typed = check_clean("func f() { let a = 1 + 2; let b = 1 + 2.0; let c = 1 < 2; }");
        let f = typed.function("f").unwrap();
        assert_eq!(f.locals[0], Type::Int);
        assert_eq!(f.locals[1], Type::F32);
        assert_eq!(f.locals[2], Type::Bool);
    }

    #[test]
    fn the_widening_reaches_codegen_rather_than_merely_being_permitted() {
        // A rule the checker allows but does not record would emit an i32 where
        // an f32 belongs, and the module would not validate.
        let typed = check_clean("func f(a: f32) -> f32 { a + 1 }");
        let tail = typed.function("f").unwrap().body.tail.as_ref().unwrap();
        let TypedExprKind::Binary { op, rhs, .. } = &tail.kind else {
            panic!("expected a binary expression");
        };
        assert_eq!(*op, BinaryOp::AddF32);
        assert!(matches!(rhs.kind, TypedExprKind::Widen(_)));
    }

    #[test]
    fn narrowing_needs_asking_for() {
        check_clean("func f() { let x = int(3.7); }");
        let typed = check_clean("func f() -> int { int(3.7) }");
        assert_eq!(typed.function("f").unwrap().ret, Type::Int);
        // And `int` is the engine's, not a name to redefine.
        assert_eq!(
            messages("func int(x: f32) -> f32 { x }"),
            ["`int` is provided by the engine and cannot be redefined"]
        );
    }

    #[test]
    fn a_literal_too_large_for_an_int_is_reported_rather_than_wrapped() {
        assert_eq!(
            messages("func f() { let x = 3000000000; }"),
            ["`3000000000` does not fit in an int"]
        );
    }

    #[test]
    fn vec2_scales_by_a_whole_number_too() {
        check_clean("func f() { let v = vec2(1.0, 2.0) * 2; }");
        check_clean("func f() { let v = 2 * vec2(1.0, 2.0); }");
        check_clean("func f() { let v = vec2(1.0, 2.0) / 2; }");
    }

    // --- exported variables ---

    #[test]
    fn an_exported_variable_may_carry_only_a_type() {
        // What a well-written script looks like: the inspector owns the value,
        // so there is no number in the source to be a second answer.
        let typed = check_clean("@export let speed: f32;");
        assert!(typed.state[0].exported);
        assert_eq!(typed.state[0].ty, Type::F32);
        // And it starts at the type's default rather than at nothing.
        assert!(matches!(typed.state[0].init.kind, TypedExprKind::Number(v) if v == 0.0));
    }

    #[test]
    fn every_type_that_can_be_exported_has_a_default() {
        for (source, want) in [
            ("@export let a: f32;", Type::F32),
            ("@export let a: int;", Type::Int),
            ("@export let a: bool;", Type::Bool),
            ("@export let a: Vec2;", Type::Vec2),
        ] {
            let typed = check_clean(source);
            assert_eq!(typed.state[0].ty, want);
            assert!(!matches!(typed.state[0].init.kind, TypedExprKind::Error));
        }
    }

    #[test]
    fn an_exported_variable_with_an_initializer_warns_but_still_compiles() {
        let warnings = warnings("@export let speed: f32 = 120.0;");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("the inspector owns its value"));
        assert!(messages("@export let speed: f32 = 120.0;").is_empty());
    }

    #[test]
    fn a_declaration_with_neither_a_type_nor_a_value_says_so() {
        assert_eq!(
            messages("let speed;"),
            [
                "`speed` needs a type or a value - with neither there is nothing to work out what it holds"
            ]
        );
    }

    #[test]
    fn only_what_can_be_stored_on_the_component_can_be_exported() {
        // A String is a pointer into the module's own memory, so handing one
        // across needs an ownership rule that is its own decision.
        assert_eq!(
            messages(r#"@export let name: String = "hi";"#)
                .into_iter()
                .filter(|m| m.contains("exported"))
                .collect::<Vec<_>>(),
            [
                "a `String` cannot be exported yet - only f32, int, bool, Vec2 and payload-free enums can be stored on the component"
            ]
        );
    }

    #[test]
    fn an_unknown_annotation_is_reported_rather_than_ignored() {
        // A typo that silently does nothing is the wrong failure mode, and is
        // the reason annotations are grammar rather than a comment convention.
        assert_eq!(
            messages("@exprot let speed: f32;"),
            ["unknown annotation `@exprot` - did you mean `@export`?"]
        );
        assert_eq!(
            messages("@export(1) let speed: f32;"),
            ["`@export` takes no arguments"]
        );
    }

    // --- enums and match ---

    #[test]
    fn a_match_must_cover_every_variant() {
        // The whole point of the construct: a compiler that tells a learner
        // they forgot a case. There is no wildcard, so it always can.
        assert_eq!(
            messages(
                "enum S { Idle, Walking, Falling }\n\
                 func f(s: S) -> f32 { match s { Idle => 0.0 } }"
            ),
            [
                "this `match` does not cover `Walking`, `Falling` - every variant needs an arm, and there is no wildcard"
            ]
        );
    }

    #[test]
    fn every_arm_of_a_match_produces_the_same_type() {
        assert_eq!(
            messages("enum S { A, B }\nfunc f(s: S) -> f32 { match s { A => 1.0, B => true } }"),
            [
                "every arm of a `match` has to produce the same type - this one is bool, and the first is f32"
            ]
        );
    }

    #[test]
    fn a_pattern_names_exactly_what_its_variant_carries() {
        assert_eq!(
            messages("enum H { Wall(f32) }\nfunc f(h: H) -> f32 { match h { Wall => 1.0 } }"),
            // The bad arm does not count as covering its variant, so the
            // coverage error follows - which is the honest reading.
            [
                "this `match` does not cover `Wall` - every variant needs an arm, and there is no wildcard",
                "`Wall` carries 1 value, but this pattern names 0"
            ]
        );
        assert_eq!(
            messages("enum H { Wall(f32) }\nfunc f() -> H { H::Wall(1.0, 2.0) }"),
            ["`Wall` carries 1 value, but 2 were given"]
        );
    }

    #[test]
    fn an_unknown_variant_suggests_the_nearest_one() {
        assert_eq!(
            messages("enum S { Idle, Walking }\nfunc f() -> S { S::Wlaking }"),
            ["`S` has no variant `Wlaking` - did you mean `Walking`?"]
        );
        assert_eq!(
            messages("enum State { Idle }\nfunc f() -> State { Stat::Idle }"),
            ["cannot find enum `Stat` - did you mean `State`?"]
        );
    }

    #[test]
    fn a_repeated_arm_is_reported_and_the_rest_still_checked() {
        assert_eq!(
            messages("enum S { A, B }\nfunc f(s: S) -> f32 { match s { A => 1.0, A => 2.0 } }"),
            // Sorted by span, and the `match` starts before its second arm.
            [
                "this `match` does not cover `B` - every variant needs an arm, and there is no wildcard",
                "`A` already has an arm"
            ]
        );
    }

    #[test]
    fn a_payload_binding_has_the_type_the_variant_declared() {
        check_clean(
            "enum H { Miss, At(Vec2) }\n\
             func f(h: H) -> f32 { match h { Miss => 0.0, At(p) => p.x } }",
        );
        assert_eq!(
            messages(
                "enum H { Miss, At(Vec2) }\n\
                 func f(h: H) -> f32 { match h { Miss => 0.0, At(p) => p } }"
            ),
            [
                "every arm of a `match` has to produce the same type - this one is Vec2, and the first is f32"
            ]
        );
    }

    #[test]
    fn matching_on_something_that_is_not_an_enum_says_so() {
        assert_eq!(
            messages("func f(x: f32) -> f32 { match x { A => 1.0 } }"),
            ["`match` works on an enum, and this is a f32"]
        );
    }

    #[test]
    fn an_exported_enum_defaults_to_its_first_variant() {
        let typed = check_clean("enum S { Idle, Walking }\n@export let state: S;");
        assert!(matches!(
            typed.state[0].init.kind,
            TypedExprKind::MakeVariant { variant: 0, .. }
        ));

        // Unless that variant carries something, in which case there is nothing
        // to fill the payload with and the author has to say.
        assert_eq!(
            messages("enum S { Wall(f32), Idle }\n@export let state: S;")
                .into_iter()
                .filter(|m| m.contains("first variant"))
                .count(),
            1
        );
    }

    #[test]
    fn an_enum_needs_a_name_of_its_own_and_variants_of_their_own() {
        assert_eq!(
            messages("enum S { A }\nenum S { B }"),
            ["`S` is already defined in this script"]
        );
        assert_eq!(
            messages("enum S { A, A }"),
            ["`A` is already a variant of this enum"]
        );
        assert_eq!(
            messages("enum S { }"),
            ["an enum needs at least one variant"]
        );
    }
}
