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

use wasmtime::{Caller, Engine, Extern, Instance, Linker, Module, Store};

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

    let bytes = match comet::compile(&source) {
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

#[derive(Default)]
struct Host {
    position: (f32, f32),
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
                printed: Vec::new(),
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
