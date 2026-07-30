# Plan - Milestone 4: Comet

## Context

Milestone 3's iteration phase (103 commits, six reports) made the editor and its GUI framework genuinely good, but the project underneath it is still, as the M3.6 report put it, "a pile of static sprites" - `helios` has exactly one component (`Sprite`), and nothing can be played. Milestone 4 is where user code first enters a Scene: a Comet script attached to a node, compiling in milliseconds and running on wasmtime. It is explicitly narrower than "Play" (that is Milestone 5, which brings `voyager` online and adds hot reload) - M4 proves the pipeline is real and gives the editor a code editor to write scripts in, not a game loop to run them in.

This is the largest milestone since M1. Three ADRs already settled its architecture before any of this planning started (0006: static types, GC references, casual surface; 0007: single-pass WASM emission, wasmtime, refcounting in linear memory; 0010: in-process language service, error-tolerant and incremental from the start). What remained open - concrete syntax, how M4 proves a node actually moves without Play existing yet, and how much of the code editor to build - was resolved directly with Philip:

- **Syntax:** casual C-family. Braces for blocks (no indentation-sensitive lexing), `func`/`let`, required semicolons, and Rust's tail-expression-as-return rule.
- **Proof that it runs:** headless only. No "Run" button or execution surface in atlas for M4 - the pipeline is proven by tests that compile a script, run it on wasmtime, and assert a real `Scene` node's `Transform` changed. Nothing needs to visibly move in the editor until M5's Play button exists.
- **Code editor scope:** the whole ideatank list - colored syntax highlighting (styled text runs), error/warning decorations, a monospace font, a gutter, find/replace, and an autocomplete popup (which needs a dropdown/list widget aurora does not have yet).
- **Ship-time optimization** (a second, optimizing codegen path distinct from Cranelift's JIT, for exported games) is explicitly out of scope here - see Scope below.

## Example scripts

Five `.cmt` scripts in the syntax this plan assumes, written so each one becomes a real compiler test fixture the moment step 1/2/13 exist - the three valid ones must compile clean, the two broken ones are the first parser-recovery and type-diagnostic fixtures.

**Naming/semantic choices embedded below:**
- `pos` is a bare, magic identifier bound to the owning node's position - not `self.pos`, not `node.position`. No `self` exists in v1 at all (there is nothing for it to be a receiver of - no user methods yet).
- A top-level `let` is **persistent script state** - a field that survives across `update` calls for the life of the script instance (it compiles to a WASM global/memory slot that is simply never reset) - while a `let` inside a function is an ordinary local. Same keyword, two scopes, distinguished by where it appears.
- `let` alone means a mutable binding - there is no `let mut` distinction and no immutable-by-default (unlike Rust). Casual scripts are just variables.
- **Semicolons are required** after every statement.
- **A block's last line, if it is a bare expression with no trailing semicolon, is that block's value** - Rust's tail-expression rule, borrowed exactly. This is the only implicit return: an early return mid-function still needs an explicit `return value;`. See `clamp` below - two explicit early returns, one implicit tail return.
- Functions: `func name(param: Type) -> ReturnType { ... }`.
- Bare integer literals (`200`, not `200.0`) are valid `f32` literals - there is no separate integer type in v1, so there is nothing to coerce.
- `//` line comments.
- `print(s: String)` is the one host-bound debug function, and the only thing that exercises the refcounted `String` path in v1.

### 1. The roadmap's literal proof point - a node that moves

```
// Bounces the node along X between -200 and 200.
let speed = 120.0;
let direction = 1.0;

func update(dt: f32) {
    pos.x += speed * direction * dt;

    if pos.x > 200.0 {
        direction = -1.0;
    }
    if pos.x < -200.0 {
        direction = 1.0;
    }
}
```

### 2. String + print - exercises the one heap/refcounted type

```
let ticks = 0.0;

func update(dt: f32) {
    ticks += dt;
    if ticks > 1.0 {
        print("one second passed");
        ticks = 0.0;
    }
}
```

### 3. A user function, a tail-position return, and Vec2 field arithmetic together

```
func clamp(value: f32, lo: f32, hi: f32) -> f32 {
    if value < lo {
        return lo;
    }
    if value > hi {
        return hi;
    }
    value
}

let speed = 80.0;

func update(dt: f32) {
    pos.x = clamp(pos.x + speed * dt, -300.0, 300.0);
}
```
The two early exits use explicit `return ...;`; the final line, `value` with no semicolon and no `return`, is the implicit tail return - the case the rule exists for.

### 4. Deliberately broken - a type error (first diagnostics fixture)

```
func update(dt: f32) {
    let ready = true;
    pos.x += ready;
}
```
`pos.x += ready` adds a `bool` to an `f32` - the type checker's diagnostics test asserts exactly this error, at this span, and nothing else.

### 5. Deliberately broken - bad syntax (first parser-recovery fixture)

```
func update(dt: f32) {
    let speed = 100.0;
    pos.x += speed * dt;

    if pos.x > 200.0 {
        pos.x = -200.0;
    // missing closing brace, twice over
```
Proves ADR 0010's error tolerance concretely: the parser must recover enough to report something useful about the missing brace rather than cascading into unrelated garbage - and (once step 13 exists) editing distant unrelated code in the same file must not go blind because of this one error.

## Decisions carried in

- **ADR 0006** (static types, local inference, GC references for objects, value semantics for small structs, no ownership/borrowing/lifetimes).
- **ADR 0007** (lex -> parse -> check -> emit WASM directly via `wasm-encoder`, no optimizing middle-end; wasmtime/Cranelift execution; refcounting over a free-list allocator in linear memory, cycles leak until a later milestone). The "no optimizer" call is not a corner cut - compile time is on the interactive path twice over (every keystroke feeds this milestone's live diagnostics, every save is a hot-reload in M5), so comet's only job is emitting *correct* WASM as directly as possible; Cranelift already does the optimizing, once, at load time, for free.
- **ADR 0010** (the frontend is a reusable in-process language service - error-tolerant, incremental-capable - that atlas calls directly; no LSP between our own editor and our own language).
- **ADR 0016** (`Component` is a closed enum; each variant implements `Reflect` by hand; the inspector and serializer walk that one contract). `Script` is a new variant, built the same way `Sprite` was.
- **ADR 0009** (a Project is a directory of text files; scripts are plain `.cmt` source files alongside scenes).
- **ADR 0008 and full Play/hot-reload are Milestone 5's job**, not touched here.
- Every aurora-side gap this needs was already named in `docs/internals/ideas.md` under "Toward Comet (M4)" before this plan existed: styled runs, decorations, a monospace font, a gutter, find/replace, and an autocomplete popup with caret-relative anchoring.

## Ground truth checked before planning this

- `helios` has **no** per-frame update/tick concept anywhere today - `Scene::sprite_draws()` is a pure, stateless, timeless walk. "Call every Script's update function each frame" is new, and (per the headless-only decision) M4 does not need to build the general version of it - only enough to call one script once, from a test.
- `atlas::Pane` (`crates/atlas/src/dock.rs`) is a plain, data-less enum with the invariant "each variant appears exactly once across the whole dock tree." A new `Pane::Code` fits that shape directly as long as M4 only ever has one script open at a time (the same cut the ideatank already makes for "multiple open scenes") - no structural change to `Pane` or the dock is needed.
- aurora's multiline text area (`Style::multiline`, `WidgetKind::TextInput(String)`) already has real vertical scroll-to-caret, multi-line selection-as-ranges, and line-relative caret movement (`crates/aurora/src/ui.rs`). Selection is drawn as one `FillRect` per visual line from a byte-range span (`emit_multiline_selection`) - this is the exact mechanism to generalize for error/warning decorations, not a new one.
- Confirmed the thing that makes styled runs cheap: `DrawCommand::Text` carries one `Color` for a whole glyph run and `Glyph` carries none - so per-token color is impossible today - but the fix does not need to touch `draw.rs` or aurora-wgpu at all. `emit_text` can chunk a widget's shaped glyphs into multiple `DrawCommand::Text` runs, one per consecutive same-color span, the same coalesce-adjacent-runs pattern `crates/atlas/src/textures.rs` already uses for sprite batching.
- `wasm-encoder`/`wasmparser` (0.244.0) are already in the local cargo cache (unpacked on first build, no network needed). `wasmtime` and `wat` have never been fetched - no `cranelift*` anywhere in the registry - so the first `cargo build` that adds wasmtime will be a real, sizeable download. Worth doing as a throwaway `cargo add`/`cargo build`/revert early, so a big download doesn't stall the middle of a work session.

## Scope

**In:** the `comet` compiler (lexer, parser, type checker, WASM codegen) for a small language surface - `f32`, `bool`, a builtin `Vec2` value type with field access, `let`, `if`, `while`, functions with explicit parameter/return types, arithmetic/comparison/logical operators - plus a builtin `String` as the **one** heap-allocated, refcounted type, which is what actually exercises ADR 0007's allocator and is what the debug `print` host call needs. `comet::service`, the in-process language service (diagnostics on every edit; completions at a cursor position - keywords, in-scope locals, and `Vec2` field access). `helios`'s script host: a `Script` component (a project-relative `.cmt` path, reflected like `Sprite`), and the wasmtime glue that instantiates a compiled module and binds its imports to a real `Node`'s `Transform` (`get_position`/`set_position`) plus a `print` host call. Six aurora capabilities: a monospace font, styled text runs, decorations (error/warning underlines), a gutter (line numbers, current-line highlight, a per-line click target), a dropdown/list widget with keyboard navigation and caret-relative popup anchoring, and find/replace. A new `Pane::Code` in atlas wired to all of it, plus a minimal "Add Script" action (mirroring "Add Sprite") since M4 has only two component kinds and does not need a general Add Component browser yet.

**Out (and which milestone owns it):** **an optional ship-time optimization pass inside Comet itself**, distinct from and additional to Cranelift's JIT optimization - which already runs uniformly at module-load time in both atlas and a shipped game today, since ADR 0002 makes the editor embed the exact same runtime a shipped game uses. Nothing here forecloses adding a second, optimizing codegen path gated behind M6's Build & Export step (the same shape as a debug/release split), but building it now would be optimizing against a guess: there is no shipped game yet to profile, and guessing where the cost is instead of measuring it is precisely the mistake the M3.5 report spent five chapters undoing. Revisit at M6, and only if a real profile ever shows Cranelift's own pass is not enough. A Run/Play trigger of any kind, per-frame script ticking, input reaching a script, and voyager embedding -> M5. Hot reload and field migration (ADR 0008) -> M5. User-defined `struct` types, arrays, and any heap type beyond `String` -> a fast-follow once the single-refcounted-type case is proven, the same way M3's iteration phase followed M3 itself. A general Add Component browser -> whenever a third component kind arrives. Multiple simultaneously open scripts / script tabs -> whenever multiple open scenes are (the ideatank already defers both together). True incremental/differential re-parsing -> only if re-running the whole small-script pipeline on every keystroke ever turns out to be too slow to measure as instant, which is not expected given the whole pipeline is deliberately optimizer-free.

## Crate layout

- **`comet`**: `lexer`, `parser` (hand-written recursive descent, producing `ast`; error-tolerant with recovery, not just batch-or-fail, since the service needs it from day one per ADR 0010), `types` (the checker: local inference, the closed builtin type set, diagnostics), `codegen` (typed AST -> WASM bytes via `wasm-encoder`, including the emitted alloc/retain/release sequences around `String`), `service` (wraps the pipeline: `diagnostics(text) -> Vec<Diagnostic>`, `completions_at(text, offset) -> Vec<CompletionItem>`). Dependencies: `wasm-encoder`, `wasmparser` (validating emitted modules structurally in tests). `wasmtime` is a **dev-dependency only** - comet's own tests prove emitted code actually executes correctly against a trivial in-test host, but the compiler crate itself never links an execution engine into its public surface.
- **`helios`**: a new `script` module (per the module list `lib.rs`'s doc comment already names) owning the real `wasmtime::Engine`/`Store`, the `Script` component and its `Reflect` impl, and the host-import bindings from a compiled module to a live `Node`. Depends on `comet` (to compile a `.cmt` file's text into bytes) and `wasmtime` (real dependency, to run them).
- **aurora**: the six capabilities above land as aurora changes with no knowledge of Comet at all - they are generic editor capability, not language-specific, and (per the M3.6 report's rule - aurora grows when a second caller proves the need) already partly justified by wanting a general-purpose code editor rather than a Comet-specific one.
- **atlas**: `Pane::Code`, the build function for it, an "Add Script" action, and the plumbing that feeds a script's text through `comet::service` on every keystroke and turns the result into decorations/completions.

## Ordered steps

Grouped into five parts. Parts A and C (compiler core; aurora capabilities) do not depend on each other and can be built in either order or interleaved - they only meet in part E.

### Part A - the compiler core (headless, no UI, no Scene)

1. **Lexer, AST, and a hand-written parser** for the language surface in scope, with error recovery (skip-to-a-recovery-token) rather than stop-on-first-error, since the service needs that from the start. Tests parse fixture `.cmt` snippets - valid and intentionally broken - and assert the resulting AST/error list shape.
2. **The type checker**: local inference for `let`, explicit function signatures, the builtin `Vec2` (value type, field access) and `String` (the one reference type) with their fixed member sets, producing a `Vec<Diagnostic>` (span + message + severity) rather than failing outright. Tests cover well-typed fixtures (zero diagnostics) and a battery of intentional type errors with the expected span.
3. **WASM codegen**: `codegen` walks the typed AST and emits a module via `wasm-encoder` - arithmetic, control flow, function calls, `Vec2` field access lowered to locals, and the alloc/retain/release sequence around every `String` creation, assignment, and scope exit, backed by a free-list allocator emitted into the module's own linear memory (ADR 0007). Tests validate emitted bytes structurally with `wasmparser` (well-formed, correct export/import signatures).
4. **Execution proof, comet-internal**: a dev-dependency-only wasmtime harness that instantiates emitted modules against a trivial fake host (no `Scene` involved) and asserts the compiled code actually runs and produces the right numbers - including a `String`-using script, to prove the refcounting model works end to end before `helios` ever touches it. *Deliverable: the compiler is provably correct in isolation.*

### Part B - the script host and the real "moves a node" proof

5. **`helios::script`**: the `Script` component (a `.cmt` path) and its `Reflect` impl, mirroring `SpriteComponent` exactly; wired into `Component`'s closed enum, serialization, and an "Add Script" action in atlas alongside "Add Sprite."
6. **The real host bindings**: compile a node's script via `comet`, instantiate it on a real `wasmtime::Engine`, and bind `get_position`/`set_position` to that node's actual `Transform.position` in a live `Scene`, plus a `print` host call that reads a UTF-8 string out of the module's own exported memory. *Deliverable: this is the literal "moves a node" proof from the roadmap - a headless test that compiles a fixture script, calls its exported `update(dt)` against a real `Scene`, and asserts the `Node`'s `Transform` changed.*

### Part C - aurora: the code-editor capabilities (independent of Comet)

7. **A monospace font**: bundle DejaVu Sans Mono, extend the font-family selection alongside the existing bundled faces, and a `Style` flag to request it.
8. **Styled text runs**: chunk a widget's shaped glyphs into multiple same-color `DrawCommand::Text` runs instead of one - the coalesce-adjacent-runs pattern already used for sprite batching, applied to text. First job to verify during implementation: whether cosmic-text's `Attrs`/rich-text API already carries a per-span color aurora can key the chunking on, or whether the app needs to supply spans directly.
9. **Decorations**: generalize `emit_multiline_selection`'s per-visual-line range-highlighting into a marker an app can attach many of (byte range, color, underline vs. squiggle) - reusing the exact mechanism selection already proved, not a new one.
10. **The gutter**: line numbers and a current-line highlight from the same per-visual-line data `vscroll_of`/`emit_multiline_selection` already compute, plus a click-target-per-line that fires an event (breakpoints are a later payoff; the click target is the whole ask for now).
11. **A dropdown/list widget**: keyboard navigation (up/down/enter), filter-as-you-type, and caret-relative popup anchoring (the multiline caret's on-screen position is already computed for drawing the caret itself, so the anchor point exists). This is aurora's single biggest standing gap per the ideatank, and lands here because autocomplete needs it - not built speculatively.
12. **Find/replace**: a small find bar on the popup layer, anchored to the text area; match highlighting reuses step 9's decoration mechanism; replace reuses the existing text-splice editing path a paste already uses.

### Part D - the language service

13. **`comet::service` diagnostics**: `diagnostics(text) -> Vec<Diagnostic>`, re-running the whole part-A pipeline on every call. Deliberately NOT differential/incremental re-parsing for v1 - scripts are small and the pipeline has no optimizer, so a full batch re-run on every keystroke is expected to be fast enough to not need measuring first (the same "make it correct, then measure before you optimize" lesson the M3.5 report drew). Tests assert error tolerance concretely: one typo does not blank out diagnostics for the rest of the file.
14. **`comet::service` completions**: `completions_at(text, offset) -> Vec<CompletionItem>` - keywords always; locals in scope at that AST position; `Vec2`'s fixed field set once the receiver's type is known. No cross-file resolution - v1 scripts are single-file.

### Part E - wiring it into atlas

15. **`Pane::Code`**, a build function for it, and one new piece of persisted state - the currently open script's path (an `Option<PathBuf>`, the same shape as existing single-value editor state like `renaming`) - not a structural change to `Pane` or the dock, per the "one script open at a time" cut.
16. **Live wiring**: every edit re-runs `comet::service::diagnostics` and turns the result into part C's decorations (squiggles) and, via a small hand-written Comet token classifier, styled runs for syntax color; ctrl-space (or typing an identifier prefix) opens the autocomplete dropdown from `completions_at`; Save writes the buffer back to the `.cmt` file on disk through the existing file-write path. *Deliverable: the roadmap's other explicit ask - "the code editor shows live error squiggles as you type."*
17. **Gate and demo**: full workspace `fmt`/`clippy`/`test`/`doc`/ASCII gate; a demo script in the demo project exercising `Vec2`, `String`/print, and a type error on purpose (to show a squiggle); update the roadmap and ideatank the same way M1-M3 did on completion.

## Testing strategy

Headless-first, as every milestone so far: parts A, B, and D are entirely unit- and integration-tested with no window and no GPU - lexing, type errors, codegen validity, execution correctness, and diagnostic/completion correctness are all plain `cargo test`. Part C's rendering correctness (does a squiggle actually draw, does the dropdown actually anchor at the caret) is not headlessly assertable - aurora and atlas still have no golden-image tests (a standing gap both the ideatank and the M3.5/M3.6 reports name) - so those get manual verification against a running editor, the same `#[ignore]`d-GPU-test-plus-eyeball pattern M1-M3 used for anything visual. `clippy -D warnings`, `fmt --check`, and the ASCII scan gate every step.

Two new ADRs are worth writing during implementation rather than guessing their content here, the way ADR 0019 was written mid-milestone-3: the exact WASM host-binding ABI (how a script's magic `pos`-style accessors lower to imports, and why `String` alone is v1's heap type) once part A/B's shape is real, and the styled-run rendering approach once part C step 8's cosmic-text question is answered.

## Verification

- `cargo test --workspace` green throughout, with real new coverage in `comet` (lexer/parser/checker/codegen fixtures, the dev-only wasmtime execution proof) and `helios` (the end-to-end "moves a node" test against a real `Scene` - this is the literal roadmap proof point and should be quotable by name).
- `cargo clippy --workspace --all-targets`, `cargo fmt --all --check`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`, and the ASCII scan clean at every step, matching the project's standing gate.
- Manual: open the demo project's script in the new Code pane, type a deliberate type error and see a squiggle appear live, fix it and see it clear, trigger autocomplete and pick a completion, use find/replace - screenshot or describe each for the record, since none of it is headlessly provable.
- `cargo build` once early with `wasmtime` added (even as a throwaway `cargo add` reverted after) to pull the large first-time download out of the middle of a work session.
