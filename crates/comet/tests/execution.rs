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

use wasmtime::{
    Caller, Engine, Error, Extern, Instance, Linker, Module, Store, WasmParams, WasmResults,
};

/// Bytes of header on a heap block: `[size][refcount][len]`, with the string's
/// bytes following. This is comet's documented ABI - a host is allowed to know
/// it, and this test acts as one.
const HEADER: i32 = 12;
const OFF_RC: i32 = 4;

/// The property ids `comet::example_schema()` assigns, in its declaration
/// order. A real host derives these by walking the schema; a test that acts as
/// a host is allowed to know them, and asserting them here is what would catch
/// the numbering drifting.
const POSITION: i32 = 0;
const ROTATION: i32 = 1;
const SCALE: i32 = 2;

#[derive(Default)]
struct Host {
    position: (f32, f32),
    rotation: f32,
    scale: (f32, f32),
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
    /// during instantiation, so a script whose state reads the position sees this.
    fn at(source: &str, x: f32, y: f32) -> Self {
        let bytes = match comet::compile(source, &comet::example_schema()) {
            Ok(bytes) => bytes,
            Err(diagnostics) => panic!("fixture should compile: {diagnostics:?}"),
        };
        let engine = Engine::default();
        let module = Module::new(&engine, &bytes).expect("wasmtime must accept comet's output");

        let mut linker = Linker::new(&engine);
        let host = comet::HOST_MODULE;
        // The property accessors. Every one takes the property's schema id, so
        // this table is the same size whatever the schema says - which is the
        // point of numbering properties rather than importing one function per
        // property.
        linker
            .func_wrap(host, "get_f32", |c: Caller<'_, Host>, id: i32| {
                if id == ROTATION {
                    c.data().rotation
                } else {
                    0.0
                }
            })
            .expect("host binding");
        linker
            .func_wrap(
                host,
                "set_f32",
                |mut c: Caller<'_, Host>, id: i32, v: f32| {
                    if id == ROTATION {
                        c.data_mut().rotation = v;
                    }
                },
            )
            .expect("host binding");
        linker
            .func_wrap(host, "get_bool", |_: Caller<'_, Host>, _id: i32| 0i32)
            .expect("host binding");
        linker
            .func_wrap(
                host,
                "set_bool",
                |_: Caller<'_, Host>, _id: i32, _v: i32| {},
            )
            .expect("host binding");
        linker
            .func_wrap(
                host,
                "get_vec2_x",
                |c: Caller<'_, Host>, id: i32| match id {
                    POSITION => c.data().position.0,
                    SCALE => c.data().scale.0,
                    _ => 0.0,
                },
            )
            .expect("host binding");
        linker
            .func_wrap(
                host,
                "get_vec2_y",
                |c: Caller<'_, Host>, id: i32| match id {
                    POSITION => c.data().position.1,
                    SCALE => c.data().scale.1,
                    _ => 0.0,
                },
            )
            .expect("host binding");
        linker
            .func_wrap(
                host,
                "set_vec2",
                |mut c: Caller<'_, Host>, id: i32, x: f32, y: f32| match id {
                    POSITION => c.data_mut().position = (x, y),
                    SCALE => c.data_mut().scale = (x, y),
                    _ => {}
                },
            )
            .expect("host binding");
        linker
            .func_wrap(
                host,
                "str",
                |mut c: Caller<'_, Host>, value: f32| -> Result<i32, Error> {
                    // The only host function that allocates. It calls back into the
                    // module rather than reserving memory of its own, so the string
                    // it returns is an ordinary refcounted block that the script
                    // releases like any other.
                    let text = comet::format_f32(value);
                    let Some(Extern::Func(alloc)) = c.get_export("comet_alloc") else {
                        return Err(Error::msg("every comet module exports comet_alloc"));
                    };
                    let alloc = alloc.typed::<i32, i32>(&c)?;
                    let ptr = alloc.call(&mut c, text.len() as i32)?;
                    let Some(Extern::Memory(memory)) = c.get_export("memory") else {
                        return Err(Error::msg("every comet module exports its memory"));
                    };
                    comet::write_str(memory.data_mut(&mut c), ptr, &text);
                    Ok(ptr)
                },
            )
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
                ..Host::default()
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

    fn rotation(&self) -> f32 {
        self.store.data().rotation
    }

    fn scale(&self) -> (f32, f32) {
        self.store.data().scale
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
        comet::write_str(memory.data_mut(&mut self.store), ptr, text);
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
    // `transform.position.x = v` has to read y back and write the pair, since the host only
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
        func update(dt: f32) { transform.position.x = counted(7.0) % counted(3.0); }
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
            if false && bump() { transform.position.x = 1.0; }
            if true || bump() { transform.position.y = 1.0; }
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
            let start = transform.position;
            transform.position.x = 99.0;
            transform.position.y = 99.0;
            transform.position = start;
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
    // Before this a Vec2 could only ever come from the host, so there was no way
    // to have a second one - a home position, a velocity, a target.
    let mut script = Script::at(
        "
        let home = vec2(10.0, 20.0);
        func make(a: f32, b: f32) -> Vec2 { vec2(a, b) }
        func x_of(v: Vec2) -> f32 { v.x }
        func update(dt: f32) { transform.position = home; }
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
            transform.position = vec2(v.x + v.y, home.x + home.y);
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
        func here() -> Vec2 { transform.position }
        func x_of(v: Vec2) -> f32 { v.x }
        func y_of(v: Vec2) -> f32 { v.y }
        ",
        1.0,
        2.0,
    );
    assert_eq!(script.call::<(), (f32, f32)>("here", ()), (1.0, 2.0));
    // Field access off a local, rather than the host-property fast path.
    assert_eq!(script.call::<(f32, f32), f32>("x_of", (3.0, 4.0)), 3.0);
    assert_eq!(script.call::<(f32, f32), f32>("y_of", (3.0, 4.0)), 4.0);
}

