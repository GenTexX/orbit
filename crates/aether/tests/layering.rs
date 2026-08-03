//! The architecture, as a check rather than as a comment.
//!
//! Four manifests and one module doc state what a crate must not depend on:
//! voyager depends on helios and comet and nothing else, "photon stays out";
//! comet "must not link an execution engine into its public surface"; spectrum
//! has "no GUI dependency"; and aether's own doc says "wgpu stays confined to
//! the two rendering backends and this bootstrap".
//!
//! All of it was prose. This is the milestone where it matters most: M6 adds a
//! window and a renderer to the shipping path, which is exactly the change that
//! would violate the layering by accident.
//!
//! It lives in aether because aether is the crate the rule is about and the one
//! with least else to do; the check itself reads manifests and sources and needs
//! no dependencies at all.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("aether sits two levels under the workspace root")
        .to_path_buf()
}

/// The crates a manifest names as dependencies, of any kind.
fn dependencies(crate_name: &str) -> HashSet<String> {
    let manifest = workspace()
        .join("crates")
        .join(crate_name)
        .join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("reading {}: {e}", manifest.display()));
    let mut found = HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with('[') || !line.contains('=') {
            continue;
        }
        if let Some(name) = line.split(['=', ' ']).next()
            && !name.is_empty()
        {
            found.insert(name.trim().to_string());
        }
    }
    found
}

/// Whether any `.rs` under a crate's `src` mentions `needle`.
fn mentions(crate_name: &str, needle: &str) -> bool {
    fn walk(dir: &Path, needle: &str) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if walk(&path, needle) {
                    return true;
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
                && std::fs::read_to_string(&path).is_ok_and(|text| text.contains(needle))
            {
                return true;
            }
        }
        false
    }
    walk(
        &workspace().join("crates").join(crate_name).join("src"),
        needle,
    )
}

#[test]
fn wgpu_stays_in_the_crates_that_are_allowed_to_touch_it() {
    // ADR 0018: the GPU lives in the two rendering backends and this bootstrap.
    // Everything else - the engine, the scene model, the language, the theming
    // crate - is meant to be testable without a device, which is what makes
    // most of this workspace's tests possible at all.
    for crate_name in ["aurora", "helios", "comet", "spectrum", "voyager"] {
        assert!(
            !mentions(crate_name, "wgpu::"),
            "{crate_name} names wgpu, which belongs to photon, aurora-wgpu and aether"
        );
    }
}

#[test]
fn the_runtime_library_does_not_depend_on_a_renderer() {
    // voyager's own manifest says photon stays out, because stepping a scene is
    // not drawing one - and because a bin shares its package's dependencies, so
    // giving the shipping wrapper a window would give the library one too.
    let deps = dependencies("voyager");
    for forbidden in ["photon", "aurora", "aurora-wgpu", "winit", "wgpu", "aether"] {
        assert!(
            !deps.contains(forbidden),
            "voyager depends on {forbidden}, which stops the runtime library being headless"
        );
    }
}

#[test]
fn the_compiler_does_not_ship_an_execution_engine() {
    // comet writes WebAssembly and never runs it; owning the engine is helios's
    // job (ADR 0002 and 0007). wasmtime is a dev-dependency there for the tests
    // that execute what it emitted, which is a different thing.
    let text = std::fs::read_to_string(workspace().join("crates/comet/Cargo.toml"))
        .expect("comet has a manifest");
    let (before, after) = text
        .split_once("[dev-dependencies]")
        .expect("comet declares dev-dependencies");
    assert!(
        !before.contains("wasmtime"),
        "wasmtime is in comet's public dependencies"
    );
    assert!(
        after.contains("wasmtime"),
        "the execution tests still need wasmtime"
    );
}

#[test]
fn the_theming_crate_has_no_gui_dependency() {
    // spectrum is a model two applications share, so it must not reach for the
    // toolkit either of them happens to use.
    let deps = dependencies("spectrum");
    for forbidden in ["aurora", "aurora-wgpu", "winit", "helios", "photon"] {
        assert!(!deps.contains(forbidden), "spectrum depends on {forbidden}");
    }
}

#[test]
fn the_gui_framework_does_not_touch_the_filesystem() {
    // aurora consumes input and produces draw lists. Anything that reads a file
    // or decodes an image stays in the application, which is the boundary that
    // decided what could be extracted into it and what could not.
    assert!(
        !mentions("aurora", "std::fs"),
        "aurora reads the filesystem"
    );
    assert!(
        !mentions("comet", "std::fs"),
        "the compiler reads the filesystem"
    );
}
