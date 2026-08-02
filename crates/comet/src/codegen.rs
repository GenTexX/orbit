//! comet codegen: the typed IR to a WebAssembly module.
//!
//! There is no middle-end (ADR 0007). This walks the tree the checker produced
//! and writes instructions, once, in one pass. Every decision it could have to
//! make - what type a value is, which slot a name lives in, whether `+` is float
//! addition - was already made in [`check`](crate::check), so this file is
//! mechanical on purpose: compile time is on the interactive path, and Cranelift
//! does the optimizing once at module load.
//!
//! # Memory
//!
//! The module owns and exports one linear memory laid out as
//!
//! ```text
//! 0 .. 16      reserved and zeroed, so a null String reads a length of 0
//! 16 .. heap   string literals, written by the module's data segment
//! heap ..      the heap, handed out by comet_alloc
//! ```
//!
//! Every heap block carries a 12-byte header - `[size][refcount][len]` - and a
//! `String` value is a pointer to that header, with its bytes at `ptr + 12`.
//! A refcount of `0` means immortal: string literals live in the data segment
//! and must never be freed, so retain and release both leave them alone.
//!
//! # Reference counting
//!
//! One rule, applied everywhere: **an expression of type `String` evaluates to
//! an owned reference**, and whoever consumes it releases it. Reading a variable
//! retains; storing into a place releases what was there before (after the new
//! value exists, so `s = s` is safe); a discarded value is released; a function
//! releases every `String` slot it owns on the way out, including its
//! parameters, which the caller handed over. Uniform beats clever: there is no
//! liveness analysis to get subtly wrong, just a retain at every read and a
//! release at every consumer.
//!
//! v1 has no operation that *produces* a new string - there is no concatenation
//! and no number formatting yet - so every `String` in a script today is a
//! literal, and the allocator, while real and exported, is not yet on any path a
//! script can reach. It is emitted and tested now because the first heap type
//! that can be constructed (concatenation, or a user `struct`) should find a
//! working allocator rather than have to bring one.

use std::collections::HashMap;

use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportKind, ExportSection,
    Function, FunctionSection, GlobalSection, GlobalType, Ieee32, ImportSection, InstructionSink,
    MemArg, MemorySection, MemoryType, Module, NameMap, NameSection, StartSection, TypeSection,
    ValType,
};

use crate::tir::{
    ArrayOp, Axis, BinaryOp, Host, Place, Type, TypedArm, TypedBlock, TypedEnum, TypedExpr,
    TypedExprKind, TypedFn, TypedScript, TypedStmt, TypedStruct, UnaryOp,
};

/// The module the host must satisfy every import from.
pub const HOST_MODULE: &str = "orbit";

/// Bytes of header on every heap block: `[size][refcount][len]`.
const HEADER: i32 = 12;

/// Fill in the `String` block at `ptr`, which the host has just obtained from
/// the module's `comet_alloc` with a size of `text.len()`.
///
/// The block layout is codegen's business rather than the host's, so this lives
/// here, over plain bytes, and every host uses the same one instead of
/// hardcoding offsets that only this file gets to choose.
///
/// Panics if the block does not fit, which would mean `comet_alloc` returned a
/// pointer for a smaller size than the one it was asked for.
pub fn write_str(memory: &mut [u8], ptr: i32, text: &str) {
    let base = ptr as usize;
    let len_at = base + OFF_LEN as usize;
    memory[len_at..len_at + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
    let start = base + HEADER as usize;
    memory[start..start + text.len()].copy_from_slice(text.as_bytes());
}

/// How comet renders an f32 as text: what `str(x)` returns, and the one place
/// that decides whether `10.0 / 2.0` prints as "5" or "5.0".
///
/// Shortest-round-trip, which is Rust's `Display` for f32 - so a value that came
/// from a literal prints the way it was written, and a whole number prints
/// without a decimal point.
pub fn format_f32(value: f32) -> String {
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    if value.is_nan() {
        return "nan".to_string();
    }
    format!("{value}")
}
const OFF_SIZE: u64 = 0;
const OFF_RC: u64 = 4;
const OFF_LEN: u64 = 8;

/// An array's handle, in the payload of an ordinary refcounted block:
/// how many elements it holds, how many it has room for, and where they are.
///
/// Two blocks rather than one, because growing must be invisible to every other
/// name for the same array. Elements inline would mean `push` moving the block,
/// and an alias would still be pointing at the old one - which is exactly what
/// reference semantics promise will not happen.
const OFF_ARRAY_LEN: u64 = 12;
const OFF_ARRAY_CAP: u64 = 16;
const OFF_ARRAY_DATA: u64 = 20;
/// How big an array handle's payload is.
const ARRAY_HANDLE: i32 = 12;

/// The first 16 bytes are reserved and left zero, so a null `String` pointer
/// reads a length of 0 instead of the first literal's bytes.
const DATA_BASE: u32 = 16;
const PAGE: u32 = 65536;

// Function indices. Imports come first in wasm's index space, then everything
// the module defines, so these are fixed for every module comet emits.
/// The host property accessors. Every one takes the property's schema id, so
/// the import list stays fixed no matter how many properties the engine
/// exposes - a host that can run one comet module can run all of them, and
/// there is one binding table to write rather than one per script.
///
/// A `Vec2` is read a component at a time and written whole, which is what the
/// dedicated position accessors did before the schema generalized them.
const F_GET_F32: u32 = 0;
const F_SET_F32: u32 = 1;
const F_GET_BOOL: u32 = 2;
const F_SET_BOOL: u32 = 3;
const F_GET_VEC2_X: u32 = 4;
const F_GET_VEC2_Y: u32 = 5;
const F_SET_VEC2: u32 = 6;
const F_PRINT: u32 = 7;
/// `str(f32) -> String`. The host formats and allocates: float-to-decimal is a
/// page of code nobody should emit into every module.
const F_STR: u32 = 8;
const F_STR_INT: u32 = 9;
/// The transcendentals. WebAssembly has no opcodes for these, so they are the
/// only maths that leaves the module; abs, sqrt, floor, ceil, min and max are
/// one instruction each and are emitted inline.
const F_SIN: u32 = 10;
const F_COS: u32 = 11;
const F_ATAN2: u32 = 12;
const F_POW: u32 = 13;
const F_ALLOC: u32 = 14;
const F_RETAIN: u32 = 15;
const F_RELEASE: u32 = 16;
/// The array runtime. Two blocks per array - a handle and its elements - so
/// growing is invisible to every other name for the same array.
const F_ARRAY_NEW: u32 = 17;
const F_ARRAY_AT: u32 = 18;
const F_ARRAY_PUSH: u32 = 19;
const F_ARRAY_RELEASE: u32 = 20;
const F_ARRAY_COPY: u32 = 21;
/// The first script-defined function's index.
const USER_BASE: u32 = 22;

/// Emit a WebAssembly module for `script`.
///
/// The tree must have checked clean - [`compile`](crate::compile) is the entry
/// point that guarantees it. A tree that still contains [`Type::Error`] nodes
/// emits `unreachable` where the error was rather than panicking, so a caller
/// that ignores the diagnostics gets a module that traps instead of one that
/// silently computes something wrong.
pub fn emit(script: &TypedScript) -> Vec<u8> {
    let mut literals = Literals::default();
    collect_literals(script, &mut literals);

    let globals = Globals::new(script);
    let heap_base = align4(DATA_BASE + literals.data.len() as u32);
    let pages = (heap_base.div_ceil(PAGE)).max(1);

    let mut types = Types::default();
    // The host ABI, in import order.
    let t_get_f32 = types.get(vec![ValType::I32], vec![ValType::F32]);
    let t_set_f32 = types.get(vec![ValType::I32, ValType::F32], vec![]);
    let t_get_bool = types.get(vec![ValType::I32], vec![ValType::I32]);
    let t_set_bool = types.get(vec![ValType::I32, ValType::I32], vec![]);
    let t_set_vec2 = types.get(vec![ValType::I32, ValType::F32, ValType::F32], vec![]);
    let t_print = types.get(vec![ValType::I32, ValType::I32], vec![]);
    let t_str = types.get(vec![ValType::F32], vec![ValType::I32]);
    let t_unary = types.get(vec![ValType::F32], vec![ValType::F32]);
    let t_binary = types.get(vec![ValType::F32, ValType::F32], vec![ValType::F32]);
    let t_alloc = types.get(vec![ValType::I32], vec![ValType::I32]);
    let t_rc = types.get(vec![ValType::I32], vec![]);
    let t_init = types.get(vec![], vec![]);

    let mut imports = ImportSection::new();
    imports.import(HOST_MODULE, "get_f32", EntityType::Function(t_get_f32));
    imports.import(HOST_MODULE, "set_f32", EntityType::Function(t_set_f32));
    imports.import(HOST_MODULE, "get_bool", EntityType::Function(t_get_bool));
    imports.import(HOST_MODULE, "set_bool", EntityType::Function(t_set_bool));
    imports.import(HOST_MODULE, "get_vec2_x", EntityType::Function(t_get_f32));
    imports.import(HOST_MODULE, "get_vec2_y", EntityType::Function(t_get_f32));
    imports.import(HOST_MODULE, "set_vec2", EntityType::Function(t_set_vec2));
    imports.import(HOST_MODULE, "print", EntityType::Function(t_print));
    imports.import(HOST_MODULE, "str", EntityType::Function(t_str));
    imports.import(HOST_MODULE, "str_int", EntityType::Function(t_get_bool));
    imports.import(HOST_MODULE, "sin", EntityType::Function(t_unary));
    imports.import(HOST_MODULE, "cos", EntityType::Function(t_unary));
    imports.import(HOST_MODULE, "atan2", EntityType::Function(t_binary));
    imports.import(HOST_MODULE, "pow", EntityType::Function(t_binary));

    let t_array_new = types.get(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let t_array_at = types.get(
        vec![ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I32],
    );

    let mut functions = FunctionSection::new();
    functions.function(t_alloc);
    functions.function(t_rc);
    functions.function(t_rc);
    functions.function(t_array_new);
    functions.function(t_array_at);
    functions.function(t_array_new);
    functions.function(t_rc);
    functions.function(t_array_new);
    for f in &script.functions {
        let ty = types.get(
            param_types(f, &script.enums, &script.structs),
            val_types(f.ret, &script.enums, &script.structs),
        );
        functions.function(ty);
    }
    let has_state = !script.state.is_empty();
    if has_state {
        functions.function(t_init);
    }

    let mut code = CodeSection::new();
    code.function(&emit_alloc(&globals));
    code.function(&emit_retain());
    code.function(&emit_release(&globals));
    code.function(&emit_array_new());
    code.function(&emit_array_at());
    code.function(&emit_array_push());
    code.function(&emit_array_release());
    code.function(&emit_array_copy());
    for f in &script.functions {
        code.function(&emit_function(f, layouts(script), &globals, &literals));
    }
    if has_state {
        code.function(&emit_init(script, &globals, &literals));
    }

    let mut global_section = GlobalSection::new();
    for state in &script.state {
        for vt in val_types(state.ty, &script.enums, &script.structs) {
            global_section.global(mutable(vt), &zero_of(vt));
        }
    }
    global_section.global(
        mutable(ValType::I32),
        &ConstExpr::i32_const(heap_base as i32),
    );
    global_section.global(mutable(ValType::I32), &ConstExpr::i32_const(0));

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: pages as u64,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });

    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("comet_alloc", ExportKind::Func, F_ALLOC);
    exports.export("comet_retain", ExportKind::Func, F_RETAIN);
    exports.export("comet_release", ExportKind::Func, F_RELEASE);
    // An exported variable's globals, so the host can write the inspector's
    // stored value in after instantiation and read it back out. A Vec2 takes
    // two, which is why the naming is derived rather than assumed - see
    // `exported_globals`.
    for state in &script.state {
        if !state.exported {
            continue;
        }
        let base = globals.base[state.slot as usize];
        for (offset, name) in exported_globals(&state.name, state.ty)
            .into_iter()
            .enumerate()
        {
            exports.export(&name, ExportKind::Global, base + offset as u32);
        }
    }
    for (i, f) in script.functions.iter().enumerate() {
        exports.export(&f.name, ExportKind::Func, USER_BASE + i as u32);
    }

    let mut module = Module::new();
    module.section(&types.section());
    module.section(&imports);
    module.section(&functions);
    module.section(&memories);
    module.section(&global_section);
    module.section(&exports);
    if has_state {
        // State initializers run at instantiation, so no exported function can
        // observe an uninitialized global.
        module.section(&StartSection {
            function_index: USER_BASE + script.functions.len() as u32,
        });
    }
    module.section(&code);
    // A name section, so a trap's backtrace reads as `update` rather than as
    // `wasm-function[7]`. It costs a few dozen bytes and is the difference
    // between a runtime error naming something and naming nothing.
    let mut names = NameMap::new();
    names.append(F_GET_F32, "orbit::get_f32");
    names.append(F_SET_F32, "orbit::set_f32");
    names.append(F_GET_BOOL, "orbit::get_bool");
    names.append(F_SET_BOOL, "orbit::set_bool");
    names.append(F_GET_VEC2_X, "orbit::get_vec2_x");
    names.append(F_GET_VEC2_Y, "orbit::get_vec2_y");
    names.append(F_SET_VEC2, "orbit::set_vec2");
    names.append(F_PRINT, "orbit::print");
    names.append(F_STR, "orbit::str");
    names.append(F_STR_INT, "orbit::str_int");
    names.append(F_SIN, "orbit::sin");
    names.append(F_COS, "orbit::cos");
    names.append(F_ATAN2, "orbit::atan2");
    names.append(F_POW, "orbit::pow");
    names.append(F_ARRAY_NEW, "comet_array_new");
    names.append(F_ARRAY_AT, "comet_array_at");
    names.append(F_ARRAY_PUSH, "comet_array_push");
    names.append(F_ARRAY_RELEASE, "comet_array_release");
    names.append(F_ARRAY_COPY, "comet_array_copy");
    names.append(F_ALLOC, "comet_alloc");
    names.append(F_RETAIN, "comet_retain");
    names.append(F_RELEASE, "comet_release");
    for (i, f) in script.functions.iter().enumerate() {
        names.append(USER_BASE + i as u32, &f.name);
    }
    if has_state {
        names.append(USER_BASE + script.functions.len() as u32, "<script state>");
    }
    let mut name_section = NameSection::new();
    name_section.functions(&names);
    module.section(&name_section);
    if !literals.data.is_empty() {
        let mut data = DataSection::new();
        data.active(
            0,
            &ConstExpr::i32_const(DATA_BASE as i32),
            literals.data.iter().copied(),
        );
        module.section(&data);
    }
    module.finish()
}