#[test]
fn script_state_is_initialized_before_any_call_can_observe_it() {
    let mut script = Script::at(
        "
        let home = transform.position;
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

#[test]
fn concatenation_joins_two_strings() {
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            print("hello, " + "world");
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["hello, world"]);
}

#[test]
fn nested_concatenation_does_not_lose_an_operand() {
    // `(a + b) + (c + d)` is the shape that a single shared set of scratch
    // locals gets wrong: the outer join parks its left operand, then evaluates a
    // right operand that parks its own over the top of it. Each level of nesting
    // gets its own frame precisely so this reads "abcd" and not "cdcd".
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            print(("a" + "b") + ("c" + "d"));
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["abcd"]);
}

#[test]
fn nested_remainder_does_not_lose_an_operand() {
    // The same trap, on the operator that had it first: `%` parks both operands
    // because it computes `a - trunc(a / b) * b` and needs each twice.
    let mut script = Script::new(
        r#"
        let result: f32 = 0.0;
        func update(dt: f32) {
            result = (17.0 % 10.0) % (9.0 % 5.0);
        }
        func value() -> f32 { result }
        "#,
    );
    script.update(0.0);
    // 17 % 10 = 7, 9 % 5 = 4, 7 % 4 = 3.
    assert_eq!(script.call::<(), f32>("value", ()), 3.0);
}

#[test]
fn str_formats_a_number_the_way_it_was_written() {
    // What a beginner's first debug line looks like. A whole number prints
    // without a decimal point - "score: 3", not "score: 3.0000000".
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            print("score: " + str(3.0));
            print("ratio: " + str(1.0 / 4.0));
            print(str(0.0 - 2.5));
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["score: 3", "ratio: 0.25", "-2.5"]);
}

#[test]
fn a_concatenated_string_can_be_joined_again_without_leaking() {
    // Building a line in pieces, which is what a loop body does. Each join
    // allocates and then releases both of its operands, so the intermediates -
    // and the strings `str` allocated - come straight back to the free list.
    // A missing release shows up as the heap growing without bound, which is
    // what this measures.
    let mut script = Script::new(
        r#"
        let line: String = "";
        func update(dt: f32) {
            line = "x=" + str(dt) + " y=" + str(dt + 1.0);
        }
        func print_line() { print(line); }
        "#,
    );
    script.update(1.0);
    let after_first = script.memory_bytes();
    for _ in 0..100 {
        script.update(1.0);
    }
    script.call::<(), ()>("print_line", ());
    assert_eq!(script.printed(), ["x=1 y=2"]);
    assert_eq!(
        script.memory_bytes(),
        after_first,
        "intermediates are reused from the free list rather than growing memory"
    );
}

#[test]
fn a_for_loop_runs_from_the_lower_bound_up_to_but_not_including_the_upper() {
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            for i in 0.0..4.0 {
                print(str(i));
            }
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["0", "1", "2", "3"]);
}

#[test]
fn a_for_loop_whose_bounds_cross_runs_no_iterations() {
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            for i in 3.0..3.0 { print("never"); }
            for i in 5.0..1.0 { print("never"); }
            print("done");
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["done"]);
}

#[test]
fn a_for_loop_evaluates_its_upper_bound_once() {
    // The bound is hoisted into a local, so a bound with a side effect - or
    // simply an expensive one - happens once rather than per iteration.
    let mut script = Script::new(
        r#"
        let calls: f32 = 0.0;
        func bound() -> f32 {
            calls = calls + 1.0;
            3.0
        }
        func update(dt: f32) {
            for i in 0.0..bound() { }
        }
        func call_count() -> f32 { calls }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.call::<(), f32>("call_count", ()), 1.0);
}

#[test]
fn nested_for_loops_each_get_their_own_counter() {
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            for y in 0.0..2.0 {
                for x in 0.0..2.0 {
                    print(str(x) + "," + str(y));
                }
            }
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["0,0", "1,0", "0,1", "1,1"]);
}

