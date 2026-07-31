//! comet's execution proof: emitted modules, run on a real wasmtime engine,
//! against a fake host that owns nothing but a position and a list of printed
//! strings.
//!
//! Everything up to here checks *structure* - the type checker proves a script
//! is consistent, and wasmparser proves the bytes are a well-formed module.
//! Neither can tell whether `a < b` emitted `f32.lt` or `f32.gt`, whether a
//! release actually reaches the free list, or whether `&&` really skips its
//! right operand. Only running the code can, so this file runs the code.
//!
//! There is no `Scene` here on purpose (that is helios's job in part B):
//! the compiler has to be provably correct on its own before anything real
//! depends on it.

use wasmtime::{Caller, Engine, Extern, Instance, Linker, Module, Store, WasmParams, WasmResults};

/// Bytes of header on a heap block: `[size][refcount][len]`, with the string's
/// bytes following. This is comet's documented ABI - a host is allowed to know
/// it, and this test acts as one.
const HEADER: i32 = 12;
const OFF_RC: i32 = 4;
const OFF_LEN: i32 = 8;

#[derive(Default)]
struct Host {
    position: (f32, f32),
    printed: Vec<String>,
}

struct Script {
    store: Store<Host>,
    instance: Instance,
}

impl Script {
    fn new(source: &str) -> Self {
        Self::at(source, 0.0, 0.0)
    }

    /// Instantiate with the node already at `(x, y)`. State initializers run
    /// during instantiation, so a script whose state reads `pos` sees this.
    fn at(source: &str, x: f32, y: f32) -> Self {
        let bytes = match comet::compile(source) {
            Ok(bytes) => bytes,
            Err(diagnostics) => panic!("fixture should compile: {diagnostics:?}"),
        };
        let engine = Engine::default();
        let module = Module::new(&engine, &bytes).expect("wasmtime must accept comet's output");

        let mut linker = Linker::new(&engine);
        let host = comet::HOST_MODULE;
        linker
            .func_wrap(host, "get_position_x", |c: Caller<'_, Host>| {
                c.data().position.0
            })
            .expect("host binding");
        linker
            .func_wrap(host, "get_position_y", |c: Caller<'_, Host>| {
                c.data().position.1
            })
            .expect("host binding");
        linker
            .func_wrap(host, "set_position", |mut c: Caller<'_, Host>, x, y| {
                c.data_mut().position = (x, y);
            })
            .expect("host binding");
        linker
            .func_wrap(host, "sin", |_: Caller<'_, Host>, x: f32| x.sin())
            .expect("host binding");
        linker
            .func_wrap(host, "cos", |_: Caller<'_, Host>, x: f32| x.cos())
            .expect("host binding");
        linker
            .func_wrap(host, "atan2", |_: Caller<'_, Host>, y: f32, x: f32| {
                y.atan2(x)
            })
            .expect("host binding");
        linker
            .func_wrap(host, "pow", |_: Caller<'_, Host>, a: f32, b: f32| a.powf(b))
            .expect("host binding");
        linker
            .func_wrap(
                host,
                "print",
                |mut c: Caller<'_, Host>, ptr: i32, len: i32| {
                    let Some(Extern::Memory(memory)) = c.get_export("memory") else {
                        panic!("every comet module exports its memory");
                    };
                    let text = {
                        let data = memory.data(&c);
                        let start = ptr as usize;
                        String::from_utf8_lossy(&data[start..start + len as usize]).into_owned()
                    };
                    c.data_mut().printed.push(text);
                },
            )
            .expect("host binding");

        let mut store = Store::new(
            &engine,
            Host {
                position: (x, y),
                printed: Vec::new(),
            },
        );
        let instance = linker
            .instantiate(&mut store, &module)
            .expect("instantiation, including the start function, must succeed");
        Self { store, instance }
    }

    fn call<P: WasmParams, R: WasmResults>(&mut self, name: &str, params: P) -> R {
        let func = self
            .instance
            .get_typed_func::<P, R>(&mut self.store, name)
            .unwrap_or_else(|e| panic!("`{name}` should be exported with this signature: {e}"));
        func.call(&mut self.store, params)
            .unwrap_or_else(|e| panic!("`{name}` trapped: {e}"))
    }

    fn update(&mut self, dt: f32) {
        self.call::<f32, ()>("update", dt);
    }

    fn position(&self) -> (f32, f32) {
        self.store.data().position
    }

    fn printed(&self) -> Vec<&str> {
        self.store
            .data()
            .printed
            .iter()
            .map(String::as_str)
            .collect()
    }

    fn alloc(&mut self, payload: i32) -> i32 {
        self.call::<i32, i32>("comet_alloc", payload)
    }

    fn release(&mut self, ptr: i32) {
        self.call::<i32, ()>("comet_release", ptr);
    }

    fn memory_bytes(&mut self) -> usize {
        let memory = self.memory();
        memory.data(&self.store).len()
    }

    fn memory(&mut self) -> wasmtime::Memory {
        self.instance
            .get_memory(&mut self.store, "memory")
            .expect("every comet module exports its memory")
    }

    fn read_i32(&mut self, addr: i32) -> i32 {
        let memory = self.memory();
        let data = memory.data(&self.store);
        let at = addr as usize;
        i32::from_le_bytes(data[at..at + 4].try_into().expect("four bytes"))
    }

    fn refcount(&mut self, ptr: i32) -> i32 {
        self.read_i32(ptr + OFF_RC)
    }

    /// Fill a freshly allocated block in as a String, exactly the way a host
    /// would have to. `comet_alloc` sets the size and the refcount; the length
    /// and the bytes are the caller's to write.
    fn write_string(&mut self, ptr: i32, text: &str) {
        let memory = self.memory();
        let data = memory.data_mut(&mut self.store);
        let len = ptr as usize + OFF_LEN as usize;
        data[len..len + 4].copy_from_slice(&(text.len() as i32).to_le_bytes());
        let bytes = ptr as usize + HEADER as usize;
        data[bytes..bytes + text.len()].copy_from_slice(text.as_bytes());
    }

    /// The block address of a string literal, found by its bytes in the data
    /// segment.
    fn literal(&mut self, text: &str) -> i32 {
        let memory = self.memory();
        let data = memory.data(&self.store);
        let at = data
            .windows(text.len())
            .position(|window| window == text.as_bytes())
            .expect("the literal is in the data segment");
        at as i32 - HEADER
    }
}

