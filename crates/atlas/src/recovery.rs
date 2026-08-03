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
/// What a previous run left behind, newest first.
///
/// The dump was a floor nobody could stand on: the editor wrote the files and
/// then never mentioned them again, so recovering meant knowing that
/// `.orbit/recovered/` exists and going to look. Nothing cleaned them up
/// either, so a project that had crashed twice had two directories and no way
/// to tell which was which without reading timestamps.
pub fn recovered(project_dir: &Path) -> Vec<Recovered> {
    let root = project_dir.join(".orbit").join("recovered");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut found: Vec<Recovered> = entries
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            let stamp: u64 = dir.file_name()?.to_str()?.parse().ok()?;
            let mut script = None;
            let mut scene = None;
            for file in std::fs::read_dir(&dir).ok()?.flatten() {
                let path = file.path();
                match path.extension().and_then(|e| e.to_str()) {
                    Some("cmt") => script = Some(path),
                    Some("ron") => scene = Some(path),
                    _ => {}
                }
            }
            (script.is_some() || scene.is_some()).then_some(Recovered {
                dir,
                stamp,
                script,
                scene,
            })
        })
        .collect();
    found.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    found
}

/// One crash's worth of unsaved work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    pub dir: PathBuf,
    /// Seconds since the epoch, which is what the directory is named.
    pub stamp: u64,
    pub script: Option<PathBuf>,
    pub scene: Option<PathBuf>,
}

impl Recovered {
    /// What to say about it, in the words somebody who has just reopened a
    /// crashed editor needs.
    pub fn summary(&self) -> String {
        match (&self.script, &self.scene) {
            (Some(script), Some(_)) => format!(
                "a scene and {}",
                script.file_name().unwrap_or_default().to_string_lossy()
            ),
            (Some(script), None) => script
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            (None, Some(_)) => "a scene".to_string(),
            (None, None) => "nothing".to_string(),
        }
    }
}

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

    #[test]
    fn a_recovery_directory_is_found_and_described() {
        // The dump was a floor nobody could stand on: the files were written
        // and never mentioned again, so recovering meant knowing that
        // `.orbit/recovered/` exists and going to look.
        let dir = tempfile::tempdir().expect("a temp dir");
        let one = dir.path().join(".orbit/recovered/1000");
        std::fs::create_dir_all(&one).expect("the folder");
        std::fs::write(one.join("player.cmt"), "func update(dt: f32) { }").expect("a script");
        std::fs::write(one.join("scene.ron"), "()").expect("a scene");

        let found = recovered(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].stamp, 1000);
        assert_eq!(found[0].summary(), "a scene and player.cmt");
    }

    #[test]
    fn recoveries_come_back_newest_first() {
        let dir = tempfile::tempdir().expect("a temp dir");
        for stamp in ["10", "300", "20"] {
            let at = dir.path().join(".orbit/recovered").join(stamp);
            std::fs::create_dir_all(&at).expect("the folder");
            std::fs::write(at.join("a.cmt"), "x").expect("a script");
        }
        let stamps: Vec<u64> = recovered(dir.path()).iter().map(|r| r.stamp).collect();
        assert_eq!(stamps, [300, 20, 10]);
    }

    #[test]
    fn an_empty_directory_is_not_offered() {
        // A dump that wrote nothing must not produce an offer to restore
        // nothing.
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::create_dir_all(dir.path().join(".orbit/recovered/1")).expect("the folder");
        assert!(recovered(dir.path()).is_empty());
        // Nor does a project that has never crashed.
        let clean = tempfile::tempdir().expect("a temp dir");
        assert!(recovered(clean.path()).is_empty());
    }
}