#[test]
fn a_warning_does_not_stop_a_script_running() {
    // Warnings are advice. The unused binding below is real and reported, and
    // the script still compiles and still runs.
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            let forgotten = 1.0;
            print("ran");
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["ran"]);

    let (_, diagnostics) = comet::check(
        &comet::parse("func f() { let x = 1.0; }").0,
        &comet::example_schema(),
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, comet::Severity::Warning);
}

#[test]
fn vector_addition_and_subtraction_work_on_both_components() {
    // A Vec2 is two stack slots, so every one of these has to reach under the
    // top of the stack. Distinct numbers everywhere: reading the same lane
    // twice, or crossing x with y, has to change the answer.
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            let a = vec2(1.0, 2.0);
            let b = vec2(10.0, 20.0);
            let sum = a + b;
            let difference = b - a;
            print(str(sum.x) + "," + str(sum.y));
            print(str(difference.x) + "," + str(difference.y));
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["11,22", "9,18"]);
}

#[test]
fn subtraction_keeps_its_operands_the_right_way_round() {
    // The operand order is the thing a stack juggle is most likely to lose.
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            let d = vec2(1.0, 2.0) - vec2(10.0, 20.0);
            print(str(d.x) + "," + str(d.y));
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["-9,-18"]);
}

#[test]
fn scaling_works_with_the_number_on_either_side() {
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            let v = vec2(3.0, 5.0);
            let right = v * 2.0;
            let left = 2.0 * v;
            let divided = v / 2.0;
            print(str(right.x) + "," + str(right.y));
            print(str(left.x) + "," + str(left.y));
            print(str(divided.x) + "," + str(divided.y));
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["6,10", "6,10", "1.5,2.5"]);
}

#[test]
fn negating_a_vector_negates_both_components() {
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            let v = -vec2(3.0, -5.0);
            print(str(v.x) + "," + str(v.y));
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["-3,5"]);
}

#[test]
fn nested_vector_arithmetic_does_not_lose_an_operand() {
    // The same trap `%` and string concatenation had: each level of nesting
    // parks operands, and a shared set of scratch locals would let the inner
    // expression overwrite the outer one's.
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            let v = (vec2(1.0, 2.0) + vec2(10.0, 20.0)) - (vec2(100.0, 200.0) - vec2(1000.0, 2000.0));
            print(str(v.x) + "," + str(v.y));
        }
        "#,
    );
    script.update(0.0);
    // (11, 22) - (-900, -1800)
    assert_eq!(script.printed(), ["911,1822"]);
}

#[test]
fn a_node_moves_by_a_velocity_vector() {
    // The line decision 6 exists for. `pos += vel * dt` is a compound
    // assignment on a Vec2 place, which is the whole path end to end.
    let mut script = Script::at(
        r#"
        let velocity: Vec2 = vec2(30.0, -10.0);
        func update(dt: f32) {
            transform.position += velocity * dt;
        }
        "#,
        100.0,
        50.0,
    );
    script.update(0.5);
    assert_eq!(script.position(), (115.0, 45.0));
    script.update(0.5);
    assert_eq!(script.position(), (130.0, 40.0));
}

#[test]
fn a_script_reads_and_writes_a_property_that_is_not_position() {
    // The point of the schema. `rotation` needed no new IR variant, no new
    // import, and no change to the checker - it is a row in a table the engine
    // owns, and the compiler learned it from there.
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            transform.rotation = transform.rotation + 0.5;
        }
        "#,
    );
    script.update(0.0);
    script.update(0.0);
    assert_eq!(script.rotation(), 1.0);
}

#[test]
fn properties_of_the_same_type_do_not_collide() {
    // Two Vec2 properties, told apart only by the id passed to the accessor.
    // Getting that wrong would move the node when the script scaled it.
    let mut script = Script::at(
        r#"
        func update(dt: f32) {
            transform.scale = vec2(2.0, 3.0);
        }
        "#,
        10.0,
        20.0,
    );
    script.update(0.0);
    assert_eq!(script.scale(), (2.0, 3.0));
    assert_eq!(script.position(), (10.0, 20.0), "position is untouched");
}

#[test]
fn writing_one_axis_of_a_property_leaves_the_other_alone() {
    // A Vec2 property is written whole, so a partial write has to read the
    // other component back first.
    let mut script = Script::at(
        r#"
        func update(dt: f32) {
            transform.position.x = 99.0;
        }
        "#,
        1.0,
        2.0,
    );
    script.update(0.0);
    assert_eq!(script.position(), (99.0, 2.0));
}

#[test]
fn an_object_name_can_be_shadowed_because_nothing_is_magic_now() {
    // `transform` is an ordinary name resolved against the schema, not a
    // keyword, so a local wins - which is also why a script may still call
    // something `pos`.
    let mut script = Script::new(
        r#"
        func update(dt: f32) {
            let pos = 7.0;
            print(str(pos));
        }
        "#,
    );
    script.update(0.0);
    assert_eq!(script.printed(), ["7"]);
}
