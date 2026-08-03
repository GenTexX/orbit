//! Run a `.cmt` script and watch what it does.
//!
//! ```text
//! cargo run -p comet --example run -- crates/comet/tests/fixtures/bounce.cmt
//! ```
//!
//! This is a toy host, not the engine: it owns a single position, calls the
//! script's `update(dt)` in a loop, and draws where the node ended up on an
//! ASCII track. The real host - the one that moves an actual `Node` in a real
//! `Scene` - is helios's job in part B of milestone 4, and the editor's Play
//! button is milestone 5. Until then this is the way to feel the language.
//!
//! Options (all optional, shown with their defaults):
//!
//! ```text
//! --steps 120     how many times to call update
//! --dt 0.05       seconds per step
//! --at 0,0        where the node starts
//! --range 320     the half-width of the track, in world units
//! --width 78      the track's width, in characters
//! --delay 30      milliseconds between steps, so it animates
//! --axis x        which axis the track follows (x or y)
//! ```

use std::time::Duration;

use wasmtime::{Caller, Engine, Error, Extern, Instance, Linker, Module, Store};

fn main() {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("usage: cargo run -p comet --example run -- <script.cmt> [options]");
            std::process::exit(2);
        }
    };

    let source = match std::fs::read_to_string(&options.path) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", options.path);
            std::process::exit(2);
        }
    };

    let bytes = match comet::compile(&source, &comet::example_schema()) {
        Ok(bytes) => bytes,
        Err(diagnostics) => {
            for diagnostic in &diagnostics {
                report(&options.path, &source, diagnostic);
            }
            let count = diagnostics.len();
            eprintln!(
                "{count} error{} - nothing to run",
                if count == 1 { "" } else { "s" }
            );
            std::process::exit(1);
        }
    };

    println!(
        "compiled {} to {} bytes of WebAssembly",
        options.path,
        bytes.len()
    );

    let mut script = Runner::new(&bytes, options.start);
    let Some(update) = script.update_fn() else {
        println!("this script has no `update(dt: f32)`, so there is nothing to run per frame.");
        println!("its state was still initialized at instantiation.");
        return;
    };

    println!(
        "running update(dt = {}) for {} steps\n",
        options.dt, options.steps
    );

    for step in 0..options.steps {
        if let Err(e) = update.call(&mut script.store, options.dt) {
            eprintln!("\ntrapped on step {step}: {e}");
            std::process::exit(1);
        }
        let (x, y) = script.store.data().position;
        let value = if options.axis_y { y } else { x };
        println!(
            "{:>4} {} {:>10.2}   ({:.2}, {:.2})",
            step,
            track(value, options.range, options.width),
            value,
            x,
            y
        );
        for line in script.store.data_mut().printed.drain(..) {
            println!("       print: {line}");
        }
        if options.delay > 0 {
            std::thread::sleep(Duration::from_millis(options.delay));
        }
    }
}

/// Draw one frame of the track: a `|` at each end, `+` at the origin, `o` where
/// the node is, and `<` or `>` when it has run off the end.
fn track(value: f32, range: f32, width: usize) -> String {
    let mut cells = vec![b' '; width];
    let origin = width / 2;
    cells[origin] = b'+';

    let normalized = (value / range + 1.0) / 2.0;
    let column = (normalized * (width - 1) as f32).round();
    let mut line = String::with_capacity(width + 2);
    line.push('|');
    if column < 0.0 {
        cells[0] = b'<';
    } else if column > (width - 1) as f32 {
        cells[width - 1] = b'>';
    } else {
        cells[column as usize] = b'o';
    }
    line.push_str(std::str::from_utf8(&cells).expect("ascii only"));
    line.push('|');
    line
}

/// Print a diagnostic the way an editor would draw it, with the offending source
/// line underneath and the span underlined.
fn report(path: &str, source: &str, diagnostic: &comet::Diagnostic) {
    let (line_number, column, line) = locate(source, diagnostic.span.start as usize);
    let label = match diagnostic.severity {
        comet::Severity::Error => "error",
        comet::Severity::Warning => "warning",
    };
    let width = (diagnostic.span.end - diagnostic.span.start).max(1) as usize;
    let gutter = " ".repeat(line_number.to_string().len());

    eprintln!("{label}: {}", diagnostic.message);
    eprintln!("{gutter}--> {path}:{line_number}:{column}");
    eprintln!("{gutter} |");
    eprintln!("{line_number} | {line}");
    eprintln!("{gutter} | {}{}", " ".repeat(column - 1), "^".repeat(width));
    eprintln!();
}