// --- module-level layout ---

fn align4(value: u32) -> u32 {
    value.div_ceil(4) * 4
}

/// How a comet type is represented on the wasm stack. `Vec2` is two f32s - it is
/// a value type, so it never touches the heap; `String` is a pointer.
/// The names a module exports the globals of `name` under.
///
/// One for a scalar, two for a `Vec2`. The host never builds these itself: it
/// asks, so the convention has one definition rather than two that agree until
/// somebody changes one.
pub fn exported_globals(name: &str, ty: Type) -> Vec<String> {
    match ty {
        Type::Vec2 => vec![format!("state.{name}.x"), format!("state.{name}.y")],
        _ => vec![format!("state.{name}")],
    }
}

/// How a value of `ty` is laid out in wasm slots.
///
/// An enum is a tag followed by the widest variant's payload, and every payload
/// slot is an `i32` - an `f32` payload is stored reinterpreted, which is one
/// free instruction each way. A uniform slot type is what lets one layout serve
/// every variant, and keeps an enum a stack value rather than an allocation.
fn val_types(ty: Type, enums: &[TypedEnum], structs: &[TypedStruct]) -> Vec<ValType> {
    match ty {
        Type::Enum(index) => vec![ValType::I32; 1 + enums[index as usize].payload_slots],
        // A struct is exactly its fields, one after another, keeping their own
        // types - unlike an enum, where one layout has to serve every variant.
        Type::Struct(index) => structs[index as usize]
            .fields
            .iter()
            .flat_map(|f| val_types(f.ty, enums, structs))
            .collect(),
        _ => scalar_types(ty).to_vec(),
    }
}

fn scalar_types(ty: Type) -> &'static [ValType] {
    match ty {
        Type::F32 => &[ValType::F32],
        Type::Int => &[ValType::I32],
        Type::Bool => &[ValType::I32],
        Type::Vec2 => &[ValType::F32, ValType::F32],
        Type::Str => &[ValType::I32],
        Type::Unit | Type::Error => &[],
        // A reference: one slot holding the handle's address.
        Type::Array(_) => &[ValType::I32],
        Type::Enum(_) | Type::Struct(_) => {
            unreachable!("handled by val_types, which knows the layouts")
        }
    }
}

fn param_types(f: &TypedFn, enums: &[TypedEnum], structs: &[TypedStruct]) -> Vec<ValType> {
    f.locals[..f.param_count]
        .iter()
        .flat_map(|ty| val_types(*ty, enums, structs))
        .collect()
}

fn mutable(ty: ValType) -> GlobalType {
    GlobalType {
        val_type: ty,
        mutable: true,
        shared: false,
    }
}

fn zero_of(ty: ValType) -> ConstExpr {
    match ty {
        ValType::F32 => ConstExpr::f32_const(Ieee32::from(0.0f32)),
        _ => ConstExpr::i32_const(0),
    }
}

fn mem(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 2,
        memory_index: 0,
    }
}

#[derive(Default)]
struct Types {
    entries: Vec<(Vec<ValType>, Vec<ValType>)>,
    index: HashMap<(Vec<ValType>, Vec<ValType>), u32>,
}

impl Types {
    fn get(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        let key = (params, results);
        if let Some(&i) = self.index.get(&key) {
            return i;
        }
        let i = self.entries.len() as u32;
        self.entries.push(key.clone());
        self.index.insert(key, i);
        i
    }

    fn section(&self) -> TypeSection {
        let mut section = TypeSection::new();
        for (params, results) in &self.entries {
            section
                .ty()
                .function(params.iter().copied(), results.iter().copied());
        }
        section
    }
}

/// Where each piece of script state lives in the module's globals.
struct Globals {
    /// State slot -> its first wasm global index.
    base: Vec<u32>,
    types: Vec<Type>,
    /// The bump pointer: the next unused heap address.
    heap_next: u32,
    /// Head of the free list, or 0 when it is empty.
    free_head: u32,
}

impl Globals {
    fn new(script: &TypedScript) -> Self {
        let (enums, structs) = (&script.enums, &script.structs);
        let mut base = Vec::new();
        let mut types = Vec::new();
        let mut next = 0;
        for state in &script.state {
            base.push(next);
            types.push(state.ty);
            next += val_types(state.ty, enums, structs).len() as u32;
        }
        Self {
            base,
            types,
            heap_next: next,
            free_head: next + 1,
        }
    }
}

#[derive(Default)]
struct Literals {
    offsets: HashMap<String, u32>,
    data: Vec<u8>,
}

impl Literals {
    /// Place `value` in the data segment as an immortal heap block, or reuse the
    /// block an identical literal already got.
    fn intern(&mut self, value: &str) -> u32 {
        if let Some(&offset) = self.offsets.get(value) {
            return offset;
        }
        let offset = DATA_BASE + self.data.len() as u32;
        let len = value.len() as i32;
        let size = align4((HEADER + len) as u32) as i32;
        self.data.extend_from_slice(&size.to_le_bytes());
        self.data.extend_from_slice(&0i32.to_le_bytes()); // refcount 0: immortal
        self.data.extend_from_slice(&len.to_le_bytes());
        self.data.extend_from_slice(value.as_bytes());
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        self.offsets.insert(value.to_string(), offset);
        offset
    }

    fn offset(&self, value: &str) -> u32 {
        *self
            .offsets
            .get(value)
            .expect("every literal was interned before emission")
    }
}

fn collect_literals(script: &TypedScript, out: &mut Literals) {
    for state in &script.state {
        collect_expr(&state.init, out);
    }
    for f in &script.functions {
        collect_block(&f.body, out);
    }
}

fn collect_block(block: &TypedBlock, out: &mut Literals) {
    for stmt in &block.stmts {
        match stmt {
            TypedStmt::Let { init, .. } => collect_expr(init, out),
            TypedStmt::Assign { place, value } => {
                // A place can hold expressions too: `a[i] = v` has both.
                if let Place::Element { array, index, .. } = place {
                    collect_expr(array, out);
                    collect_expr(index, out);
                }
                collect_expr(value, out);
            }
            TypedStmt::If {
                cond,
                then,
                otherwise,
            } => {
                collect_expr(cond, out);
                collect_block(then, out);
                if let Some(otherwise) = otherwise {
                    collect_block(otherwise, out);
                }
            }
            TypedStmt::While { cond, body } => {
                collect_expr(cond, out);
                collect_block(body, out);
            }
            TypedStmt::Return { value } => {
                if let Some(value) = value {
                    collect_expr(value, out);
                }
            }
            TypedStmt::Expr(expr) => collect_expr(expr, out),
        }
    }
    if let Some(tail) = &block.tail {
        collect_expr(tail, out);
    }
}