// --- the fixtures, actually running ---

#[test]
fn the_bouncing_node_moves_and_turns_around_at_both_edges() {
    // The roadmap's proof point, minus the Scene: speed 120, direction 1,
    // reversing outside +/-200.
    let mut script = Script::new(include_str!("fixtures/bounce.cmt"));

    script.update(0.5);
    assert_eq!(script.position().0, 60.0, "120 * 1 * 0.5");
    script.update(0.5);
    assert_eq!(script.position().0, 120.0);

    // Past 200 the direction flips, so the next step goes back the other way.
    script.update(0.5);
    script.update(0.5);
    assert_eq!(script.position().0, 240.0);
    script.update(0.5);
    assert_eq!(
        script.position().0,
        180.0,
        "it turned around at the far edge"
    );

    // And again at the near edge.
    for _ in 0..7 {
        script.update(0.5);
    }
    assert_eq!(script.position().0, -240.0);
    script.update(0.5);
    assert_eq!(
        script.position().0,
        -180.0,
        "it turned around at the near edge"
    );
}

#[test]
fn writing_one_axis_leaves_the_other_untouched() {
    // `pos.x = v` has to read y back and write the pair, since the host only
    // takes a whole position. Getting that backwards would silently zero y.
    let mut script = Script::at(include_str!("fixtures/bounce.cmt"), 0.0, 7.5);
    script.update(0.5);
    assert_eq!(script.position(), (60.0, 7.5));
}