/// Turn a byte offset into a 1-based line and column, plus that whole line.
fn locate(source: &str, offset: usize) -> (usize, usize, &str) {
    let offset = offset.min(source.len());
    let start = source[..offset].rfind('\n').map_or(0, |at| at + 1);
    let end = source[offset..]
        .find('\n')
        .map_or(source.len(), |at| offset + at);
    let line_number = source[..start].matches('\n').count() + 1;
    (line_number, offset - start + 1, &source[start..end])
}

// --- the toy host ---

/// The property ids `comet::example_schema()` assigns, in its declaration
/// order. The same table the execution tests use.
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

struct Runner {
    store: Store<Host>,
    instance: Instance,
}

impl Runner {
    fn new(bytes: &[u8], start: (f32, f32)) -> Self {
        let engine = Engine::default();
        let module = Module::new(&engine, bytes).expect("comet emits valid modules");
        let mut linker = Linker::new(&engine);
        let host = comet::HOST_MODULE;

        // The property accessors ADR 0020 replaced the old
        // get_position_x/get_position_y/set_position trio with. Every one takes
        // the property's schema id, so this table is the same size whatever the
        // schema says - which is the point of numbering properties rather than
        // importing one function per property.
        //
        // The ids are the declaration order of `comet::example_schema()`. A real
        // host derives them by walking the schema; an example is allowed to know
        // them, and this way the example is a readable model of what a host does.
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
                    // The one host function that allocates. It calls back into the
                    // module rather than reserving memory of its own, so what it
                    // returns is an ordinary refcounted block the script releases
                    // like any other String.
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
            .func_wrap(
                host,
                "str_int",
                |mut c: Caller<'_, Host>, value: i32| -> Result<i32, Error> {
                    // Whole numbers print whole. Widening to f32 first would round
                    // past 2^24, silently, in the one function used to see a value.
                    let text = value.to_string();
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
                        let at = ptr as usize;
                        String::from_utf8_lossy(&data[at..at + len as usize]).into_owned()
                    };
                    c.data_mut().printed.push(text);
                },
            )
            .expect("host binding");

        let mut store = Store::new(
            &engine,
            Host {
                position: start,
                scale: (1.0, 1.0),
                ..Host::default()
            },
        );
        let instance = linker
            .instantiate(&mut store, &module)
            .expect("instantiation runs the script's state initializers");
        Self { store, instance }
    }

    fn update_fn(&mut self) -> Option<wasmtime::TypedFunc<f32, ()>> {
        self.instance
            .get_typed_func::<f32, ()>(&mut self.store, "update")
            .ok()
    }
}

// --- arguments ---

struct Options {
    path: String,
    steps: u32,
    dt: f32,
    start: (f32, f32),
    range: f32,
    width: usize,
    delay: u64,
    axis_y: bool,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Options {
            path: String::new(),
            steps: 120,
            dt: 0.05,
            start: (0.0, 0.0),
            range: 320.0,
            width: 78,
            delay: 30,
            axis_y: false,
        };
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("`{arg}` needs a value"));
            match arg.as_str() {
                "--steps" => options.steps = parse(&value()?)?,
                "--dt" => options.dt = parse(&value()?)?,
                "--range" => options.range = parse(&value()?)?,
                "--width" => options.width = parse::<usize>(&value()?)?.clamp(8, 400),
                "--delay" => options.delay = parse(&value()?)?,
                "--axis" => options.axis_y = value()?.eq_ignore_ascii_case("y"),
                "--at" => {
                    let text = value()?;
                    let (x, y) = text
                        .split_once(',')
                        .ok_or_else(|| format!("`--at` wants `x,y`, got `{text}`"))?;
                    options.start = (parse(x.trim())?, parse(y.trim())?);
                }
                other if other.starts_with("--") => {
                    return Err(format!("unknown option `{other}`"));
                }
                other => options.path = other.to_string(),
            }
        }
        if options.path.is_empty() {
            return Err("no script given".to_string());
        }
        Ok(options)
    }
}

fn parse<T: std::str::FromStr>(text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("`{text}` is not a valid value"))
}