fn collect_expr(expr: &TypedExpr, out: &mut Literals) {
    match &expr.kind {
        TypedExprKind::Str(value) => {
            out.intern(value);
        }
        TypedExprKind::Field { receiver, .. } => collect_expr(receiver, out),
        TypedExprKind::Call { args, .. } | TypedExprKind::HostCall { args, .. } => {
            for arg in args {
                collect_expr(arg, out);
            }
        }
        TypedExprKind::Unary { operand, .. } => collect_expr(operand, out),
        TypedExprKind::Binary { lhs, rhs, .. } => {
            collect_expr(lhs, out);
            collect_expr(rhs, out);
        }
        TypedExprKind::MakeVec2 { x, y } => {
            collect_expr(x, out);
            collect_expr(y, out);
        }
        TypedExprKind::Widen(inner) | TypedExprKind::Narrow(inner) => collect_expr(inner, out),
        TypedExprKind::MakeVariant { args, .. } => {
            for arg in args {
                collect_expr(arg, out);
            }
        }
        TypedExprKind::MakeStruct { fields, .. } => {
            for field in fields {
                collect_expr(field, out);
            }
        }
        TypedExprKind::MakeArray { elements, .. } => {
            for element in elements {
                collect_expr(element, out);
            }
        }
        TypedExprKind::Index { array, index, .. } => {
            collect_expr(array, out);
            collect_expr(index, out);
        }
        TypedExprKind::ArrayGet { array, index, .. } => {
            collect_expr(array, out);
            collect_expr(index, out);
        }
        TypedExprKind::ArrayOp { array, value, .. } => {
            collect_expr(array, out);
            if let Some(value) = value {
                collect_expr(value, out);
            }
        }
        TypedExprKind::Match {
            scrutinee, arms, ..
        } => {
            collect_expr(scrutinee, out);
            for arm in arms {
                collect_expr(&arm.body, out);
            }
        }
        // Listed rather than caught by a wildcard: a new kind that holds a
        // string must fail to compile here rather than panic at emission with
        // "every literal was interned", which is how this was found.
        TypedExprKind::Number(_)
        | TypedExprKind::Int(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Local(_)
        | TypedExprKind::Global(_)
        | TypedExprKind::HostField { .. }
        | TypedExprKind::Error => {}
    }
}

// --- the runtime helpers ---

/// `comet_alloc(size) -> ptr`: first fit from the free list, then bump.
fn emit_alloc(globals: &Globals) -> Function {
    // 0: size (param), 1: need, 2: cur, 3: prev, 4: next, 5: p, 6: end
    let mut f = Function::new_with_locals_types([ValType::I32; 6]);
    let mut i = f.instructions();

    // need = align4(size + HEADER)
    i.local_get(0)
        .i32_const(HEADER + 3)
        .i32_add()
        .i32_const(-4)
        .i32_and()
        .local_set(1);

    i.i32_const(0).local_set(3);
    i.global_get(globals.free_head).local_set(2);

    i.block(BlockType::Empty);
    i.loop_(BlockType::Empty);
    i.local_get(2).i32_eqz().br_if(1);
    // A block is reusable when the space it already has covers what we need.
    i.local_get(2)
        .i32_load(mem(OFF_SIZE))
        .local_get(1)
        .i32_ge_u();
    i.if_(BlockType::Empty);
    i.local_get(2).i32_load(mem(OFF_RC)).local_set(4);
    i.local_get(3).i32_eqz();
    i.if_(BlockType::Empty);
    i.local_get(4).global_set(globals.free_head);
    i.else_();
    i.local_get(3).local_get(4).i32_store(mem(OFF_RC));
    i.end();
    i.local_get(2).i32_const(1).i32_store(mem(OFF_RC));
    i.local_get(2).return_();
    i.end();
    i.local_get(2).local_set(3);
    i.local_get(2).i32_load(mem(OFF_RC)).local_set(2);
    i.br(0);
    i.end();
    i.end();

    // Nothing reusable: take fresh space off the top of the heap.
    i.global_get(globals.heap_next).local_set(5);
    i.local_get(5).local_get(1).i32_add().local_set(6);
    i.local_get(6)
        .memory_size(0)
        .i32_const(16)
        .i32_shl()
        .i32_gt_u();
    i.if_(BlockType::Empty);
    i.local_get(6)
        .memory_size(0)
        .i32_const(16)
        .i32_shl()
        .i32_sub()
        .i32_const(PAGE as i32 - 1)
        .i32_add()
        .i32_const(16)
        .i32_shr_u()
        .memory_grow(0)
        .i32_const(-1)
        .i32_eq();
    // Out of memory is a trap, not a null pointer that corrupts the heap later.
    i.if_(BlockType::Empty);
    i.unreachable();
    i.end();
    i.end();

    i.local_get(6).global_set(globals.heap_next);
    i.local_get(5).local_get(1).i32_store(mem(OFF_SIZE));
    i.local_get(5).i32_const(1).i32_store(mem(OFF_RC));
    i.local_get(5);
    i.end();
    f
}

/// `comet_array_new(count, width) -> handle`.
///
/// Two allocations: a handle holding `[len, capacity, data]`, and a block for
/// the elements. `width` is how many 4-byte slots one element takes.
fn emit_array_new() -> Function {
    // 0: count, 1: width, 2: handle, 3: data
    let mut f = Function::new_with_locals_types([ValType::I32; 2]);
    let mut i = f.instructions();
    i.i32_const(ARRAY_HANDLE).call(F_ALLOC).local_set(2);
    // Room for what was asked for, and never zero bytes - a zero-size block
    // would make two empty arrays share an address.
    i.local_get(0)
        .local_get(1)
        .i32_mul()
        .i32_const(4)
        .i32_mul()
        .i32_const(4)
        .i32_add()
        .call(F_ALLOC)
        .local_set(3);
    i.local_get(2).local_get(0).i32_store(mem(OFF_ARRAY_LEN));
    i.local_get(2).local_get(0).i32_store(mem(OFF_ARRAY_CAP));
    i.local_get(2).local_get(3).i32_store(mem(OFF_ARRAY_DATA));
    i.local_get(2);
    i.end();
    f
}

/// `comet_array_at(handle, index, width) -> address`.
///
/// Traps if `index` is not a position in the array. That is the decision: the
/// common case stays one character, and a script that wants to handle a miss
/// asks for an `Option` instead.
fn emit_array_at() -> Function {
    let mut f = Function::new_with_locals_types(Vec::<ValType>::new());
    let mut i = f.instructions();
    i.local_get(1).i32_const(0).i32_lt_s();
    i.if_(BlockType::Empty).unreachable().end();
    i.local_get(1)
        .local_get(0)
        .i32_load(mem(OFF_ARRAY_LEN))
        .i32_ge_s();
    i.if_(BlockType::Empty).unreachable().end();
    // The elements start after the block's own header, like a String's bytes
    // do: data + HEADER + index * width * 4. Leaving the header out puts
    // element zero on top of the allocator's size field, which corrupts the
    // free list rather than failing.
    i.local_get(0)
        .i32_load(mem(OFF_ARRAY_DATA))
        .i32_const(HEADER)
        .i32_add();
    i.local_get(1)
        .local_get(2)
        .i32_mul()
        .i32_const(4)
        .i32_mul()
        .i32_add();
    i.end();
    f
}

/// `comet_array_push(handle, width) -> address of the new last element`.
///
/// Grows by doubling when full: a new element block, the old contents copied
/// in, the old block freed, and the handle updated - so every other name for
/// this array sees the new storage without knowing anything happened.
fn emit_array_push() -> Function {
    // 0: handle, 1: width, 2: len, 3: cap, 4: data, 5: bigger
    let mut f = Function::new_with_locals_types([ValType::I32; 4]);
    let mut i = f.instructions();
    i.local_get(0).i32_load(mem(OFF_ARRAY_LEN)).local_set(2);
    i.local_get(0).i32_load(mem(OFF_ARRAY_CAP)).local_set(3);
    i.local_get(2).local_get(3).i32_ge_s();
    i.if_(BlockType::Empty);
    // Double, or start at four.
    i.local_get(3).i32_const(0).i32_eq();
    i.if_(BlockType::Result(ValType::I32))
        .i32_const(4)
        .else_()
        .local_get(3)
        .i32_const(2)
        .i32_mul()
        .end();
    i.local_set(3);
    i.local_get(3)
        .local_get(1)
        .i32_mul()
        .i32_const(4)
        .i32_mul()
        .i32_const(4)
        .i32_add()
        .call(F_ALLOC)
        .local_set(5);
    i.local_get(5).i32_const(HEADER).i32_add();
    i.local_get(0)
        .i32_load(mem(OFF_ARRAY_DATA))
        .i32_const(HEADER)
        .i32_add();
    i.local_get(2).local_get(1).i32_mul().i32_const(4).i32_mul();
    i.memory_copy(0, 0);
    i.local_get(0).i32_load(mem(OFF_ARRAY_DATA)).local_set(4);
    i.local_get(0).local_get(5).i32_store(mem(OFF_ARRAY_DATA));
    i.local_get(0).local_get(3).i32_store(mem(OFF_ARRAY_CAP));
    // The old element block was owned by the handle alone.
    i.local_get(4).call(F_RELEASE);
    i.end();
    i.local_get(0)
        .local_get(2)
        .i32_const(1)
        .i32_add()
        .i32_store(mem(OFF_ARRAY_LEN));
    i.local_get(0)
        .i32_load(mem(OFF_ARRAY_DATA))
        .i32_const(HEADER)
        .i32_add();
    i.local_get(2).local_get(1).i32_mul().i32_const(4).i32_mul();
    i.i32_add();
    i.end();
    f
}

/// `comet_array_release(handle)`: one fewer owner, and the elements go with the
/// last one.
///
/// The element block is reachable only through the handle, so it is freed here
/// rather than by the general release, which knows nothing about arrays.
fn emit_array_release() -> Function {
    let mut f = Function::new_with_locals_types(Vec::<ValType>::new());
    let mut i = f.instructions();
    i.local_get(0).i32_eqz();
    i.if_(BlockType::Empty).return_().end();
    // A refcount of one means this release is the last, so the storage goes
    // before the handle does - after, the handle is on the free list and its
    // data pointer is whatever the allocator wrote there.
    i.local_get(0).i32_load(mem(OFF_RC)).i32_const(1).i32_eq();
    i.if_(BlockType::Empty);
    i.local_get(0).i32_load(mem(OFF_ARRAY_DATA)).call(F_RELEASE);
    i.end();
    i.local_get(0).call(F_RELEASE);
    i.end();
    f
}

/// `comet_array_copy(handle, width) -> a new array with the same elements`.
///
/// What makes reference semantics workable: `let b = a;` gives two names for
/// one array, and this is how you say you meant a second one.
fn emit_array_copy() -> Function {
    // 0: handle, 1: width, 2: len, 3: fresh
    let mut f = Function::new_with_locals_types([ValType::I32; 2]);
    let mut i = f.instructions();
    i.local_get(0).i32_load(mem(OFF_ARRAY_LEN)).local_set(2);
    i.local_get(2).local_get(1).call(F_ARRAY_NEW).local_set(3);
    i.local_get(3)
        .i32_load(mem(OFF_ARRAY_DATA))
        .i32_const(HEADER)
        .i32_add();
    i.local_get(0)
        .i32_load(mem(OFF_ARRAY_DATA))
        .i32_const(HEADER)
        .i32_add();
    i.local_get(2).local_get(1).i32_mul().i32_const(4).i32_mul();
    i.memory_copy(0, 0);
    i.local_get(3);
    i.end();
    f
}

/// `comet_retain(ptr)`: one more owner.
fn emit_retain() -> Function {
    let mut f = Function::new_with_locals_types(Vec::<ValType>::new());
    let mut i = f.instructions();
    i.local_get(0).i32_eqz();
    i.if_(BlockType::Empty);
    i.return_();
    i.end();
    // Refcount 0 marks a literal in the data segment - immortal, never counted.
    i.local_get(0).i32_load(mem(OFF_RC)).i32_eqz();
    i.if_(BlockType::Empty);
    i.return_();
    i.end();
    i.local_get(0)
        .local_get(0)
        .i32_load(mem(OFF_RC))
        .i32_const(1)
        .i32_add()
        .i32_store(mem(OFF_RC));
    i.end();
    f
}

/// `comet_release(ptr)`: one fewer owner, and back on the free list at zero.
fn emit_release(globals: &Globals) -> Function {
    // 0: ptr (param), 1: rc
    let mut f = Function::new_with_locals_types([ValType::I32]);
    let mut i = f.instructions();
    i.local_get(0).i32_eqz();
    i.if_(BlockType::Empty);
    i.return_();
    i.end();
    i.local_get(0).i32_load(mem(OFF_RC)).local_tee(1).i32_eqz();
    i.if_(BlockType::Empty);
    i.return_();
    i.end();
    i.local_get(1).i32_const(1).i32_sub().local_set(1);
    i.local_get(1).i32_const(0).i32_gt_s();
    i.if_(BlockType::Empty);
    i.local_get(0).local_get(1).i32_store(mem(OFF_RC));
    i.return_();
    i.end();
    // Dead. The refcount field doubles as the free list's next pointer, so a
    // freed block costs no extra space to keep track of.
    i.local_get(0)
        .global_get(globals.free_head)
        .i32_store(mem(OFF_RC));
    i.local_get(0).global_set(globals.free_head);
    i.end();
    f
}

// --- script functions ---

fn emit_function(
    f: &TypedFn,
    layouts: Layouts<'_>,
    globals: &Globals,
    literals: &Literals,
) -> Function {
    let depth = block_depth(&f.body);
    let mut out = FnGen::new(&f.locals, f.param_count, depth, layouts, globals, literals);
    // A tail expression is the return value. It stays on the stack while the
    // locals it may have been read from are released - it already holds its own
    // reference, so releasing theirs cannot free it.
    out.block(&f.body);
    out.release_owned();
    if f.ret != Type::Unit && f.body.tail.is_none() {
        // Every path returned explicitly, or the checker already reported that
        // some path does not. Either way this end is unreachable - saying so
        // keeps the module valid instead of falling off with an empty stack.
        out.ins().unreachable();
    }
    out.finish()
}

/// The start function: run each state initializer once, at instantiation.
/// The layout tables a script's own types produce.
fn layouts(script: &TypedScript) -> Layouts<'_> {
    Layouts {
        enums: &script.enums,
        structs: &script.structs,
        arrays: &script.arrays,
    }
}

fn emit_init(script: &TypedScript, globals: &Globals, literals: &Literals) -> Function {
    let layouts = layouts(script);
    let depth = script.state.iter().map(|s| expr_depth(&s.init)).max();
    let mut out = FnGen::new(&[], 0, depth.unwrap_or(0), layouts, globals, literals);
    for state in &script.state {
        out.expr(&state.init);
        out.store_global(state.slot);
    }
    out.finish()
}