#[test]
fn the_ticker_prints_once_per_second_and_resets_its_counter() {
    let mut script = Script::new(include_str!("fixtures/ticker.cmt"));
    script.update(0.6);
    assert!(script.printed().is_empty(), "0.6s is not a second yet");
    script.update(0.6);
    assert_eq!(script.printed(), ["one second passed"]);
    script.update(0.6);
    assert_eq!(script.printed().len(), 1, "the counter reset, so 0.6 again");
    script.update(0.6);
    assert_eq!(
        script.printed(),
        ["one second passed", "one second passed"],
        "state persisted across every call"
    );
}

#[test]
fn clamp_returns_through_both_its_early_returns_and_its_tail() {
    let mut script = Script::new(include_str!("fixtures/clamp.cmt"));
    let call = |s: &mut Script, v, lo, hi| s.call::<(f32, f32, f32), f32>("clamp", (v, lo, hi));
    assert_eq!(
        call(&mut script, 5.0, 0.0, 10.0),
        5.0,
        "the tail expression"
    );
    assert_eq!(call(&mut script, -5.0, 0.0, 10.0), 0.0, "the first return");
    assert_eq!(
        call(&mut script, 50.0, 0.0, 10.0),
        10.0,
        "the second return"
    );
}

#[test]
fn clamp_holds_the_node_inside_its_range() {
    let mut script = Script::new(include_str!("fixtures/clamp.cmt"));
    script.update(1.0);
    assert_eq!(script.position().0, 80.0, "speed 80 for one second");
    script.update(10.0);
    assert_eq!(script.position().0, 300.0, "clamped, not 880");
}

// --- operators, where a wrong opcode is invisible until it runs ---

const OPERATORS: &str = "
    func sub(a: f32, b: f32) -> f32 { a - b }
    func div(a: f32, b: f32) -> f32 { a / b }
    func mix(a: f32, b: f32) -> f32 { a + b * 2.0 - a / 4.0 }
    func negate(a: f32) -> f32 { -a }
    func flip(b: bool) -> bool { !b }
    func lt(a: f32, b: f32) -> bool { a < b }
    func gt(a: f32, b: f32) -> bool { a > b }
    func le(a: f32, b: f32) -> bool { a <= b }
    func ge(a: f32, b: f32) -> bool { a >= b }
    func eq(a: f32, b: f32) -> bool { a == b }
    func ne(a: f32, b: f32) -> bool { a != b }
";

#[test]
fn the_maths_builtins_compute_what_they_say() {
    // Most of these are one WebAssembly instruction, so this is really asking
    // whether the right opcode was chosen - which structure cannot tell.
    let mut script = Script::new(
        "
        func f_abs(a: f32) -> f32 { abs(a) }
        func f_sqrt(a: f32) -> f32 { sqrt(a) }
        func f_floor(a: f32) -> f32 { floor(a) }
        func f_ceil(a: f32) -> f32 { ceil(a) }
        func f_min(a: f32, b: f32) -> f32 { min(a, b) }
        func f_max(a: f32, b: f32) -> f32 { max(a, b) }
        func f_rem(a: f32, b: f32) -> f32 { a % b }
        func f_pow(a: f32, b: f32) -> f32 { pow(a, b) }
        func f_sin(a: f32) -> f32 { sin(a) }
        func f_cos(a: f32) -> f32 { cos(a) }
        ",
    );
    assert_eq!(script.call::<f32, f32>("f_abs", -3.5), 3.5);
    assert_eq!(script.call::<f32, f32>("f_sqrt", 9.0), 3.0);
    assert_eq!(script.call::<f32, f32>("f_floor", 2.7), 2.0);
    assert_eq!(script.call::<f32, f32>("f_ceil", 2.1), 3.0);
    // Asymmetric arguments, so min and max cannot be swapped unnoticed.
    assert_eq!(script.call::<(f32, f32), f32>("f_min", (2.0, 7.0)), 2.0);
    assert_eq!(script.call::<(f32, f32), f32>("f_max", (2.0, 7.0)), 7.0);
    assert_eq!(script.call::<(f32, f32), f32>("f_pow", (2.0, 10.0)), 1024.0);

    // Remainder has no instruction - it is emitted as a - trunc(a / b) * b - and
    // takes the sign of the left operand, like Rust's `%`.
    assert_eq!(script.call::<(f32, f32), f32>("f_rem", (7.0, 3.0)), 1.0);
    assert_eq!(script.call::<(f32, f32), f32>("f_rem", (-7.0, 3.0)), -1.0);
    assert_eq!(script.call::<(f32, f32), f32>("f_rem", (7.5, 2.0)), 1.5);

    let half_pi = std::f32::consts::FRAC_PI_2;
    assert!((script.call::<f32, f32>("f_sin", half_pi) - 1.0).abs() < 1e-6);
    assert!(script.call::<f32, f32>("f_cos", 0.0) == 1.0);
}

