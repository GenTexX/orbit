//! atlas recovery: the last known good buffer and scene, written out if the editor dies.
//!
//! ADR 0002 puts the compiler, the language service, wasmtime and the renderer
//! in the editor's own process, with the user's unsaved work beside them. That
//! is the right trade for an in-process Play button, and it has a cost that was
//! never paid: any panic anywhere - and the compiler runs on the live buffer on
//! every keystroke - took the session with it. There was no panic hook, no
//! autosave, and no recovery file anywhere in the workspace.
//!
//! This does not make anything not crash. It makes a crash cost a restart
//! instead of an afternoon, which is the difference between the bugs found on
//! 2026-08-03 being annoying and being catastrophic.
//!
//! Two rules shape the implementation. It records at *edit* rate, not frame
//! rate - a `Scene` clone per undoable edit, a `String` clone per text change -
//! so nothing is paid on an idle frame. And the hook itself never blocks: it
//! takes the lock with `try_lock` and gives up rather than deadlocking against
//! whatever was holding it when the panic happened.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use helios::Scene;

/// What would be written out if the process died right now.
#[derive(Debug, Default)]
struct Recovery {
    project_dir: PathBuf,
    /// The open script's project-relative path, and its current text.
    script: Option<(String, String)>,
    /// The scene as of the last undoable edit.
    scene: Option<Scene>,
}

static STATE: Mutex<Option<Recovery>> = Mutex::new(None);

/// Start recording, and arrange for a panic to write what was recorded.
///
/// Called once, before the window exists, so a panic during startup is covered
/// too - it simply has nothing to write yet.
pub fn install(project_dir: &Path) {
    if let Ok(mut guard) = STATE.lock() {
        *guard = Some(Recovery {
            project_dir: project_dir.to_path_buf(),
            ..Recovery::default()
        });
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The default hook first: the message and the backtrace are what makes
        // the crash reportable, and dumping must not be able to swallow them.
        previous(info);
        match dump() {
            Ok(Some(dir)) => eprintln!("atlas: unsaved work written to {}", dir.display()),
            Ok(None) => {}
            Err(err) => eprintln!("atlas: could not write recovery files: {err}"),
        }
    }));
}

/// Record the open script's text. Cheap enough to call whenever it changes,
/// which is what `sync_script_buffer` already detects.
pub fn record_script(path: &str, text: &str) {
    if let Ok(mut guard) = STATE.lock()
        && let Some(state) = guard.as_mut()
    {
        state.script = Some((path.to_string(), text.to_string()));
    }
}

/// Forget the open script - it was saved, or closed. Keeps a stale buffer from
/// being restored over a file that is already correct on disk.
pub fn forget_script() {
    if let Ok(mut guard) = STATE.lock()
        && let Some(state) = guard.as_mut()
    {
        state.script = None;
    }
}

/// Record the scene. Called when the edit history moves, not per frame.
pub fn record_scene(scene: &Scene) {
    if let Ok(mut guard) = STATE.lock()
        && let Some(state) = guard.as_mut()
    {
        state.scene = Some(scene.clone());
    }
}

/// Forget the scene - it was saved, so the file on disk is the better copy.
pub fn forget_scene() {
    if let Ok(mut guard) = STATE.lock()
        && let Some(state) = guard.as_mut()
    {
        state.scene = None;
    }
}

/// Write whatever is recorded into `<project>/.orbit/recovered/<stamp>/`.
///
/// Returns the directory written, or `None` when there was nothing to write.
/// Separated from the hook so a test can drive it.
fn dump() -> std::io::Result<Option<PathBuf>> {
    // try_lock, not lock: if the panic happened while this lock was held, the
    // hook must not hang the process on the way out. Losing the dump is bad;
    // a wedged editor that cannot even print its backtrace is worse.
    let Ok(guard) = STATE.try_lock() else {
        return Ok(None);
    };
    let Some(state) = guard.as_ref() else {
        return Ok(None);
    };
    if state.script.is_none() && state.scene.is_none() {
        return Ok(None);
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = state
        .project_dir
        .join(".orbit")
        .join("recovered")
        .join(stamp.to_string());
    std::fs::create_dir_all(&dir)?;
    if let Some((path, text)) = &state.script {
        // The file name only, flattened: a recovery directory is a pile of
        // things to look at, not a tree to navigate, and two scripts with the
        // same name in different folders is rarer than one directory deep
        // enough to be annoying.
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled.cmt".to_string());
        std::fs::write(dir.join(name), text)?;
    }
    if let Some(scene) = &state.scene
        && let Ok(ron) = scene.to_ron()
    {
        std::fs::write(dir.join("scene.ron"), ron)?;
    }
    Ok(Some(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios::{Node, Scene};

    #[test]
    fn a_dump_writes_the_buffer_and_the_scene_it_was_given() {
        let temp = std::env::temp_dir().join(format!(
            "atlas-recovery-{}",
            std::process::id() as u64 * 7 + 1
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        // Not through install(): setting a panic hook is process-global and a
        // test that does it changes how every other test reports a failure.
        let mut scene = Scene::new("root");
        scene.add_child(scene.root(), Node::new("kept"));
        {
            let mut guard = STATE.lock().unwrap();
            *guard = Some(Recovery {
                project_dir: temp.clone(),
                script: Some((
                    "scripts/bounce.cmt".into(),
                    "func update(dt: f32) {}".into(),
                )),
                scene: Some(scene),
            });
        }

        let dir = dump().unwrap().expect("something to write");
        assert_eq!(
            std::fs::read_to_string(dir.join("bounce.cmt")).unwrap(),
            "func update(dt: f32) {}"
        );
        let ron = std::fs::read_to_string(dir.join("scene.ron")).unwrap();
        assert!(ron.contains("kept"), "the scene went out too: {ron}");

        // Nothing recorded, nothing written - a clean session leaves no litter.
        {
            let mut guard = STATE.lock().unwrap();
            *guard = Some(Recovery {
                project_dir: temp.clone(),
                ..Recovery::default()
            });
        }
        assert!(dump().unwrap().is_none());

        let _ = std::fs::remove_dir_all(&temp);
    }
}
