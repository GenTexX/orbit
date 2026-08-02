# Two numeric types, and widening that only goes one way

Comet has `f32` and `int` (32-bit signed). How a literal is written decides which it is: `5` is an `int`, `5.0` is an `f32`. An `int` widens to `f32` implicitly wherever one is wanted; going the other way needs `int(x)`, which truncates toward zero. A `for` loop counts in `int`.

## Why a second numeric type at all

An f32-only language has no honest index and no honest counter. `arr[1.5]` is legal, a loop counter accumulates rounding, and "how many" is stored in a type that cannot exactly represent every answer. In a language whose stated purpose is teaching, the type system saying `1.5` is not a position in a list is worth more than the simplicity of having one number.

It was added now rather than when arrays force it, because every iteration after this one writes numeric code. Retrofitting `int` after enums, generics and containers means revisiting all of them.

The cost is real and was accepted: two literal kinds, int and float variants of every arithmetic and comparison operation, and a conversion rule. This is the largest mechanical change in the language's completion plan.

## Why widening is implicit, and only upward

**Implicit** because the alternative breaks every script ever written. The Milestone 4 plan said bare integer literals are `f32` literals, and every script in the repo relied on it: `transform.position.x += 1` and `past_edge(x, 200)` would all have become errors. Widening keeps them compiling, which is why decision 5 exists and why that plan line is marked superseded rather than quietly contradicted.

**Only upward** because there is then no precision-loss surprise. `int` to `f32` is exact for every value a 32-bit int holds up to 2^24, and beyond that it is a rounding a script asked for by using an f32. `f32` to `int` throws away information, so it is spelled.

Fully context-inferred literal types - where `let x: int = 5` and `let y: f32 = 5` both work by inferring the literal's type from its use - were rejected. They need a second pass or unification variables, against a checker whose headline property is speed (ADR 0007). Widening gets most of the benefit for one comparison in one function.

## How

**One coercion point.** `Checker::coerce` is the only place widening happens, and every site that checks an expression against an expected type routes through it: an annotated `let`, an argument, a return, a tail expression, an assignment, a Vec2 scalar. Scattering the rule would have meant eventually missing one, and the failure mode is silent - the checker permits it, codegen emits an i32 where an f32 belongs, and the module fails to validate with no line number.

The widening is a node in the typed IR (`Widen`), not a permission. A rule the checker allows but does not record is a rule codegen cannot act on.

**Whole numbers print whole.** `str` of an `int` is a separate host call rather than a widen-then-format. Past 2^24, widening would print a rounded number, silently, in the one function a beginner uses to look at a value. There is a test that fails with `16777216` if that path is taken.

**Narrowing saturates rather than traps.** `i32.trunc_f32_s` traps on NaN and out of range; `int(1.0 / 0.0)` would take the editor down with the script. `i32.trunc_sat_f32_s` gives the extreme instead. A wrong number that can be seen and reasoned about beats a crash.

**Integer division traps on zero.** This is the one place `int` is sharper than `f32`, which quietly gives infinity. It is left as a trap: dividing by zero is a bug, and whole numbers have no value that means "not a number".

## Consequences

- `for i in a..b` counts in `int`, and `for i in 0.0..3.5` is now an error rather than a question nobody wants to answer.
- The maths builtins - `abs`, `sqrt`, `floor`, `min`, `max`, and the transcendentals - stay `f32`-only. `abs(-5)` widens and returns an `f32`, which is a wart. Doubling them is a bigger surface than this iteration should carry, and containers are what will make `min`/`max` on ints actually hurt.
- `str_int` joins the fixed import list, so every host binds one more function.
- Nothing about ADR 0007's memory model changes: an `int` is a value type in a wasm local or global, and never allocates.