#[test]
fn an_operand_of_a_remainder_is_evaluated_once() {
    // Both operands are needed twice by the formula, so a naive emission
    // evaluates them twice - and an operand can be a call.
    let mut script = Script::new(
        "
        let calls = 0.0;
        func counted(v: f32) -> f32 {
            calls += 1.0;
            v
        }
        func how_many() -> f32 { calls }
        func update(dt: f32) { pos.x = counted(7.0) % counted(3.0); }
        ",
    );
    script.update(0.0);
    assert_eq!(script.position().0, 1.0);
    assert_eq!(
        script.call::<(), f32>("how_many", ()),
        2.0,
        "once per operand, not twice"
    );
}

#[test]
fn arithmetic_keeps_its_operands_in_order() {
    // Subtraction and division are the two that a swapped operand order would
    // quietly survive everywhere else.
    let mut script = Script::new(OPERATORS);
    assert_eq!(script.call::<(f32, f32), f32>("sub", (10.0, 3.0)), 7.0);
    assert_eq!(script.call::<(f32, f32), f32>("div", (10.0, 4.0)), 2.5);
    assert_eq!(
        script.call::<(f32, f32), f32>("mix", (8.0, 3.0)),
        12.0,
        "8 + 6 - 2: precedence survived the round trip through wasm"
    );
    assert_eq!(script.call::<f32, f32>("negate", 3.0), -3.0);
}

#[test]
fn every_comparison_picks_the_opcode_it_named() {
    // All six called with (1, 2), so a swap or a neighbouring opcode shows up as
    // a flipped answer rather than as nothing at all.
    let mut script = Script::new(OPERATORS);
    let expected = [
        ("lt", 1),
        ("gt", 0),
        ("le", 1),
        ("ge", 0),
        ("eq", 0),
        ("ne", 1),
    ];
    for (name, want) in expected {
        assert_eq!(
            script.call::<(f32, f32), i32>(name, (1.0, 2.0)),
            want,
            "{name}(1, 2)"
        );
    }
    assert_eq!(script.call::<i32, i32>("flip", 1), 0);
    assert_eq!(script.call::<i32, i32>("flip", 0), 1);
}

#[test]
fn logical_operators_do_not_evaluate_what_they_do_not_need() {
    // Short-circuiting is invisible to the validator: both spellings emit
    // perfectly valid code. Only a side effect in the right operand can tell.
    let mut script = Script::new(
        "
        let hits = 0.0;
        func bump() -> bool {
            hits += 1.0;
            true
        }
        func hits_so_far() -> f32 { hits }
        func update(dt: f32) {
            if false && bump() { pos.x = 1.0; }
            if true || bump() { pos.y = 1.0; }
        }
        ",
    );
    script.update(0.0);
    assert_eq!(
        script.call::<(), f32>("hits_so_far", ()),
        0.0,
        "neither branch should have called bump"
    );
    assert_eq!(
        script.position(),
        (0.0, 1.0),
        "but both ifs decided correctly"
    );
}

#[test]
fn a_while_loop_runs_to_completion() {
    let mut script = Script::new(
        "
        func sum_below(n: f32) -> f32 {
            let total = 0.0;
            let i = 0.0;
            while i < n {
                total += i;
                i += 1.0;
            }
            total
        }
        ",
    );
    assert_eq!(script.call::<f32, f32>("sum_below", 5.0), 10.0, "0+1+2+3+4");
    assert_eq!(
        script.call::<f32, f32>("sum_below", 0.0),
        0.0,
        "never entered"
    );
}