/// The three shapes Vec2 arithmetic takes on the stack. Grouped because the
/// stack juggling, not the arithmetic, is what differs between them.
#[derive(Debug, Clone, Copy)]
enum Vec2Op {
    /// Two vectors, componentwise. `true` adds, `false` subtracts.
    Componentwise(bool),
    /// A vector then a number. `true` multiplies, `false` divides.
    ScaleRight(bool),
    /// A number then a vector, which only multiplication allows.
    ScaleLeft,
}

impl Vec2Op {
    fn of(op: BinaryOp) -> Option<Self> {
        Some(match op {
            BinaryOp::AddVec2 => Vec2Op::Componentwise(true),
            BinaryOp::SubVec2 => Vec2Op::Componentwise(false),
            BinaryOp::MulVec2F32 => Vec2Op::ScaleRight(true),
            BinaryOp::DivVec2F32 => Vec2Op::ScaleRight(false),
            BinaryOp::MulF32Vec2 => Vec2Op::ScaleLeft,
            _ => return None,
        })
    }
}

/// The three tables that decide how wide a value is. Passed together because
/// they are always needed together, and because a function taking each of them
/// separately alongside everything else grows past what anyone can read.
#[derive(Clone, Copy)]
struct Layouts<'a> {
    enums: &'a [TypedEnum],
    structs: &'a [TypedStruct],
    arrays: &'a [Type],
}

/// The scratch locals available at one level of nesting.
#[derive(Debug, Clone, Copy)]
struct Frame {
    f32s: [u32; 3],
    i32s: [u32; 4],
    /// Where a `match` parks its scrutinee.
    ///
    /// Per nesting level rather than shared. With the current emission order a
    /// shared region would in fact survive - an arm's bindings are unpacked
    /// before its body runs, and nothing reads the parked value afterwards - so
    /// this is defensive rather than load-bearing, and deliberately so: it makes
    /// the reasoning local instead of a property of the order things happen to
    /// be emitted in. The cost is a handful of locals the JIT drops.
    enum_base: u32,
    /// Where a value with f32 slots is parked while part of it is picked out.
    spare_f32: u32,
    /// Where each arm leaves its value.
    ///
    /// A `match` could have used the `if` block's result instead, but a result
    /// wider than one slot needs a function type in the type section, which is
    /// built long before any instruction is emitted. Locals have no such limit
    /// and cost nothing after the JIT.
    result_base: u32,
}

/// The fixed part of a [`Frame`]: three f32 and four i32 scratches.
const FRAME_SCRATCH: u32 = 7;

/// How wide each variable region of a frame is: enough for the widest value in
/// the script, since any of them may be parked while part of it is picked out.
fn spare_width(enums: &[TypedEnum], structs: &[TypedStruct]) -> u32 {
    let widest_enum = enums
        .iter()
        .map(|e| 1 + e.payload_slots as u32)
        .max()
        .unwrap_or(0);
    let widest_struct = structs
        .iter()
        .map(|s| {
            s.fields
                .iter()
                .map(|f| val_types(f.ty, enums, structs).len() as u32)
                .sum::<u32>()
        })
        .max()
        .unwrap_or(0);
    widest_enum.max(widest_struct).max(2)
}

/// The greatest number of [`Frame`]s a block can need at once.
fn block_depth(block: &TypedBlock) -> usize {
    let stmts = block.stmts.iter().map(stmt_depth);
    let tail = block.tail.iter().map(expr_depth);
    stmts.chain(tail).max().unwrap_or(0)
}

fn stmt_depth(stmt: &TypedStmt) -> usize {
    match stmt {
        TypedStmt::Let { init, .. } => expr_depth(init),
        // An assignment's depth is its value's, and also its place's: writing
        // an element parks the handle, the address and the value.
        TypedStmt::Assign { place, value } => expr_depth(value).max(place_depth(place)),
        TypedStmt::If {
            cond,
            then,
            otherwise,
        } => expr_depth(cond)
            .max(block_depth(then))
            .max(otherwise.as_ref().map_or(0, block_depth)),
        TypedStmt::While { cond, body } => expr_depth(cond).max(block_depth(body)),
        TypedStmt::Return { value } => value.as_ref().map_or(0, expr_depth),
        TypedStmt::Expr(e) => expr_depth(e),
    }
}

/// How many frames writing to `place` needs.
fn place_depth(place: &Place) -> usize {
    match place {
        Place::Element { array, index, .. } => 2 + expr_depth(array).max(expr_depth(index)),
        _ => 0,
    }
}

/// Whether emitting `expr` needs a frame of its own, and how deep the ones
/// inside it go.
fn expr_depth(expr: &TypedExpr) -> usize {
    use TypedExprKind as K;
    let (mine, children): (usize, Vec<&TypedExpr>) = match &expr.kind {
        K::Binary { op, lhs, rhs } => {
            let parks = matches!(
                op,
                BinaryOp::RemF32
                    | BinaryOp::ConcatStr
                    | BinaryOp::AddVec2
                    | BinaryOp::SubVec2
                    | BinaryOp::MulVec2F32
                    | BinaryOp::MulF32Vec2
                    | BinaryOp::DivVec2F32
            );
            (usize::from(parks), vec![lhs, rhs])
        }
        // Print parks the string to release it after the call.
        K::HostCall { host, args } => (
            usize::from(*host == Host::Print),
            args.iter().collect::<Vec<_>>(),
        ),
        K::Unary { op, operand } => (usize::from(*op == UnaryOp::NegVec2), vec![operand]),
        K::MakeVec2 { x, y } => (0, vec![x, y]),
        K::Call { args, .. } => (0, args.iter().collect()),
        K::Widen(inner) | K::Narrow(inner) => (0, vec![inner]),
        K::MakeVariant { args, .. } => (0, args.iter().collect()),
        K::MakeStruct { fields, .. } => (0, fields.iter().collect()),
        // Building an array parks the handle, and writing each element parks
        // the address and the value - two levels, not one.
        K::MakeArray { elements, .. } => (2, elements.iter().collect()),
        // Indexing parks the address while the element is read out of it.
        K::Index { array, index, .. } => (1, vec![array, index]),
        // `get` parks the handle, the index and the result it is building.
        K::ArrayGet { array, index, .. } => (1, vec![array, index]),
        // `push` parks the handle and then writes an element, which parks
        // again. `len` and `copy` need one, and taking the larger costs a
        // couple of locals the JIT drops.
        K::ArrayOp { array, value, .. } => (
            2,
            std::iter::once(array.as_ref())
                .chain(value.as_deref())
                .collect(),
        ),
        // Picking a field out of a value parks the whole of it first.
        K::Field { receiver, .. } => (1, vec![receiver]),
        // A match parks its scrutinee to read the tag and then unpack from it.
        K::Match {
            scrutinee, arms, ..
        } => (
            1,
            std::iter::once(scrutinee.as_ref())
                .chain(arms.iter().map(|a| &a.body))
                .collect(),
        ),
        K::Number(_)
        | K::Int(_)
        | K::Bool(_)
        | K::Str(_)
        | K::Local(_)
        | K::Global(_)
        | K::HostField { .. }
        | K::Error => (0, Vec::new()),
    };
    mine + children.into_iter().map(expr_depth).max().unwrap_or(0)
}

struct FnGen<'a> {
    func: Function,
    /// Logical slot -> its first wasm local index.
    slot_base: Vec<u32>,
    slot_types: &'a [Type],
    /// Slots holding a `String`, released on the way out of the function.
    owned: Vec<u32>,
    /// The script's enum layouts, for sizing anything that holds one.
    enums: &'a [TypedEnum],
    /// The script's struct layouts, likewise.
    structs: &'a [TypedStruct],
    /// The element type of each array type the script uses.
    arrays: &'a [Type],
    /// One set of scratch locals per level of nesting that needs them, and a
    /// cursor into it. See [`FnGen::enter_frame`].
    frames: Vec<Frame>,
    frame: usize,
    globals: &'a Globals,
    literals: &'a Literals,
}

impl<'a> FnGen<'a> {
    fn new(
        slot_types: &'a [Type],
        param_count: usize,
        depth: usize,
        layouts: Layouts<'a>,
        globals: &'a Globals,
        literals: &'a Literals,
    ) -> Self {
        let Layouts {
            enums,
            structs,
            arrays,
        } = layouts;
        let mut slot_base = Vec::with_capacity(slot_types.len());
        let mut next = 0;
        for ty in slot_types {
            slot_base.push(next);
            next += val_types(*ty, enums, structs).len() as u32;
        }
        // Scratch locals come in frames, one per level of nesting that needs
        // them. A single shared set would be wrong: the outer `%` in
        // `(a % b) % (c % d)` parks its left operand, then evaluates a right
        // operand that parks its own - and would overwrite it. wasm-encoder
        // wants the local list before the first instruction, so the depth is
        // measured up front rather than discovered while emitting.
        let spare = spare_width(enums, structs);
        let frame_width = FRAME_SCRATCH + spare * 3;
        let frames: Vec<Frame> = (0..depth + 1)
            .map(|i| {
                let base = next + (i as u32) * frame_width;
                Frame {
                    f32s: [base, base + 1, base + 2],
                    i32s: [base + 3, base + 4, base + 5, base + 6],
                    enum_base: base + FRAME_SCRATCH,
                    result_base: base + FRAME_SCRATCH + spare,
                    spare_f32: base + FRAME_SCRATCH + spare * 2,
                }
            })
            .collect();

        let declared: Vec<ValType> = slot_types[param_count..]
            .iter()
            .flat_map(|ty| val_types(*ty, enums, structs))
            // Three f32 scratches, then the i32 regions - four scratches, the
            // parked enum, the match result - then an f32 region for parking a
            // value whose slots are floats.
            .chain(frames.iter().flat_map(|_| {
                [ValType::F32; 3]
                    .into_iter()
                    .chain(std::iter::repeat_n(
                        ValType::I32,
                        (frame_width - 3 - spare) as usize,
                    ))
                    .chain(std::iter::repeat_n(ValType::F32, spare as usize))
            }))
            .collect();

        // Slots whose release does something: a String, or an enum that can
        // hold one. Working this out here rather than at each release site is
        // what keeps the common enum - no payload owning anything - free.
        let owned = slot_types
            .iter()
            .enumerate()
            .filter(|(_, ty)| match ty {
                Type::Str | Type::Array(_) => true,
                Type::Enum(index) => enums[*index as usize].holds_str,
                _ => false,
            })
            .map(|(slot, _)| slot as u32)
            .collect();

        Self {
            func: Function::new_with_locals_types(declared),
            slot_base,
            slot_types,
            owned,
            enums,
            structs,
            arrays,
            frames,
            frame: 0,
            globals,
            literals,
        }
    }

    /// Take this level's scratch locals and move the cursor down, so anything
    /// evaluated inside gets a different set. Returns the frame by value, which
    /// is what keeps the borrow checker from letting two levels share one.
    fn enter_frame(&mut self) -> Frame {
        let frame = self.frames[self.frame];
        self.frame += 1;
        frame
    }

    fn leave_frame(&mut self) {
        self.frame -= 1;
    }

    /// The innermost frame no enclosing construct has taken. For the users that
    /// write a scratch local and read it back with nothing evaluated in
    /// between - which is every user except `%`, `+` on strings, and `print`.
    fn scratch(&self) -> Frame {
        self.frames[self.frame]
    }

    fn finish(mut self) -> Function {
        self.func.instructions().end();
        self.func
    }

