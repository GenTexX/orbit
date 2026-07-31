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
    Axis, BinaryOp, Host, Place, Type, TypedBlock, TypedExpr, TypedExprKind, TypedFn, TypedScript,
    TypedStmt, UnaryOp,
};

/// The module the host must satisfy every import from.
pub const HOST_MODULE: &str = "orbit";

/// Bytes of header on every heap block: `[size][refcount][len]`.
const HEADER: i32 = 12;
const OFF_SIZE: u64 = 0;
const OFF_RC: u64 = 4;
const OFF_LEN: u64 = 8;

/// The first 16 bytes are reserved and left zero, so a null `String` pointer
/// reads a length of 0 instead of the first literal's bytes.
const DATA_BASE: u32 = 16;
const PAGE: u32 = 65536;

// Function indices. Imports come first in wasm's index space, then everything
// the module defines, so these are fixed for every module comet emits.
const F_GET_X: u32 = 0;
const F_GET_Y: u32 = 1;
const F_SET_POS: u32 = 2;
const F_PRINT: u32 = 3;
/// The transcendentals. WebAssembly has no opcodes for these, so they are the
/// only maths that leaves the module; abs, sqrt, floor, ceil, min and max are
/// one instruction each and are emitted inline.
const F_SIN: u32 = 4;
const F_COS: u32 = 5;
const F_ATAN2: u32 = 6;
const F_POW: u32 = 7;
const F_ALLOC: u32 = 8;
const F_RETAIN: u32 = 9;
const F_RELEASE: u32 = 10;
/// The first script-defined function's index.
const USER_BASE: u32 = 11;

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
    let t_get = types.get(vec![], vec![ValType::F32]);
    let t_set = types.get(vec![ValType::F32, ValType::F32], vec![]);
    let t_print = types.get(vec![ValType::I32, ValType::I32], vec![]);
    let t_unary = types.get(vec![ValType::F32], vec![ValType::F32]);
    let t_binary = types.get(vec![ValType::F32, ValType::F32], vec![ValType::F32]);
    let t_alloc = types.get(vec![ValType::I32], vec![ValType::I32]);
    let t_rc = types.get(vec![ValType::I32], vec![]);
    let t_init = types.get(vec![], vec![]);

    let mut imports = ImportSection::new();
    imports.import(HOST_MODULE, "get_position_x", EntityType::Function(t_get));
    imports.import(HOST_MODULE, "get_position_y", EntityType::Function(t_get));
    imports.import(HOST_MODULE, "set_position", EntityType::Function(t_set));
    imports.import(HOST_MODULE, "print", EntityType::Function(t_print));
    imports.import(HOST_MODULE, "sin", EntityType::Function(t_unary));
    imports.import(HOST_MODULE, "cos", EntityType::Function(t_unary));
    imports.import(HOST_MODULE, "atan2", EntityType::Function(t_binary));
    imports.import(HOST_MODULE, "pow", EntityType::Function(t_binary));

    let mut functions = FunctionSection::new();
    functions.function(t_alloc);
    functions.function(t_rc);
    functions.function(t_rc);
    for f in &script.functions {
        let ty = types.get(param_types(f), val_types(f.ret).to_vec());
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
    for f in &script.functions {
        code.function(&emit_function(f, &globals, &literals));
    }
    if has_state {
        code.function(&emit_init(script, &globals, &literals));
    }

    let mut global_section = GlobalSection::new();
    for state in &script.state {
        for vt in val_types(state.ty) {
            global_section.global(mutable(*vt), &zero_of(*vt));
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
    names.append(F_GET_X, "orbit::get_position_x");
    names.append(F_GET_Y, "orbit::get_position_y");
    names.append(F_SET_POS, "orbit::set_position");
    names.append(F_PRINT, "orbit::print");
    names.append(F_SIN, "orbit::sin");
    names.append(F_COS, "orbit::cos");
    names.append(F_ATAN2, "orbit::atan2");
    names.append(F_POW, "orbit::pow");
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

/// Which of a `Vec2`'s two slots an axis is.
fn axis_offset(axis: Axis) -> u32 {
    match axis {
        Axis::X => 0,
        Axis::Y => 1,
    }
}

fn align4(value: u32) -> u32 {
    value.div_ceil(4) * 4
}

/// How a comet type is represented on the wasm stack. `Vec2` is two f32s - it is
/// a value type, so it never touches the heap; `String` is a pointer.
fn val_types(ty: Type) -> &'static [ValType] {
    match ty {
        Type::F32 => &[ValType::F32],
        Type::Bool => &[ValType::I32],
        Type::Vec2 => &[ValType::F32, ValType::F32],
        Type::Str => &[ValType::I32],
        Type::Unit | Type::Error => &[],
    }
}

fn param_types(f: &TypedFn) -> Vec<ValType> {
    f.locals[..f.param_count]
        .iter()
        .flat_map(|ty| val_types(*ty))
        .copied()
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
        let mut base = Vec::new();
        let mut types = Vec::new();
        let mut next = 0;
        for state in &script.state {
            base.push(next);
            types.push(state.ty);
            next += val_types(state.ty).len() as u32;
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
            TypedStmt::Assign { value, .. } => collect_expr(value, out),
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
        _ => {}
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

fn emit_function(f: &TypedFn, globals: &Globals, literals: &Literals) -> Function {
    let mut out = FnGen::new(&f.locals, f.param_count, globals, literals);
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
fn emit_init(script: &TypedScript, globals: &Globals, literals: &Literals) -> Function {
    let mut out = FnGen::new(&[], 0, globals, literals);
    for state in &script.state {
        out.expr(&state.init);
        out.store_global(state.slot);
    }
    out.finish()
}

struct FnGen<'a> {
    func: Function,
    /// Logical slot -> its first wasm local index.
    slot_base: Vec<u32>,
    slot_types: &'a [Type],
    /// Slots holding a `String`, released on the way out of the function.
    owned: Vec<u32>,
    scratch_f32: u32,
    /// A second f32 scratch, for the one operation that needs both operands
    /// twice.
    scratch_f32b: u32,
    scratch_i32: u32,
    globals: &'a Globals,
    literals: &'a Literals,
}

impl<'a> FnGen<'a> {
    fn new(
        slot_types: &'a [Type],
        param_count: usize,
        globals: &'a Globals,
        literals: &'a Literals,
    ) -> Self {
        let mut slot_base = Vec::with_capacity(slot_types.len());
        let mut next = 0;
        for ty in slot_types {
            slot_base.push(next);
            next += val_types(*ty).len() as u32;
        }
        // Two scratch locals, always. They are only ever written immediately
        // before they are read, so one of each type is enough no matter how
        // deeply expressions nest - and a fixed layout means the local indices
        // are known before a single instruction is emitted, which is what
        // wasm-encoder wants.
        let scratch_f32 = next;
        let scratch_f32b = next + 1;
        let scratch_i32 = next + 2;

        let declared: Vec<ValType> = slot_types[param_count..]
            .iter()
            .flat_map(|ty| val_types(*ty))
            .copied()
            .chain([ValType::F32, ValType::F32, ValType::I32])
            .collect();

        let owned = slot_types
            .iter()
            .enumerate()
            .filter(|(_, ty)| **ty == Type::Str)
            .map(|(slot, _)| slot as u32)
            .collect();

        Self {
            func: Function::new_with_locals_types(declared),
            slot_base,
            slot_types,
            owned,
            scratch_f32,
            scratch_f32b,
            scratch_i32,
            globals,
            literals,
        }
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
        let width = val_types(self.slot_types[slot as usize]).len() as u32;
        for i in 0..width {
            self.ins().local_get(base + i);
        }
    }

    fn store_slot(&mut self, slot: u32) {
        let base = self.slot_base[slot as usize];
        let width = val_types(self.slot_types[slot as usize]).len() as u32;
        // The stack has the components in order, so they come off backwards.
        for i in (0..width).rev() {
            self.ins().local_set(base + i);
        }
    }

    fn load_global(&mut self, slot: u32) {
        let base = self.globals.base[slot as usize];
        let width = val_types(self.globals.types[slot as usize]).len() as u32;
        for i in 0..width {
            self.ins().global_get(base + i);
        }
    }

    /// Store the value on the stack into a state global, releasing whatever it
    /// held. Globals start at zero, so the first store releases nothing.
    fn store_global(&mut self, slot: u32) {
        let base = self.globals.base[slot as usize];
        let ty = self.globals.types[slot as usize];
        if ty == Type::Str {
            let tmp = self.scratch_i32;
            self.ins().local_set(tmp);
            self.ins().global_get(base).call(F_RELEASE);
            self.ins().local_get(tmp);
        }
        let width = val_types(ty).len() as u32;
        for i in (0..width).rev() {
            self.ins().global_set(base + i);
        }
    }

    /// Release every `String` this function owns: its locals and its
    /// parameters, which the caller handed over.
    fn release_owned(&mut self) {
        for i in 0..self.owned.len() {
            let base = self.slot_base[self.owned[i] as usize];
            self.ins().local_get(base).call(F_RELEASE);
        }
    }

    /// Take a reference to the `String` pointer on the stack, leaving it there.
    fn retain_top(&mut self) {
        let tmp = self.scratch_i32;
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
            Type::Unit | Type::Error => {}
            other => {
                for _ in val_types(other) {
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
                if ty == Type::Str {
                    // Park the new reference before dropping the old one, so
                    // `s = s` cannot free the object between the two.
                    let tmp = self.scratch_i32;
                    self.ins().local_set(tmp);
                    self.load_slot(*slot);
                    self.ins().call(F_RELEASE);
                    self.ins().local_get(tmp);
                }
                self.store_slot(*slot);
            }

            Place::Global(slot) => {
                self.expr(value);
                self.store_global(*slot);
            }

            Place::Pos => {
                self.expr(value);
                self.ins().call(F_SET_POS);
            }

            // One axis of a named Vec2. A Vec2 is two adjacent slots, so
            // writing one component is a store into one of them and the other
            // is simply left alone - no read-modify-write needed.
            Place::LocalField(slot, axis) => {
                self.expr(value);
                let base = self.slot_base[*slot as usize];
                self.ins().local_set(base + axis_offset(*axis));
            }
            Place::GlobalField(slot, axis) => {
                self.expr(value);
                let base = self.globals.base[*slot as usize];
                self.ins().global_set(base + axis_offset(*axis));
            }

            Place::PosField(axis) => {
                // Position is written whole, so the other axis has to be read
                // back and passed through untouched.
                self.expr(value);
                let tmp = self.scratch_f32;
                self.ins().local_set(tmp);
                match axis {
                    Axis::X => {
                        self.ins().local_get(tmp).call(F_GET_Y).call(F_SET_POS);
                    }
                    Axis::Y => {
                        self.ins().call(F_GET_X).local_get(tmp).call(F_SET_POS);
                    }
                }
            }

            Place::Error => {
                self.ins().unreachable();
            }
        }
    }

    // --- expressions ---

    fn expr(&mut self, expr: &TypedExpr) {
        match &expr.kind {
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
                if self.slot_types[*slot as usize] == Type::Str {
                    self.retain_top();
                }
            }
            TypedExprKind::Global(slot) => {
                self.load_global(*slot);
                if self.globals.types[*slot as usize] == Type::Str {
                    self.retain_top();
                }
            }

            TypedExprKind::Pos => {
                self.ins().call(F_GET_X).call(F_GET_Y);
            }

            // A Vec2 is two f32s on the stack, so constructing one is just
            // evaluating both components in order.
            TypedExprKind::MakeVec2 { x, y } => {
                self.expr(x);
                self.expr(y);
            }

            TypedExprKind::Field { receiver, axis } => {
                // `pos.x` fetches only the axis it wants. Going through the
                // general path would call the host twice and throw one result
                // away, on the single most common line in a script.
                if matches!(receiver.kind, TypedExprKind::Pos) {
                    self.ins().call(match axis {
                        Axis::X => F_GET_X,
                        Axis::Y => F_GET_Y,
                    });
                    return;
                }
                self.expr(receiver);
                match axis {
                    Axis::X => {
                        self.ins().drop();
                    }
                    Axis::Y => {
                        let tmp = self.scratch_f32;
                        self.ins().local_set(tmp).drop().local_get(tmp);
                    }
                }
            }

            TypedExprKind::Call { index, args } => {
                for arg in args {
                    self.expr(arg);
                }
                self.ins().call(USER_BASE + *index as u32);
            }

            TypedExprKind::HostCall { host, args } => match host {
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
                    self.expr(&args[0]);
                    let tmp = self.scratch_i32;
                    self.ins().local_set(tmp);
                    self.ins().local_get(tmp).i32_const(HEADER).i32_add();
                    self.ins().local_get(tmp).i32_load(mem(OFF_LEN));
                    self.ins().call(F_PRINT);
                    self.ins().local_get(tmp).call(F_RELEASE);
                }
            },

            TypedExprKind::Unary { op, operand } => {
                self.expr(operand);
                match op {
                    UnaryOp::Neg => self.ins().f32_neg(),
                    UnaryOp::Not => self.ins().i32_eqz(),
                    UnaryOp::Abs => self.ins().f32_abs(),
                    UnaryOp::Sqrt => self.ins().f32_sqrt(),
                    UnaryOp::Floor => self.ins().f32_floor(),
                    UnaryOp::Ceil => self.ins().f32_ceil(),
                };
            }

            TypedExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),

            TypedExprKind::Error => {
                self.ins().unreachable();
            }
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

        // Remainder has no WebAssembly instruction: a - trunc(a / b) * b. Both
        // operands are parked first because both are needed twice, and an
        // operand can be a call - evaluating one twice would run its effects
        // twice.
        if op == BinaryOp::RemF32 {
            let (a, b) = (self.scratch_f32, self.scratch_f32b);
            self.expr(lhs);
            self.ins().local_set(a);
            self.expr(rhs);
            self.ins().local_set(b);
            self.ins().local_get(a);
            self.ins().local_get(a).local_get(b).f32_div().f32_trunc();
            self.ins().local_get(b).f32_mul();
            self.ins().f32_sub();
            return;
        }

        self.expr(lhs);
        self.expr(rhs);
        let mut i = self.func.instructions();
        match op {
            BinaryOp::AddF32 => i.f32_add(),
            BinaryOp::RemF32 => unreachable!("handled above"),
            BinaryOp::MinF32 => i.f32_min(),
            BinaryOp::MaxF32 => i.f32_max(),
            BinaryOp::SubF32 => i.f32_sub(),
            BinaryOp::MulF32 => i.f32_mul(),
            BinaryOp::DivF32 => i.f32_div(),
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
        let bytes = compile(source).expect("fixture should compile clean");
        wasmparser::validate(&bytes).expect("emitted module must validate");
        bytes
    }

    /// A script touching every emission path: all four arithmetic ops, all six
    /// comparisons, both logical ops, unary minus and not, if/else, while, a
    /// user call, a Vec2 return, Vec2 field reads off a local, both `pos` axis
    /// writes, a whole-`pos` write, String state, assignment, and print.
    const KITCHEN_SINK: &str = r#"
        let counter = 0.0;
        let flag = true;
        let label = "tick";
        let home = pos;

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
            pos
        }

        func update(dt: f32) {
            let p = whole();
            let x = p.x;
            let y = p.y;
            pos.x = mix(x, dt);
            pos.y = y + 1.0;
            pos = home;
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
        let diagnostics = compile(source).expect_err("a type error must not produce a module");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "expected `f32`, found `bool`");
    }

    #[test]
    fn a_script_that_does_not_parse_does_not_compile() {
        let source = include_str!("../tests/fixtures/unclosed.cmt");
        assert!(compile(source).is_err());
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
        // The import list is fixed rather than per-script: a host that can run
        // one comet module can run all of them, and there is one binding table
        // to write rather than one per script. Only the transcendentals are
        // imported - the rest of the maths is one instruction each and never
        // leaves the module.
        let expected: Vec<(String, String)> = [
            "get_position_x",
            "get_position_y",
            "set_position",
            "print",
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
        let bytes = compile_valid("func update(dt: f32) { pos.x = 1.0; }");
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