// --- Vec2, which is two stack slots rather than one ---

#[test]
fn a_vec2_is_copied_not_aliased() {
    let mut script = Script::at(
        "
        func update(dt: f32) {
            let start = pos;
            pos.x = 99.0;
            pos.y = 99.0;
            pos = start;
        }
        ",
        3.0,
        4.0,
    );
    script.update(0.0);
    assert_eq!(
        script.position(),
        (3.0, 4.0),
        "`start` held its own copy, so restoring it undid both writes"
    );
}

#[test]
fn a_vec2_can_be_built_and_taken_apart() {
    // Before this a Vec2 could only ever come from `pos`, so there was no way
    // to have a second one - a home position, a velocity, a target.
    let mut script = Script::at(
        "
        let home = vec2(10.0, 20.0);
        func make(a: f32, b: f32) -> Vec2 { vec2(a, b) }
        func x_of(v: Vec2) -> f32 { v.x }
        func update(dt: f32) { pos = home; }
        ",
        99.0,
        99.0,
    );
    assert_eq!(
        script.call::<(f32, f32), (f32, f32)>("make", (3.0, 4.0)),
        (3.0, 4.0)
    );
    assert_eq!(script.call::<(f32, f32), f32>("x_of", (3.0, 4.0)), 3.0);
    script.update(0.0);
    assert_eq!(script.position(), (10.0, 20.0), "state built with vec2");
}

#[test]
fn one_axis_of_a_named_vec2_can_be_written() {
    // A partial write: the other component must be left exactly as it was.
    let mut script = Script::new(
        "
        let home = vec2(1.0, 2.0);
        func update(dt: f32) {
            let v = vec2(5.0, 6.0);
            v.x = 50.0;
            home.y = 20.0;
            pos = vec2(v.x + v.y, home.x + home.y);
        }
        ",
    );
    script.update(0.0);
    assert_eq!(
        script.position(),
        (56.0, 21.0),
        "v is (50, 6) and home is (1, 20) - only one axis moved in each"
    );
}

#[test]
fn a_vec2_crosses_the_boundary_as_two_f32s() {
    let mut script = Script::at(
        "
        func here() -> Vec2 { pos }
        func x_of(v: Vec2) -> f32 { v.x }
        func y_of(v: Vec2) -> f32 { v.y }
        ",
        1.0,
        2.0,
    );
    assert_eq!(script.call::<(), (f32, f32)>("here", ()), (1.0, 2.0));
    // Field access off a local, rather than the `pos` fast path.
    assert_eq!(script.call::<(f32, f32), f32>("x_of", (3.0, 4.0)), 3.0);
    assert_eq!(script.call::<(f32, f32), f32>("y_of", (3.0, 4.0)), 4.0);
}

#[test]
fn script_state_is_initialized_before_any_call_can_observe_it() {
    let mut script = Script::at(
        "
        let home = pos;
        func home_x() -> f32 { home.x }
        func home_y() -> f32 { home.y }
        ",
        7.0,
        8.0,
    );
    assert_eq!(script.call::<(), f32>("home_x", ()), 7.0);
    assert_eq!(script.call::<(), f32>("home_y", ()), 8.0);
}

// --- reference counting, end to end on real heap blocks ---

/// A script that does the three things ownership has to survive: take a String
/// and drop it, take one and store it, and read one back out of state.
const STRINGS: &str = r#"
    let held = "initial";
    func forget(s: String) { }
    func keep(s: String) { held = s; }
    func show() { print(held); }
"#;

#[test]
fn a_string_parameter_is_owned_by_the_callee_and_freed_on_the_way_out() {
    let mut script = Script::new(STRINGS);
    let block = script.alloc(5);
    assert_eq!(script.refcount(block), 1, "a fresh block has one owner");

    // `forget` does nothing with its parameter - but the caller handed over its
    // reference, so the callee still has to release it on the way out.
    script.call::<i32, ()>("forget", block);

    let again = script.alloc(5);
    assert_eq!(
        again, block,
        "the block the callee released must be back on the free list"
    );
}