    fn ins(&mut self) -> InstructionSink<'_> {
        self.func.instructions()
    }

    // --- places ---

    fn load_slot(&mut self, slot: u32) {
        let base = self.slot_base[slot as usize];
        let width =
            val_types(self.slot_types[slot as usize], self.enums, self.structs).len() as u32;
        for i in 0..width {
            self.ins().local_get(base + i);
        }
    }

    fn store_slot(&mut self, slot: u32) {
        let base = self.slot_base[slot as usize];
        let width =
            val_types(self.slot_types[slot as usize], self.enums, self.structs).len() as u32;
        // The stack has the components in order, so they come off backwards.
        for i in (0..width).rev() {
            self.ins().local_set(base + i);
        }
    }

    fn load_global(&mut self, slot: u32) {
        let base = self.globals.base[slot as usize];
        let width =
            val_types(self.globals.types[slot as usize], self.enums, self.structs).len() as u32;
        for i in 0..width {
            self.ins().global_get(base + i);
        }
    }

    /// Store the value on the stack into a state global, releasing whatever it
    /// held. Globals start at zero, so the first store releases nothing.
    fn store_global(&mut self, slot: u32) {
        let base = self.globals.base[slot as usize];
        let ty = self.globals.types[slot as usize];
        // An enum global that owns something has to have its old value read out
        // and released, which needs the whole value in locals to reach its tag.
        if let Type::Enum(index) = ty
            && self.enums[index as usize].holds_str
        {
            let width = 1 + self.enums[index as usize].payload_slots;
            let frame = self.enter_frame();
            let (incoming, old) = (frame.enum_base, frame.result_base);
            for offset in (0..width).rev() {
                self.ins().local_set(incoming + offset as u32);
            }
            for offset in 0..width {
                self.ins().global_get(base + offset as u32);
                self.ins().local_set(old + offset as u32);
            }
            self.release_enum_slots(old, index);
            for offset in 0..width {
                self.ins().local_get(incoming + offset as u32);
            }
            self.leave_frame();
        }
        if matches!(ty, Type::Str | Type::Array(_)) {
            let tmp = self.scratch().i32s[0];
            self.ins().local_set(tmp);
            self.ins().global_get(base).call(if ty == Type::Str {
                F_RELEASE
            } else {
                F_ARRAY_RELEASE
            });
            self.ins().local_get(tmp);
        }
        let width = val_types(ty, self.enums, self.structs).len() as u32;
        for i in (0..width).rev() {
            self.ins().global_set(base + i);
        }
    }

    /// Release every `String` this function owns: its locals and its
    /// parameters, which the caller handed over.
    fn release_owned(&mut self) {
        for i in 0..self.owned.len() {
            let slot = self.owned[i];
            let ty = self.slot_types[slot as usize];
            let base = self.slot_base[slot as usize];
            match ty {
                Type::Str => {
                    self.ins().local_get(base).call(F_RELEASE);
                }
                // An array's elements are reachable only through its handle, so
                // its release is its own - the general one knows nothing about
                // the second block.
                Type::Array(_) => {
                    self.ins().local_get(base).call(F_ARRAY_RELEASE);
                }
                Type::Enum(index) => self.release_enum_slots(base, index),
                _ => {}
            }
        }
    }

    /// Release whatever the enum value starting at wasm local `base` holds.
    ///
    /// This is the invariant sum types change: what to release stops being a
    /// property of a value's static type. The tag has to be read first, because
    /// which payload is live - and whether there is one at all - depends on it.
    /// The dispatch is emitted only for enums that can hold a `String`; for
    /// every other one, releasing is nothing and there is no tag read.
    fn release_enum_slots(&mut self, base: u32, index: u32) {
        if !self.enums[index as usize].holds_str {
            return;
        }
        let variants = self.enums[index as usize].variants.clone();
        for (tag, variant) in variants.iter().enumerate() {
            // Where each payload starts, and which of them own something.
            let mut at = 1u32;
            let mut owned = Vec::new();
            for ty in &variant.payload {
                let width = val_types(*ty, self.enums, self.structs).len() as u32;
                if self.owns_str(*ty) {
                    owned.push((at, *ty));
                }
                at += width;
            }
            if owned.is_empty() {
                continue;
            }
            self.ins().local_get(base).i32_const(tag as i32).i32_eq();
            self.ins().if_(BlockType::Empty);
            for (offset, ty) in owned {
                match ty {
                    Type::Str => {
                        self.ins().local_get(base + offset).call(F_RELEASE);
                    }
                    Type::Enum(inner) => self.release_enum_slots(base + offset, inner),
                    _ => {}
                }
            }
            self.ins().end();
        }
    }

    /// Take a reference to whatever the value on the stack owns, leaving it
    /// there.
    ///
    /// Binding a variant's payload in a `match` makes a second owner of it, and
    /// without this the enum and the binding would both release, one of them
    /// into a freed block.
    fn retain_value(&mut self, ty: Type) {
        match ty {
            Type::Str | Type::Array(_) => self.retain_top(),
            Type::Enum(index) if self.enums[index as usize].holds_str => {
                let width = 1 + self.enums[index as usize].payload_slots;
                let base = self.enter_frame().enum_base;
                for slot in (0..width).rev() {
                    self.ins().local_set(base + slot as u32);
                }
                self.retain_enum_slots(base, index);
                for slot in 0..width {
                    self.ins().local_get(base + slot as u32);
                }
                self.leave_frame();
            }
            _ => {}
        }
    }

    /// The retain counterpart of [`release_enum_slots`](Self::release_enum_slots),
    /// with the same tag dispatch and the same skip when nothing is owned.
    fn retain_enum_slots(&mut self, base: u32, index: u32) {
        if !self.enums[index as usize].holds_str {
            return;
        }
        let variants = self.enums[index as usize].variants.clone();
        for (tag, variant) in variants.iter().enumerate() {
            let mut at = 1u32;
            let mut owned = Vec::new();
            for ty in &variant.payload {
                let width = val_types(*ty, self.enums, self.structs).len() as u32;
                if self.owns_str(*ty) {
                    owned.push((at, *ty));
                }
                at += width;
            }
            if owned.is_empty() {
                continue;
            }
            self.ins().local_get(base).i32_const(tag as i32).i32_eq();
            self.ins().if_(BlockType::Empty);
            for (offset, ty) in owned {
                match ty {
                    Type::Str => {
                        self.ins().local_get(base + offset).call(F_RETAIN);
                    }
                    Type::Enum(inner) => self.retain_enum_slots(base + offset, inner),
                    _ => {}
                }
            }
            self.ins().end();
        }
    }

    /// Whether releasing a value of this type has anything to do.
    fn owns_str(&self, ty: Type) -> bool {
        match ty {
            Type::Str => true,
            Type::Enum(index) => self.enums[index as usize].holds_str,
            _ => false,
        }
    }

    /// Take a reference to the `String` pointer on the stack, leaving it there.
    fn retain_top(&mut self) {
        let tmp = self.scratch().i32s[0];
        self.ins().local_tee(tmp).call(F_RETAIN).local_get(tmp);
    }

    /// Consume a value that nothing wants: release it if it owns a reference,
    /// then drop however many stack slots it occupies.
    fn drop_value(&mut self, ty: Type) {
        match ty {
            // `release` consumes the pointer, so there is nothing left to drop.
            Type::Str => {
                self.ins().call(F_RELEASE);
            }
            Type::Array(_) => {
                self.ins().call(F_ARRAY_RELEASE);
            }
            Type::Unit | Type::Error => {}
            // An enum that owns something has to be parked to read its tag,
            // because the payload is under it on the stack.
            Type::Enum(index) if self.enums[index as usize].holds_str => {
                let width = 1 + self.enums[index as usize].payload_slots;
                let base = self.enter_frame().enum_base;
                for slot in (0..width).rev() {
                    self.ins().local_set(base + slot as u32);
                }
                self.release_enum_slots(base, index);
                self.leave_frame();
            }
            other => {
                for _ in val_types(other, self.enums, self.structs) {
                    self.ins().drop();
                }
            }
        }
    }

    // --- statements ---

    fn block(&mut self, block: &TypedBlock) {
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            self.expr(tail);
        }
    }

    /// A block in statement position: a `let` inside it can still leave a tail
    /// value behind, and nobody is going to take it.
    fn block_as_stmt(&mut self, block: &TypedBlock) {
        self.block(block);
        if let Some(tail) = &block.tail {
            self.drop_value(tail.ty);
        }
    }

    fn stmt(&mut self, stmt: &TypedStmt) {
        match stmt {
            TypedStmt::Let { slot, init } => self.assign(&Place::Local(*slot), init),
            TypedStmt::Assign { place, value } => self.assign(place, value),

            TypedStmt::If {
                cond,
                then,
                otherwise,
            } => {
                self.expr(cond);
                self.ins().if_(BlockType::Empty);
                self.block_as_stmt(then);
                if let Some(otherwise) = otherwise {
                    self.ins().else_();
                    self.block_as_stmt(otherwise);
                }
                self.ins().end();
            }

            TypedStmt::While { cond, body } => {
                self.ins().block(BlockType::Empty);
                self.ins().loop_(BlockType::Empty);
                self.expr(cond);
                self.ins().i32_eqz().br_if(1);
                self.block_as_stmt(body);
                self.ins().br(0);
                self.ins().end();
                self.ins().end();
            }

            TypedStmt::Return { value } => {
                if let Some(value) = value {
                    self.expr(value);
                }
                // The returned value is already owned, so releasing the local it
                // came from leaves the caller with a live reference.
                self.release_owned();
                self.ins().return_();
            }

            TypedStmt::Expr(expr) => {
                self.expr(expr);
                self.drop_value(expr.ty);
            }
        }
    }

    fn assign(&mut self, place: &Place, value: &TypedExpr) {
        match place {
            Place::Local(slot) => {
                let ty = self.slot_types[*slot as usize];
                self.expr(value);
                if let Type::Enum(index) = ty
                    && self.enums[index as usize].holds_str
                {
                    // The same order as a String: park the new value, release
                    // the old, then put the new one back.
                    let width = 1 + self.enums[index as usize].payload_slots;
                    let frame = self.enter_frame();
                    let (incoming, old) = (frame.enum_base, frame.result_base);
                    for offset in (0..width).rev() {
                        self.ins().local_set(incoming + offset as u32);
                    }
                    let base = self.slot_base[*slot as usize];
                    for offset in 0..width {
                        self.ins().local_get(base + offset as u32);
                        self.ins().local_set(old + offset as u32);
                    }
                    self.release_enum_slots(old, index);
                    for offset in 0..width {
                        self.ins().local_get(incoming + offset as u32);
                    }
                    self.leave_frame();
                }
                if matches!(ty, Type::Str | Type::Array(_)) {
                    // Park the new reference before dropping the old one, so
                    // `a = a` cannot free the object between the two.
                    let tmp = self.scratch().i32s[0];
                    self.ins().local_set(tmp);
                    self.load_slot(*slot);
                    self.ins().call(if ty == Type::Str {
                        F_RELEASE
                    } else {
                        F_ARRAY_RELEASE
                    });
                    self.ins().local_get(tmp);
                }
                self.store_slot(*slot);
            }

            Place::Global(slot) => {
                self.expr(value);
                self.store_global(*slot);
            }

            // Part of a named value - a Vec2 axis, a struct field, a field of a
            // field. Everything is laid out flat and adjacent, so writing part
            // of one is a store into the slots it occupies and the rest is
            // simply left alone: no read-modify-write anywhere.
            Place::LocalAt { slot, offset, ty } => {
                self.expr(value);
                let base = self.slot_base[*slot as usize] + offset;
                let width = val_types(*ty, self.enums, self.structs).len() as u32;
                for i in (0..width).rev() {
                    self.ins().local_set(base + i);
                }
            }
            Place::GlobalAt { slot, offset, ty } => {
                self.expr(value);
                let base = self.globals.base[*slot as usize] + offset;
                let width = val_types(*ty, self.enums, self.structs).len() as u32;
                for i in (0..width).rev() {
                    self.ins().global_set(base + i);
                }
            }

            // A host property. The id goes on the stack first, because every
            // accessor takes it as its first argument.
            Place::Host { field, ty, axis } => {
                let id = *field as i32;
                match (ty, axis) {
                    // One axis of a Vec2 property. It is written whole, so the
                    // other component has to be read back and passed through
                    // untouched.
                    (Type::Vec2, Some(axis)) => {
                        let tmp = self.scratch().f32s[0];
                        self.expr(value);
                        self.ins().local_set(tmp);
                        self.ins().i32_const(id);
                        match axis {
                            Axis::X => {
                                self.ins().local_get(tmp);
                                self.ins().i32_const(id).call(F_GET_VEC2_Y);
                            }
                            Axis::Y => {
                                self.ins().i32_const(id).call(F_GET_VEC2_X);
                                self.ins().local_get(tmp);
                            }
                        }
                        self.ins().call(F_SET_VEC2);
                    }
                    (Type::Vec2, None) => {
                        self.ins().i32_const(id);
                        self.expr(value);
                        self.ins().call(F_SET_VEC2);
                    }
                    (Type::Bool, _) => {
                        self.ins().i32_const(id);
                        self.expr(value);
                        self.ins().call(F_SET_BOOL);
                    }
                    _ => {
                        self.ins().i32_const(id);
                        self.expr(value);
                        self.ins().call(F_SET_F32);
                    }
                }
            }

            // `a[i] = v`, which traps on a bad index like a read does.
            Place::Element { array, index, ty } => {
                let width = val_types(*ty, self.enums, self.structs).len() as u32;
                let frame = self.enter_frame();
                let (at, handle) = (frame.i32s[2], frame.i32s[3]);
                self.expr(array);
                self.ins().local_tee(handle);
                self.expr(index);
                self.ins().i32_const(width as i32);
                self.ins().call(F_ARRAY_AT).local_set(at);
                self.ins().local_get(handle).call(F_ARRAY_RELEASE);
                self.ins().local_get(at);
                self.store_element(value, *ty, width);
                self.leave_frame();
            }

            Place::Error => {
                self.ins().unreachable();
            }
        }
    }

    // --- expressions ---

    fn expr(&mut self, expr: &TypedExpr) {
        match &expr.kind {
            TypedExprKind::Int(value) => {
                self.ins().i32_const(*value);
            }

            // An `int` where an `f32` is wanted. Signed, because the language's
            // int is signed and there is nothing else it could mean.
            TypedExprKind::Widen(inner) => {
                self.expr(inner);
                self.ins().f32_convert_i32_s();
            }

            // `int(x)`: truncate toward zero. Saturating rather than trapping -
            // `i32.trunc_f32_s` traps on NaN and out of range, and a script that
            // divides badly should not take the editor down with it.
            TypedExprKind::Narrow(inner) => {
                self.expr(inner);
                self.ins().i32_trunc_sat_f32_s();
            }

            TypedExprKind::Number(value) => {
                self.ins().f32_const(Ieee32::from(*value));
            }
            TypedExprKind::Bool(value) => {
                self.ins().i32_const(*value as i32);
            }
            TypedExprKind::Str(value) => {
                let offset = self.literals.offset(value) as i32;
                self.ins().i32_const(offset);
                self.retain_top();
            }

            TypedExprKind::Local(slot) => {
                self.load_slot(*slot);
                // Reading a value that owns something makes the copy on the
                // stack a second owner. `retain_value` covers both a String and
                // an enum holding one, which the Str-only check did not.
                self.retain_value(self.slot_types[*slot as usize]);
            }
            TypedExprKind::Global(slot) => {
                self.load_global(*slot);
                self.retain_value(self.globals.types[*slot as usize]);
            }

            TypedExprKind::HostField { field, ty } => {
                let id = *field as i32;
                match ty {
                    // A Vec2 is two stack slots, so it takes two calls.
                    Type::Vec2 => {
                        self.ins().i32_const(id).call(F_GET_VEC2_X);
                        self.ins().i32_const(id).call(F_GET_VEC2_Y);
                    }
                    Type::Bool => {
                        self.ins().i32_const(id).call(F_GET_BOOL);
                    }
                    _ => {
                        self.ins().i32_const(id).call(F_GET_F32);
                    }
                }
            }

            // A Vec2 is two f32s on the stack, so constructing one is just
            // evaluating both components in order.
            TypedExprKind::MakeVec2 { x, y } => {
                self.expr(x);
                self.expr(y);
            }

            // Part of a value on the stack. `transform.position.x` fetches only
            // the axis it wants: going through the general path would call the
            // host twice and throw one result away, on the most common line in
            // a script.
            TypedExprKind::Field {
                receiver,
                offset,
                ty,
            } => {
                if let TypedExprKind::HostField {
                    field,
                    ty: Type::Vec2,
                } = receiver.kind
                {
                    self.ins().i32_const(field as i32).call(if *offset == 0 {
                        F_GET_VEC2_X
                    } else {
                        F_GET_VEC2_Y
                    });
                    return;
                }
                // Otherwise the whole value is on the stack and the wanted
                // slots are somewhere inside it. Park the lot, then push back
                // only the part asked for - the surrounding slots are dropped
                // rather than reached past, because a stack cannot be indexed.
                let whole = val_types(receiver.ty, self.enums, self.structs);
                let width = val_types(*ty, self.enums, self.structs).len() as u32;
                self.expr(receiver);
                let frame = self.enter_frame();
                for slot in (0..whole.len() as u32).rev() {
                    self.ins().local_set(Self::park_slot(&frame, &whole, slot));
                }
                for slot in *offset..*offset + width {
                    self.ins().local_get(Self::park_slot(&frame, &whole, slot));
                }
                self.leave_frame();
            }

            TypedExprKind::Call { index, args } => {
                for arg in args {
                    self.expr(arg);
                }
                self.ins().call(USER_BASE + *index as u32);
            }

            TypedExprKind::HostCall { host, args } => match host {
                // The host formats and allocates, and hands back an owned
                // reference like any other String expression.
                Host::Str => {
                    self.expr(&args[0]);
                    self.ins().call(F_STR);
                }
                Host::StrInt => {
                    self.expr(&args[0]);
                    self.ins().call(F_STR_INT);
                }
                // The transcendentals are ordinary calls: arguments on the
                // stack, one import, a result.
                Host::Sin | Host::Cos | Host::Atan2 | Host::Pow => {
                    for arg in args {
                        self.expr(arg);
                    }
                    self.ins().call(match host {
                        Host::Sin => F_SIN,
                        Host::Cos => F_COS,
                        Host::Atan2 => F_ATAN2,
                        _ => F_POW,
                    });
                }
                Host::Print => {
                    let tmp = self.enter_frame().i32s[0];
                    self.expr(&args[0]);
                    self.ins().local_set(tmp);
                    self.ins().local_get(tmp).i32_const(HEADER).i32_add();
                    self.ins().local_get(tmp).i32_load(mem(OFF_LEN));
                    self.ins().call(F_PRINT);
                    self.ins().local_get(tmp).call(F_RELEASE);
                    self.leave_frame();
                }
            },

            TypedExprKind::Unary { op, operand } => {
                // A Vec2 is two f32s on the stack, so negating one means
                // reaching under the top of the stack for its x.
                // wasm has no i32.neg, so negation is a subtraction from zero -
                // which needs the zero underneath the operand, hence the park.
                if *op == UnaryOp::NegInt {
                    let tmp = self.scratch().i32s[0];
                    self.expr(operand);
                    self.ins().local_set(tmp);
                    self.ins().i32_const(0).local_get(tmp).i32_sub();
                    return;
                }
                if *op == UnaryOp::NegVec2 {
                    let y = self.enter_frame().f32s[0];
                    self.expr(operand);
                    self.ins().local_set(y);
                    self.ins().f32_neg();
                    self.ins().local_get(y).f32_neg();
                    self.leave_frame();
                    return;
                }
                self.expr(operand);
                match op {
                    UnaryOp::NegVec2 => unreachable!("handled above"),
                    UnaryOp::Neg => self.ins().f32_neg(),
                    UnaryOp::NegInt => unreachable!("handled above"),
                    UnaryOp::Not => self.ins().i32_eqz(),
                    UnaryOp::Abs => self.ins().f32_abs(),
                    UnaryOp::Sqrt => self.ins().f32_sqrt(),
                    UnaryOp::Floor => self.ins().f32_floor(),
                    UnaryOp::Ceil => self.ins().f32_ceil(),
                };
            }

            TypedExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),

            // An enum value is its tag, then its payload, then whatever slots
            // the widest variant needs and this one does not - zeroed, because
            // wasm has no undefined value and a match never reads them.
            // `[a, b, c]`: one array of the right size, then each element
            // written into it. The handle is parked because every write needs
            // it and an element can be any expression.
            TypedExprKind::MakeArray { array, elements } => {
                let element_ty = self.arrays[*array as usize];
                let width = val_types(element_ty, self.enums, self.structs).len() as u32;
                let handle = self.enter_frame().i32s[0];
                self.ins().i32_const(elements.len() as i32);
                self.ins().i32_const(width as i32);
                self.ins().call(F_ARRAY_NEW).local_set(handle);
                for (at, element) in elements.iter().enumerate() {
                    self.ins().local_get(handle);
                    self.ins().i32_const(at as i32);
                    self.ins().i32_const(width as i32);
                    self.ins().call(F_ARRAY_AT);
                    self.store_element(element, element_ty, width);
                }
                self.ins().local_get(handle);
                self.leave_frame();
            }

            TypedExprKind::ArrayOp {
                op,
                array,
                value,
                element,
            } => {
                let width = val_types(*element, self.enums, self.structs).len() as u32;
                match op {
                    ArrayOp::Len => {
                        self.expr(array);
                        // The handle is an owned reference the moment it is on
                        // the stack, so reading its length consumes it.
                        let tmp = self.scratch().i32s[0];
                        self.ins().local_tee(tmp);
                        self.ins().i32_load(mem(OFF_ARRAY_LEN));
                        let kept = self.scratch().i32s[1];
                        self.ins().local_set(kept);
                        self.ins().local_get(tmp).call(F_ARRAY_RELEASE);
                        self.ins().local_get(kept);
                    }
                    ArrayOp::Push => {
                        let handle = self.enter_frame().i32s[0];
                        self.expr(array);
                        self.ins().local_tee(handle);
                        self.ins().i32_const(width as i32);
                        self.ins().call(F_ARRAY_PUSH);
                        let value = value.as_deref().expect("push checked its arity");
                        self.store_element(value, *element, width);
                        self.ins().local_get(handle).call(F_ARRAY_RELEASE);
                        self.leave_frame();
                    }
                    ArrayOp::Copy => {
                        let handle = self.scratch().i32s[0];
                        self.expr(array);
                        self.ins().local_tee(handle);
                        self.ins().i32_const(width as i32);
                        self.ins().call(F_ARRAY_COPY);
                        let fresh = self.scratch().i32s[1];
                        self.ins().local_set(fresh);
                        self.ins().local_get(handle).call(F_ARRAY_RELEASE);
                        self.ins().local_get(fresh);
                    }
                }
            }

            // `get(a, i)`: an `Option<T>`, built without a trap. The element
            // and the option's payload are both packed i32 words, so an
            // in-range read is a straight copy - no unpacking on the way.
            TypedExprKind::ArrayGet {
                array,
                index,
                element,
                option,
                none,
                some,
            } => {
                let width = val_types(*element, self.enums, self.structs).len() as u32;
                let payload_slots = self.enums[*option as usize].payload_slots as u32;
                let frame = self.enter_frame();
                let (handle, idx, at) = (frame.i32s[0], frame.i32s[1], frame.i32s[2]);
                let res = frame.result_base;

                self.expr(array);
                self.ins().local_set(handle);
                self.expr(index);
                self.ins().local_set(idx);

                // 0 <= index < len. Both halves, because a negative index is
                // just as much a miss as one past the end.
                self.ins().local_get(idx).i32_const(0).i32_ge_s();
                self.ins().local_get(idx);
                self.ins().local_get(handle).i32_load(mem(OFF_ARRAY_LEN));
                self.ins().i32_lt_s();
                self.ins().i32_and();
                self.ins().if_(BlockType::Empty);
                self.ins().i32_const(*some as i32).local_set(res);
                self.ins().local_get(handle);
                self.ins().local_get(idx);
                self.ins().i32_const(width as i32);
                self.ins().call(F_ARRAY_AT).local_set(at);
                for slot in 0..width {
                    self.ins().local_get(at).i32_load(mem(u64::from(slot) * 4));
                    self.ins().local_set(res + 1 + slot);
                }
                self.ins().else_();
                self.ins().i32_const(*none as i32).local_set(res);
                self.ins().end();
                // Whatever the live variant did not fill: wasm has no undefined
                // value, and a `match` on the result never reads these.
                for slot in width..payload_slots {
                    self.ins().i32_const(0).local_set(res + 1 + slot);
                }
                self.ins().local_get(handle).call(F_ARRAY_RELEASE);
                for slot in 0..=payload_slots {
                    self.ins().local_get(res + slot);
                }
                self.leave_frame();
            }

            // `a[i]`: the address, then the slots at it. Out of bounds traps
            // inside comet_array_at rather than reading whatever is there.
            TypedExprKind::Index { array, index, ty } => {
                let width = val_types(*ty, self.enums, self.structs).len() as u32;
                let frame = self.enter_frame();
                let (at, handle) = (frame.i32s[0], frame.i32s[1]);
                self.expr(array);
                // The handle arrived owned - reading it took a reference - and
                // this expression is done with it once it has the address.
                self.ins().local_tee(handle);
                self.expr(index);
                self.ins().i32_const(width as i32);
                self.ins().call(F_ARRAY_AT).local_set(at);
                self.ins().local_get(handle).call(F_ARRAY_RELEASE);
                for slot in 0..width {
                    self.ins().local_get(at).i32_load(mem(u64::from(slot) * 4));
                }
                self.unpack(*ty);
                // A value read out of an array is a second owner of whatever it
                // holds, the same as reading one out of a local.
                self.retain_value(*ty);
                self.leave_frame();
            }

            // A struct is exactly its fields, in order: building one is
            // evaluating each of them, and there is no header.
            TypedExprKind::MakeStruct { fields, .. } => {
                for field in fields {
                    self.expr(field);
                }
            }

            TypedExprKind::MakeVariant {
                enum_index,
                variant,
                args,
            } => {
                let layout = &self.enums[*enum_index as usize];
                self.ins().i32_const(*variant as i32);
                let mut written = 0;
                for arg in args {
                    let ty = arg.ty;
                    self.expr(arg);
                    written += self.pack(ty);
                }
                for _ in written..layout.payload_slots {
                    self.ins().i32_const(0);
                }
            }

            TypedExprKind::Match {
                scrutinee,
                enum_index,
                arms,
            } => self.match_expr(scrutinee, *enum_index, arms),

            TypedExprKind::Error => {
                self.ins().unreachable();
            }
        }
    }

    /// Where slot `at` of a parked value lives.
    ///
    /// The frame has typed regions - f32 scratches and i32 scratches - so a
    /// value with a mix of both is spread across them by type. Any layout works
    /// as long as it is the same going in and coming out.
    fn park_slot(frame: &Frame, layout: &[ValType], at: u32) -> u32 {
        let mut f32s = 0;
        let mut i32s = 0;
        for (index, ty) in layout.iter().enumerate() {
            let here = if *ty == ValType::F32 {
                let slot = frame.spare_f32 + f32s;
                f32s += 1;
                slot
            } else {
                let slot = frame.enum_base + i32s;
                i32s += 1;
                slot
            };
            if index as u32 == at {
                return here;
            }
        }
        frame.enum_base
    }

    /// Evaluate `value` and write it into the element address on the stack.
    ///
    /// Elements are stored as `i32` words with floats reinterpreted, the same
    /// packing an enum payload uses - so one addressing rule covers every
    /// element type.
    fn store_element(&mut self, value: &TypedExpr, ty: Type, width: u32) {
        let frame = self.enter_frame();
        let (address, slots) = (frame.i32s[1], frame.enum_base);
        self.ins().local_set(address);
        self.expr(value);
        self.pack(ty);
        for slot in (0..width).rev() {
            self.ins().local_set(slots + slot);
        }
        for slot in 0..width {
            self.ins().local_get(address);
            self.ins().local_get(slots + slot);
            self.ins().i32_store(mem(u64::from(slot) * 4));
        }
        self.leave_frame();
    }

    /// Turn the value on the stack into `n` payload slots, all `i32`.
    ///
    /// A uniform slot type is what lets one layout serve every variant, and
    /// reinterpreting an f32 is one instruction that changes no bits. Returns
    /// how many slots it took.
    fn pack(&mut self, ty: Type) -> usize {
        let layout = val_types(ty, self.enums, self.structs);
        // Already all i32 - an int, a bool, a String, an enum, or a struct made
        // only of those. Nothing to do.
        if layout.iter().all(|slot| *slot == ValType::I32) {
            return layout.len();
        }
        let frame = self.enter_frame();
        for slot in (0..layout.len() as u32).rev() {
            self.ins().local_set(Self::park_slot(&frame, &layout, slot));
        }
        for slot in 0..layout.len() as u32 {
            self.ins().local_get(Self::park_slot(&frame, &layout, slot));
            if layout[slot as usize] == ValType::F32 {
                self.ins().i32_reinterpret_f32();
            }
        }
        self.leave_frame();
        layout.len()
    }

    /// The reverse: `n` i32 slots back into a value of `ty`.
    fn unpack(&mut self, ty: Type) {
        let layout = val_types(ty, self.enums, self.structs);
        if layout.iter().all(|slot| *slot == ValType::I32) {
            return;
        }
        // Everything arrives as i32, so the parking region is the i32 one
        // whatever the value's own layout says.
        let frame = self.enter_frame();
        let base = frame.enum_base;
        for slot in (0..layout.len() as u32).rev() {
            self.ins().local_set(base + slot);
        }
        for slot in 0..layout.len() as u32 {
            self.ins().local_get(base + slot);
            if layout[slot as usize] == ValType::F32 {
                self.ins().f32_reinterpret_i32();
            }
        }
        self.leave_frame();
    }

    /// `match`: park the whole value, then a chain of tag comparisons.
    ///
    /// Exhaustiveness is already checked, so the last arm needs no test - there
    /// is no path where none matches, and emitting one would be an unreachable
    /// branch the validator then wants a value from.
    fn match_expr(&mut self, scrutinee: &TypedExpr, enum_index: u32, arms: &[TypedArm]) {
        let width = 1 + self.enums[enum_index as usize].payload_slots;
        let frame = self.enter_frame();
        // The value's slots go into locals reserved after the frame's own, so a
        // nested match cannot overwrite an outer one's scrutinee.
        let base = frame.enum_base;
        self.expr(scrutinee);
        for slot in (0..width).rev() {
            self.ins().local_set(base + slot as u32);
        }

        let result_ty = arms[0].body.ty;
        let result_width = val_types(result_ty, self.enums, self.structs).len() as u32;
        let result_base = frame.result_base;
        self.emit_arms(base, result_base, enum_index, arms, 0);
        // The subject was a value like any other, and this expression consumed
        // it. Its bindings retained whatever they took, so this frees only what
        // no arm kept. Done after the arms and before the result is loaded.
        self.release_enum_slots(base, enum_index);
        for slot in 0..result_width {
            self.ins().local_get(result_base + slot);
        }
        self.unpack(result_ty);
        self.leave_frame();
    }

    /// The tag chain, one `if` per arm bar the last.
    fn emit_arms(
        &mut self,
        base: u32,
        result_base: u32,
        enum_index: u32,
        arms: &[TypedArm],
        at: usize,
    ) {
        let last = at + 1 == arms.len();
        if !last {
            self.ins().local_get(base).i32_const(at as i32).i32_eq();
            self.ins().if_(BlockType::Empty);
        }
        self.unpack_bindings(base, enum_index, at, &arms[at]);
        self.expr(&arms[at].body);
        // Through the same packing the payload uses, so the result region is
        // one uniform block of i32 slots whatever the arms produce.
        let width = self.pack(arms[at].body.ty) as u32;
        for slot in (0..width).rev() {
            self.ins().local_set(result_base + slot);
        }
        if !last {
            self.ins().else_();
            self.emit_arms(base, result_base, enum_index, arms, at + 1);
            self.ins().end();
        }
    }

    /// Copy this variant's payload slots into the locals its pattern named.
    fn unpack_bindings(&mut self, base: u32, enum_index: u32, variant: usize, arm: &TypedArm) {
        let payload = self.enums[enum_index as usize].variants[variant]
            .payload
            .clone();
        let mut at = 1u32;
        for (slot, ty) in arm.bindings.iter().zip(&payload) {
            let width = val_types(*ty, self.enums, self.structs).len() as u32;
            for offset in 0..width {
                self.ins().local_get(base + at + offset);
            }
            self.unpack(*ty);
            // The binding is a second owner of whatever the payload holds.
            self.retain_value(*ty);
            self.store_slot(*slot);
            at += width;
        }
    }

    fn binary(&mut self, op: BinaryOp, lhs: &TypedExpr, rhs: &TypedExpr) {
        // `&&` and `||` short-circuit, so the right operand goes inside a branch
        // rather than being evaluated up front - a call on the right must not
        // run when the left already decided the answer.
        match op {
            BinaryOp::And => {
                self.expr(lhs);
                self.ins().if_(BlockType::Result(ValType::I32));
                self.expr(rhs);
                self.ins().else_().i32_const(0).end();
                return;
            }
            BinaryOp::Or => {
                self.expr(lhs);
                self.ins().if_(BlockType::Result(ValType::I32));
                self.ins().i32_const(1).else_();
                self.expr(rhs);
                self.ins().end();
                return;
            }
            _ => {}
        }

        // Vec2 arithmetic. A Vec2 occupies two stack slots, so every one of
        // these has to park the values it cannot reach past - x always sits
        // under y, and under the whole right operand as well.
        if let Some(vec_op) = Vec2Op::of(op) {
            let [a, b, c] = self.enter_frame().f32s;
            self.expr(lhs);
            self.expr(rhs);
            match vec_op {
                // [ax, ay, bx, by] -> [ax op bx, ay op by]
                Vec2Op::Componentwise(add) => {
                    self.ins().local_set(a); // by
                    self.ins().local_set(b); // bx
                    self.ins().local_set(c); // ay
                    self.ins().local_get(b);
                    if add {
                        self.ins().f32_add();
                    } else {
                        self.ins().f32_sub();
                    }
                    self.ins().local_get(c).local_get(a);
                    if add {
                        self.ins().f32_add();
                    } else {
                        self.ins().f32_sub();
                    }
                }
                // [vx, vy, s] -> [vx op s, vy op s]
                Vec2Op::ScaleRight(mul) => {
                    self.ins().local_set(a); // s
                    self.ins().local_set(b); // vy
                    self.ins().local_get(a);
                    if mul {
                        self.ins().f32_mul();
                    } else {
                        self.ins().f32_div();
                    }
                    self.ins().local_get(b).local_get(a);
                    if mul {
                        self.ins().f32_mul();
                    } else {
                        self.ins().f32_div();
                    }
                }
                // [s, vx, vy] -> [s * vx, s * vy]
                Vec2Op::ScaleLeft => {
                    self.ins().local_set(a); // vy
                    self.ins().local_set(b); // vx
                    self.ins().local_set(c); // s
                    self.ins().local_get(c).local_get(b).f32_mul();
                    self.ins().local_get(c).local_get(a).f32_mul();
                }
            }
            self.leave_frame();
            return;
        }

        // Concatenation is the first thing in the language that allocates, and
        // the only operator that does. Both operands arrive owned, so both are
        // released once their bytes have been copied out.
        if op == BinaryOp::ConcatStr {
            let [a, b, total, out] = self.enter_frame().i32s;
            self.expr(lhs);
            self.ins().local_set(a);
            self.expr(rhs);
            self.ins().local_set(b);

            self.ins().local_get(a).i32_load(mem(OFF_LEN));
            self.ins().local_get(b).i32_load(mem(OFF_LEN));
            self.ins().i32_add().local_set(total);

            self.ins().local_get(total).call(F_ALLOC).local_set(out);
            self.ins()
                .local_get(out)
                .local_get(total)
                .i32_store(mem(OFF_LEN));

            // memory.copy takes destination, source, length.
            self.ins().local_get(out).i32_const(HEADER).i32_add();
            self.ins().local_get(a).i32_const(HEADER).i32_add();
            self.ins().local_get(a).i32_load(mem(OFF_LEN));
            self.ins().memory_copy(0, 0);

            self.ins()
                .local_get(out)
                .i32_const(HEADER)
                .i32_add()
                .local_get(a)
                .i32_load(mem(OFF_LEN))
                .i32_add();
            self.ins().local_get(b).i32_const(HEADER).i32_add();
            self.ins().local_get(b).i32_load(mem(OFF_LEN));
            self.ins().memory_copy(0, 0);

            self.ins().local_get(a).call(F_RELEASE);
            self.ins().local_get(b).call(F_RELEASE);
            self.ins().local_get(out);
            self.leave_frame();
            return;
        }

        // Remainder has no WebAssembly instruction: a - trunc(a / b) * b. Both
        // operands are parked first because both are needed twice, and an
        // operand can be a call - evaluating one twice would run its effects
        // twice.
        if op == BinaryOp::RemF32 {
            let [a, b, _] = self.enter_frame().f32s;
            self.expr(lhs);
            self.ins().local_set(a);
            self.expr(rhs);
            self.ins().local_set(b);
            self.ins().local_get(a);
            self.ins().local_get(a).local_get(b).f32_div().f32_trunc();
            self.ins().local_get(b).f32_mul();
            self.ins().f32_sub();
            self.leave_frame();
            return;
        }

        self.expr(lhs);
        self.expr(rhs);
        let mut i = self.func.instructions();
        match op {
            BinaryOp::AddF32 => i.f32_add(),
            BinaryOp::RemF32
            | BinaryOp::ConcatStr
            | BinaryOp::AddVec2
            | BinaryOp::SubVec2
            | BinaryOp::MulVec2F32
            | BinaryOp::MulF32Vec2
            | BinaryOp::DivVec2F32 => unreachable!("handled above"),
            BinaryOp::MinF32 => i.f32_min(),
            BinaryOp::MaxF32 => i.f32_max(),
            BinaryOp::SubF32 => i.f32_sub(),
            BinaryOp::MulF32 => i.f32_mul(),
            BinaryOp::DivF32 => i.f32_div(),
            BinaryOp::AddInt => i.i32_add(),
            BinaryOp::SubInt => i.i32_sub(),
            BinaryOp::MulInt => i.i32_mul(),
            // Signed, and trapping on a zero divisor - the honest answer for
            // whole numbers, where f32 division quietly gives infinity.
            BinaryOp::DivInt => i.i32_div_s(),
            BinaryOp::RemInt => i.i32_rem_s(),
            BinaryOp::LtInt => i.i32_lt_s(),
            BinaryOp::GtInt => i.i32_gt_s(),
            BinaryOp::LeInt => i.i32_le_s(),
            BinaryOp::GeInt => i.i32_ge_s(),
            BinaryOp::LtF32 => i.f32_lt(),
            BinaryOp::GtF32 => i.f32_gt(),
            BinaryOp::LeF32 => i.f32_le(),
            BinaryOp::GeF32 => i.f32_ge(),
            BinaryOp::Eq(Type::F32) => i.f32_eq(),
            BinaryOp::NotEq(Type::F32) => i.f32_ne(),
            BinaryOp::Eq(_) => i.i32_eq(),
            BinaryOp::NotEq(_) => i.i32_ne(),
            BinaryOp::And | BinaryOp::Or => unreachable!("short-circuited above"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;
    use wasmparser::{Payload, TypeRef};

    /// Every emitted module in these tests goes through the real validator -
    /// "it compiled" is not evidence that wasmtime will accept it.
    fn compile_valid(source: &str) -> Vec<u8> {
        let bytes = compile(source, &crate::schema::example_schema())
            .expect("fixture should compile clean");
        wasmparser::validate(&bytes).expect("emitted module must validate");
        bytes
    }

    /// A script touching every emission path: all four arithmetic ops, all six
    /// comparisons, both logical ops, unary minus and not, if/else, while, a
    /// user call, a Vec2 return, Vec2 field reads off a local, both axis writes
    /// of a host property, a whole-property write, String state, assignment, and
    /// print.
    const KITCHEN_SINK: &str = r#"
        let counter = 0.0;
        let flag = true;
        let label = "tick";
        let home = transform.position;

        func mix(a: f32, b: f32) -> f32 {
            let sum = a + b;
            let diff = a - b;
            let prod = a * b;
            let quot = a / b;
            if sum > diff && prod < quot || !flag {
                return -a;
            }
            while counter < 3.0 {
                counter += 1.0;
            }
            sum
        }

        func whole() -> Vec2 {
            transform.position
        }

        func update(dt: f32) {
            let p = whole();
            let x = p.x;
            let y = p.y;
            transform.position.x = mix(x, dt);
            transform.position.y = y + 1.0;
            transform.position = home;
            let msg = label;
            msg = "other";
            print(msg);
            flag = x == y && x != y || x <= y && x >= y;
            mix(x, y);
        }
    "#;

    #[test]
    fn the_kitchen_sink_script_emits_a_valid_module() {
        compile_valid(KITCHEN_SINK);
    }

    #[test]
    fn fixture_1_bounce_emits_a_valid_module() {
        compile_valid(include_str!("../tests/fixtures/bounce.cmt"));
    }

    #[test]
    fn fixture_2_ticker_emits_a_valid_module() {
        compile_valid(include_str!("../tests/fixtures/ticker.cmt"));
    }

    #[test]
    fn fixture_3_clamp_emits_a_valid_module() {
        compile_valid(include_str!("../tests/fixtures/clamp.cmt"));
    }

    #[test]
    fn a_script_that_does_not_check_does_not_compile() {
        let source = include_str!("../tests/fixtures/type_error.cmt");
        let diagnostics = compile(source, &crate::schema::example_schema())
            .expect_err("a type error must not produce a module");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "expected `f32`, found `bool`");
    }

    #[test]
    fn a_script_that_does_not_parse_does_not_compile() {
        let source = include_str!("../tests/fixtures/unclosed.cmt");
        assert!(compile(source, &crate::schema::example_schema()).is_err());
    }

    // --- the host ABI, pinned ---

    fn imports(bytes: &[u8]) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            if let Payload::ImportSection(section) = payload.expect("valid module") {
                for import in section.into_imports() {
                    let import = import.expect("valid import");
                    assert!(
                        matches!(import.ty, TypeRef::Func(_)),
                        "comet only ever imports functions"
                    );
                    found.push((import.module.to_string(), import.name.to_string()));
                }
            }
        }
        found
    }

    #[test]
    fn every_module_asks_the_host_for_the_same_functions() {
        // The import list is fixed rather than per-script, and now also fixed
        // rather than per-schema: a property accessor takes the property's id,
        // so exposing a hundred more of them adds no imports. A host that can
        // run one comet module can run all of them, and there is one binding
        // table to write rather than one per script. Only the transcendentals
        // are imported - the rest of the maths is one instruction each and
        // never leaves the module.
        let expected: Vec<(String, String)> = [
            "get_f32",
            "set_f32",
            "get_bool",
            "set_bool",
            "get_vec2_x",
            "get_vec2_y",
            "set_vec2",
            "print",
            "str",
            "str_int",
            "sin",
            "cos",
            "atan2",
            "pow",
        ]
        .iter()
        .map(|name| (HOST_MODULE.to_string(), name.to_string()))
        .collect();

        assert_eq!(imports(&compile_valid(KITCHEN_SINK)), expected);
        assert_eq!(
            imports(&compile_valid(include_str!("../tests/fixtures/clamp.cmt"))),
            expected,
            "a script that never prints still imports print"
        );
    }

    fn exports(bytes: &[u8]) -> Vec<String> {
        let mut found = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            if let Payload::ExportSection(section) = payload.expect("valid module") {
                for export in section {
                    found.push(export.expect("valid export").name.to_string());
                }
            }
        }
        found
    }

    #[test]
    fn the_memory_the_allocator_and_every_script_function_are_exported() {
        let exports = exports(&compile_valid(KITCHEN_SINK));
        for name in [
            "memory",
            "comet_alloc",
            "comet_retain",
            "comet_release",
            "mix",
            "whole",
            "update",
        ] {
            assert!(exports.contains(&name.to_string()), "missing {name}");
        }
    }

    #[test]
    fn update_is_exported_taking_one_f32_and_returning_nothing() {
        let bytes = compile_valid(include_str!("../tests/fixtures/bounce.cmt"));
        let mut checked = false;
        let mut types = Vec::new();
        let mut func_types = Vec::new();
        let mut import_count = 0;
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            match payload.expect("valid module") {
                Payload::TypeSection(section) => {
                    for group in section {
                        for ty in group.expect("valid type").into_types() {
                            let func = ty.unwrap_func().clone();
                            types.push((func.params().to_vec(), func.results().to_vec()));
                        }
                    }
                }
                Payload::ImportSection(section) => import_count = section.count(),
                Payload::FunctionSection(section) => {
                    for ty in section {
                        func_types.push(ty.expect("valid function"));
                    }
                }
                Payload::ExportSection(section) => {
                    for export in section {
                        let export = export.expect("valid export");
                        if export.name == "update" {
                            let defined = export.index - import_count;
                            let ty = &types[func_types[defined as usize] as usize];
                            assert_eq!(ty.0, vec![wasmparser::ValType::F32], "update(dt: f32)");
                            assert!(ty.1.is_empty(), "update returns nothing");
                            checked = true;
                        }
                    }
                }
                _ => {}
            }
        }
        assert!(checked, "update was not exported");
    }

    // --- string data ---

    fn data_of(bytes: &[u8]) -> Vec<u8> {
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            if let Payload::DataSection(section) = payload.expect("valid module")
                && let Some(segment) = section.into_iter().next()
            {
                // comet emits exactly one segment: every literal in one block.
                return segment.expect("valid segment").data.to_vec();
            }
        }
        Vec::new()
    }

    #[test]
    fn a_string_literal_reaches_the_data_segment_with_its_header() {
        let data = data_of(&compile_valid(include_str!("../tests/fixtures/ticker.cmt")));
        let text = "one second passed";
        let at = data
            .windows(text.len())
            .position(|w| w == text.as_bytes())
            .expect("the literal's bytes are in the data segment");
        // Header is [size][refcount][len] immediately before the bytes.
        let header = &data[at - HEADER as usize..at];
        assert_eq!(
            i32::from_le_bytes(header[4..8].try_into().unwrap()),
            0,
            "a literal is immortal, so its refcount is the 0 sentinel"
        );
        assert_eq!(
            i32::from_le_bytes(header[8..12].try_into().unwrap()),
            text.len() as i32,
            "the length the host reads for print"
        );
    }

    #[test]
    fn identical_literals_share_one_block() {
        let one = compile_valid(r#"func update(dt: f32) { print("same"); }"#);
        let twice = compile_valid(
            r#"func update(dt: f32) { print("same"); print("same"); let s = "same"; }"#,
        );
        assert_eq!(
            data_of(&one).len(),
            data_of(&twice).len(),
            "three uses of one literal must not be three blocks"
        );
    }

    #[test]
    fn a_script_with_no_strings_emits_no_data_segment() {
        assert!(data_of(&compile_valid(include_str!("../tests/fixtures/bounce.cmt"))).is_empty());
    }

    // --- structure ---

    #[test]
    fn script_state_becomes_globals_initialized_by_a_start_function() {
        let bytes = compile_valid(include_str!("../tests/fixtures/bounce.cmt"));
        let mut globals = 0;
        let mut start = None;
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            match payload.expect("valid module") {
                Payload::GlobalSection(section) => globals = section.count(),
                Payload::StartSection { func, .. } => start = Some(func),
                _ => {}
            }
        }
        // Two pieces of state (speed, direction), plus the heap pointer and the
        // free list head the allocator runs on.
        assert_eq!(globals, 4);
        assert!(start.is_some(), "state has to be initialized before update");
    }

    #[test]
    fn a_script_with_no_state_has_no_start_function() {
        let bytes = compile_valid("func update(dt: f32) { transform.position.x = 1.0; }");
        let has_start = wasmparser::Parser::new(0)
            .parse_all(&bytes)
            .any(|p| matches!(p, Ok(Payload::StartSection { .. })));
        assert!(!has_start);
    }

    #[test]
    fn an_empty_script_still_emits_a_valid_module() {
        compile_valid("");
    }
}