#[test]
fn storing_a_string_in_state_keeps_it_alive_and_frees_what_it_replaced() {
    let mut script = Script::new(STRINGS);

    let hello = script.alloc(5);
    script.write_string(hello, "hello");
    script.call::<i32, ()>("keep", hello);
    assert_eq!(
        script.refcount(hello),
        1,
        "state retained it and the parameter released it: one owner, not two"
    );

    script.call::<(), ()>("show", ());
    assert_eq!(
        script.printed(),
        ["hello"],
        "print read the length and bytes back out of the heap"
    );

    // Replacing it has to release the old one.
    let world = script.alloc(5);
    script.write_string(world, "world");
    script.call::<i32, ()>("keep", world);
    assert_eq!(script.refcount(world), 1);
    script.call::<(), ()>("show", ());
    assert_eq!(script.printed(), ["hello", "world"]);

    let reused = script.alloc(5);
    assert_eq!(
        reused, hello,
        "the string that state replaced must have been freed"
    );
}

#[test]
fn reading_a_string_out_of_state_leaves_the_count_where_it_found_it() {
    let mut script = Script::new(STRINGS);
    let block = script.alloc(5);
    script.write_string(block, "hello");
    script.call::<i32, ()>("keep", block);

    for _ in 0..20 {
        script.call::<(), ()>("show", ());
    }
    assert_eq!(
        script.refcount(block),
        1,
        "twenty retain/release pairs must net to zero, not drift"
    );
    assert_eq!(script.printed().len(), 20);
}

#[test]
fn a_string_literal_is_never_freed_no_matter_how_often_it_is_released() {
    // Literals live in the data segment. If a release ever reached one it would
    // land on the free list and the next alloc would hand out the middle of the
    // program's own constants.
    let mut script = Script::new(STRINGS);
    let literal = script.literal("initial");
    assert_eq!(script.refcount(literal), 0, "0 is the immortal sentinel");

    for _ in 0..20 {
        script.call::<(), ()>("show", ());
    }
    // Replacing state releases the literal that was there.
    let block = script.alloc(5);
    script.write_string(block, "hello");
    script.call::<i32, ()>("keep", block);

    assert_eq!(
        script.refcount(literal),
        0,
        "still immortal, still untouched"
    );
    let next = script.alloc(5);
    assert!(
        next > literal,
        "no allocation may ever be handed out of the data segment"
    );
}

// --- the allocator itself ---

#[test]
fn the_allocator_reuses_a_freed_block_that_is_big_enough() {
    let mut script = Script::new(STRINGS);
    let big = script.alloc(100);
    let other = script.alloc(10);
    assert_ne!(big, other, "two live blocks cannot overlap");

    script.release(big);
    let small = script.alloc(10);
    assert_eq!(
        small, big,
        "first fit: a free 100-byte block satisfies a 10-byte request"
    );
}

#[test]
fn releasing_the_last_owner_is_what_frees_a_block() {
    let mut script = Script::new(STRINGS);
    let block = script.alloc(8);
    script.call::<i32, ()>("comet_retain", block);
    assert_eq!(script.refcount(block), 2);

    script.release(block);
    assert_eq!(script.refcount(block), 1, "one owner left, still alive");
    let other = script.alloc(8);
    assert_ne!(other, block, "a live block must not be handed out again");

    script.release(block);
    let reused = script.alloc(8);
    assert_eq!(reused, block, "now that nobody owns it, it is free");
}

#[test]
fn the_allocator_grows_memory_rather_than_running_off_the_end() {
    let mut script = Script::new(STRINGS);
    let before = script.memory_bytes();
    assert!(before <= 65536, "modules start at one page");

    let big = script.alloc(200_000);
    assert_ne!(big, 0, "allocation must not fail silently");
    assert!(
        script.memory_bytes() >= big as usize + 200_000 + HEADER as usize,
        "the block has to actually be addressable"
    );

    // And the grown heap is still usable.
    script.write_string(big, "at the far end");
    script.call::<i32, ()>("keep", big);
    script.call::<(), ()>("show", ());
    assert_eq!(script.printed(), ["at the far end"]);
}
