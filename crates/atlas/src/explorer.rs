//! atlas file explorer model: the project directory scanned into a tree of
//! entries, plus the view state around it - which folders are expanded in the
//! folder tree, which folder's contents fill the right pane, the selected file,
//! and the view mode (compact list vs. large previews).
//!
//! This is headless and unit-tested; `ui.rs` renders it and `main.rs` drives it
//! from pointer events. Like the collapsed-tree-node state, the persisted parts
//! (expanded folders, selected folder) travel as project-relative PATHS, not
//! anything tied to a particular scan, so they survive a rescan or a restart.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// Whether a scanned entry is a directory or a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
}

/// One node in the scanned project tree: a file, or a directory with children.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Absolute path on disk.
    pub path: PathBuf,
    /// The file or directory name (the last path component), for display.
    pub name: String,
    pub kind: EntryKind,
    /// Child entries, directories first then files, each group alphabetical
    /// (case-insensitive). Always empty for a file.
    pub children: Vec<Entry>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }
}

/// A coarse file classification, for the icon a file gets and whether it can be
/// previewed or dragged into the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// A raster image (png/jpg/...): shown with the image icon, dragged to spawn.
    Image,
    /// A scene file (`.ron`).
    Scene,
    /// Anything else.
    Other,
}

/// Classify a path by its extension (case-insensitive).
pub fn classify(path: &Path) -> FileKind {
    match extension(path).as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp") => FileKind::Image,
        Some("ron") => FileKind::Scene,
        _ => FileKind::Other,
    }
}

/// Whether a thumbnail can be decoded for this path. Only PNG is decodable here
/// (atlas builds the `image` crate with just the png feature), so other image
/// kinds fall back to the [`FileKind::Image`] icon without a preview.
pub fn can_thumbnail(path: &Path) -> bool {
    extension(path).as_deref() == Some("png")
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// Directories skipped by the scan on top of every dotfile (which is skipped
/// too): build output and dependency caches that are never project content.
const IGNORED_DIRS: &[&str] = &["target", "node_modules"];

/// A hard cap on how deep the recursive scan descends, so a pathologically deep
/// real directory tree cannot overflow the stack. Far deeper than any real
/// project layout; a directory past it is shown but its contents are not walked.
const MAX_SCAN_DEPTH: usize = 64;

/// Scan `dir` into a sorted list of entries (recursively for subdirectories,
/// bounded by [`MAX_SCAN_DEPTH`]). Unreadable directories (permissions, races)
/// yield an empty list rather than an error - the explorer shows what it can.
fn scan_dir(dir: &Path, depth: usize) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for dirent in read.flatten() {
        let name = dirent.file_name().to_string_lossy().into_owned();
        // Hidden files and dirs (dotfiles: .orbit, .git, ...) are never shown.
        if name.starts_with('.') {
            continue;
        }
        let path = dirent.path();
        let is_dir = dirent.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if IGNORED_DIRS.contains(&name.as_str()) {
                continue;
            }
            // Recurse, unless we have hit the depth cap (then the folder is
            // listed but shown empty rather than descending further).
            let children = if depth + 1 < MAX_SCAN_DEPTH {
                scan_dir(&path, depth + 1)
            } else {
                Vec::new()
            };
            entries.push(Entry {
                path,
                name,
                kind: EntryKind::Dir,
                children,
            });
        } else {
            entries.push(Entry {
                path,
                name,
                kind: EntryKind::File,
                children: Vec::new(),
            });
        }
    }
    sort_entries(&mut entries);
    entries
}

/// Directories first, then files; each group alphabetical, case-insensitive.
fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_dir().cmp(&a.is_dir()).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
}

/// The explorer's view mode for the contents pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum FileView {
    /// A dense single-column list, each row a small icon or thumbnail + name.
    #[default]
    List,
    /// A grid of large thumbnail/icon cells (the preview view).
    Grid,
}

/// One folder row in the tree view (directories only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub path: PathBuf,
    pub name: String,
    /// Indentation depth (0 = the project root).
    pub depth: usize,
    /// Whether this folder has any sub-folders (so it gets a disclosure arrow).
    pub has_children: bool,
    /// Whether it is currently expanded.
    pub expanded: bool,
    /// Whether it is the folder whose contents are shown.
    pub selected: bool,
}

/// The file explorer's model and view state.
pub struct FileExplorer {
    /// The project root as an entry (its `children` are the scanned tree). Held
    /// whole so contents lookups and the folder tree derive from one place.
    root: Entry,
    /// Directories expanded in the folder tree (absolute paths).
    expanded: HashSet<PathBuf>,
    /// The directory whose contents fill the right pane (defaults to the root).
    selected_dir: PathBuf,
    /// The highlighted file in the contents pane, if any.
    selected_file: Option<PathBuf>,
    /// Compact list vs. large-preview grid.
    view: FileView,
    /// The folder tree's scroll offset, kept here (not in the per-pane scroll
    /// map, which the pane spends on its contents area) so it survives the shell
    /// rebuilds that a selection or a landing thumbnail trigger. Transient - not
    /// persisted across restarts.
    tree_scroll: f32,
}

impl FileExplorer {
    /// Scan `root` and open the explorer at it (root expanded, its own contents
    /// shown, list view).
    pub fn new(root: &Path) -> Self {
        let root = root_entry(root);
        let root_path = root.path.clone();
        let mut expanded = HashSet::new();
        expanded.insert(root_path.clone());
        Self {
            root,
            expanded,
            selected_dir: root_path,
            selected_file: None,
            view: FileView::default(),
            tree_scroll: 0.0,
        }
    }

    /// The folder tree's saved scroll offset (restored into the tree panel on
    /// each rebuild).
    pub fn tree_scroll(&self) -> f32 {
        self.tree_scroll
    }

    /// Remember the folder tree's current scroll offset (captured each frame).
    pub fn set_tree_scroll(&mut self, offset: f32) {
        self.tree_scroll = offset;
    }

    /// Re-read the project from disk, then drop any expanded/selected paths that
    /// no longer resolve to a directory (files or folders deleted since).
    pub fn rescan(&mut self) {
        self.root.children = scan_dir(&self.root.path, 0);
        let root_path = self.root.path.clone();
        // Collect the now-stale expanded paths (immutable borrow via `find`),
        // then drop them (mutable borrow) - the two cannot overlap.
        let stale: Vec<PathBuf> = self
            .expanded
            .iter()
            .filter(|p| **p != root_path && !self.find(p).is_some_and(Entry::is_dir))
            .cloned()
            .collect();
        for p in stale {
            self.expanded.remove(&p);
        }
        if self.find(&self.selected_dir).is_none_or(|e| !e.is_dir()) {
            self.selected_dir = root_path;
        }
        if self
            .selected_file
            .as_ref()
            .is_some_and(|p| self.find(p).is_none())
        {
            self.selected_file = None;
        }
    }

    /// The folder tree, flattened to rows in display order (root first, then
    /// each expanded folder's sub-folders, depth-first). Files never appear.
    pub fn tree_rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.push_tree_rows(&self.root, 0, &mut rows);
        rows
    }

    fn push_tree_rows(&self, dir: &Entry, depth: usize, out: &mut Vec<TreeRow>) {
        let subdirs: Vec<&Entry> = dir.children.iter().filter(|c| c.is_dir()).collect();
        let expanded = self.expanded.contains(&dir.path);
        out.push(TreeRow {
            path: dir.path.clone(),
            name: dir.name.clone(),
            depth,
            has_children: !subdirs.is_empty(),
            expanded,
            selected: dir.path == self.selected_dir,
        });
        if expanded {
            for sub in subdirs {
                self.push_tree_rows(sub, depth + 1, out);
            }
        }
    }

    /// The entries (sub-folders and files) of the currently selected folder.
    pub fn contents(&self) -> &[Entry] {
        self.find(&self.selected_dir)
            .map(|e| e.children.as_slice())
            .unwrap_or(&[])
    }

    /// The path from the root down to the selected folder as clickable crumbs
    /// `(name, path)`, root first.
    pub fn breadcrumbs(&self) -> Vec<(String, PathBuf)> {
        let mut out = vec![(self.root.name.clone(), self.root.path.clone())];
        if let Ok(rel) = self.selected_dir.strip_prefix(&self.root.path) {
            let mut acc = self.root.path.clone();
            for comp in rel.components() {
                if let Component::Normal(c) = comp {
                    acc.push(c);
                    out.push((c.to_string_lossy().into_owned(), acc.clone()));
                }
            }
        }
        out
    }

    #[allow(dead_code, reason = "read by tests; part of the explorer's surface")]
    pub fn selected_dir(&self) -> &Path {
        &self.selected_dir
    }

    pub fn is_selected_file(&self, path: &Path) -> bool {
        self.selected_file.as_deref() == Some(path)
    }

    pub fn view(&self) -> FileView {
        self.view
    }

    pub fn set_view(&mut self, view: FileView) {
        self.view = view;
    }

    /// The path relative to the project root, `/`-separated, or `None` if `path`
    /// is not under the root. Used for the drag payload and for persistence.
    pub fn project_relative(&self, path: &Path) -> Option<String> {
        rel(&self.root.path, path)
    }

    /// Toggle a folder open/closed in the tree.
    pub fn toggle_expand(&mut self, path: &Path) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
        }
    }

    /// Show `dir`'s contents in the right pane, revealing it in the tree by
    /// expanding its ancestors (not the folder itself - the user opens that with
    /// its disclosure arrow). A no-op if `dir` is not a directory in the tree.
    pub fn select_dir(&mut self, dir: &Path) {
        if self.find(dir).is_none_or(|e| !e.is_dir()) {
            return;
        }
        self.selected_dir = dir.to_path_buf();
        self.selected_file = None;
        self.expand_ancestors(dir);
    }

    /// Highlight a file in the contents pane.
    pub fn select_file(&mut self, path: &Path) {
        self.selected_file = Some(path.to_path_buf());
    }

    /// Expand every ancestor of `path` up to and including the root, so `path`
    /// is visible in the tree.
    fn expand_ancestors(&mut self, path: &Path) {
        let Ok(rel) = path.strip_prefix(&self.root.path) else {
            return;
        };
        let mut acc = self.root.path.clone();
        self.expanded.insert(acc.clone());
        for comp in rel.components() {
            if let Component::Normal(c) = comp {
                acc.push(c);
                // Stop before `path` itself: reveal it, do not open it.
                if acc == path {
                    break;
                }
                self.expanded.insert(acc.clone());
            }
        }
    }

    /// Find the entry at an absolute path by walking down from the root, or
    /// `None` if nothing there (or the path is outside the project).
    fn find(&self, path: &Path) -> Option<&Entry> {
        if path == self.root.path {
            return Some(&self.root);
        }
        let rel = path.strip_prefix(&self.root.path).ok()?;
        let mut cur = &self.root;
        for comp in rel.components() {
            let Component::Normal(name) = comp else {
                return None;
            };
            cur = cur
                .children
                .iter()
                .find(|c| c.name == *name.to_string_lossy())?;
        }
        Some(cur)
    }

    // ---- persistence (project-relative paths, like the collapsed tree nodes) ----

    /// Expanded folders as project-relative paths (the root is implicit and
    /// always expanded, so it is not stored).
    pub fn expanded_rel(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .expanded
            .iter()
            .filter(|p| **p != self.root.path)
            .filter_map(|p| rel(&self.root.path, p))
            .collect();
        out.sort();
        out
    }

    /// The selected folder as a project-relative path, or `None` at the root.
    pub fn selected_rel(&self) -> Option<String> {
        if self.selected_dir == self.root.path {
            None
        } else {
            rel(&self.root.path, &self.selected_dir)
        }
    }

    /// Restore persisted view state, resolving relative paths against the
    /// current scan and dropping any that no longer point at a directory.
    pub fn restore(&mut self, expanded: &[String], selected: Option<&str>, view: FileView) {
        self.view = view;
        for r in expanded {
            let path = resolve(&self.root.path, r);
            if self.find(&path).is_some_and(Entry::is_dir) {
                self.expanded.insert(path);
            }
        }
        if let Some(r) = selected {
            let path = resolve(&self.root.path, r);
            if self.find(&path).is_some_and(Entry::is_dir) {
                self.selected_dir = path.clone();
                self.expand_ancestors(&path);
            }
        }
    }
}

/// Build the root entry (a directory) with its scanned children.
fn root_entry(root: &Path) -> Entry {
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());
    Entry {
        path: root.to_path_buf(),
        name,
        kind: EntryKind::Dir,
        children: scan_dir(root, 0),
    }
}

/// `path` relative to `base`, `/`-separated, or `None` if not under `base`.
fn rel(base: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    Some(parts.join("/"))
}

/// Resolve a `/`-separated project-relative path back to an absolute path.
fn resolve(base: &Path, rel: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        path.push(part);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build root/{assets/{a.png, b.png}, scenes/{main.ron}, sub/{deep/}, .hidden/, orbit.toml}.
    fn sample_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir_all(p.join("assets")).unwrap();
        std::fs::create_dir_all(p.join("scenes")).unwrap();
        std::fs::create_dir_all(p.join("sub/deep")).unwrap();
        std::fs::create_dir_all(p.join(".hidden")).unwrap();
        std::fs::write(p.join("orbit.toml"), "name = \"x\"").unwrap();
        std::fs::write(p.join("assets/b.png"), []).unwrap();
        std::fs::write(p.join("assets/a.png"), []).unwrap();
        std::fs::write(p.join("scenes/main.ron"), []).unwrap();
        dir
    }

    #[test]
    fn scan_sorts_dirs_first_then_files_and_skips_hidden() {
        let dir = sample_project();
        let ex = FileExplorer::new(dir.path());
        // Root contents: dirs (assets, scenes, sub) alphabetical, then orbit.toml.
        let names: Vec<&str> = ex.contents().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["assets", "scenes", "sub", "orbit.toml"]);
        // The dotfile directory is not scanned in.
        assert!(!names.contains(&".hidden"));
    }

    #[test]
    fn classify_and_thumbnail_gate_on_extension() {
        assert_eq!(classify(Path::new("a/b.PNG")), FileKind::Image);
        assert_eq!(classify(Path::new("a/b.jpg")), FileKind::Image);
        assert_eq!(classify(Path::new("s/main.ron")), FileKind::Scene);
        assert_eq!(classify(Path::new("readme.md")), FileKind::Other);
        assert!(can_thumbnail(Path::new("x.png")));
        assert!(!can_thumbnail(Path::new("x.jpg"))); // decodable icons only for png
        assert!(!can_thumbnail(Path::new("x.ron")));
    }

    #[test]
    fn tree_rows_are_folders_only_and_honor_expansion() {
        let dir = sample_project();
        let mut ex = FileExplorer::new(dir.path());
        // Root only expanded: root + its three sub-folders, no files.
        let rows = ex.tree_rows();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names[0], dir.path().file_name().unwrap().to_str().unwrap());
        assert!(names.contains(&"assets") && names.contains(&"sub"));
        assert!(
            !names
                .iter()
                .any(|n| n.ends_with(".ron") || n.ends_with(".png"))
        );
        // "sub" has a child dir "deep" that is hidden until "sub" is expanded.
        assert!(!ex.tree_rows().iter().any(|r| r.name == "deep"));
        ex.toggle_expand(&dir.path().join("sub"));
        assert!(ex.tree_rows().iter().any(|r| r.name == "deep"));
    }

    #[test]
    fn selecting_a_folder_shows_its_contents_and_reveals_it() {
        let dir = sample_project();
        let mut ex = FileExplorer::new(dir.path());
        ex.select_dir(&dir.path().join("assets"));
        let names: Vec<&str> = ex.contents().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.png", "b.png"]);
        // Breadcrumbs run root -> assets.
        let crumbs: Vec<String> = ex.breadcrumbs().into_iter().map(|(n, _)| n).collect();
        assert_eq!(crumbs.last().unwrap(), "assets");
        // Selecting a non-directory (a file) is refused.
        ex.select_dir(&dir.path().join("assets/a.png"));
        assert_eq!(ex.selected_dir(), dir.path().join("assets"));
    }

    #[test]
    fn navigating_deep_expands_ancestors_but_not_the_target() {
        let dir = sample_project();
        let mut ex = FileExplorer::new(dir.path());
        let deep = dir.path().join("sub/deep");
        ex.select_dir(&deep);
        // "sub" is expanded (an ancestor) so "deep" shows in the tree...
        let rows = ex.tree_rows();
        assert!(rows.iter().any(|r| r.name == "deep" && r.selected));
        // ...but "deep" itself is not auto-expanded.
        assert!(!rows.iter().find(|r| r.name == "deep").unwrap().expanded);
    }

    #[test]
    fn relative_paths_round_trip_and_persist() {
        let dir = sample_project();
        let mut ex = FileExplorer::new(dir.path());
        ex.toggle_expand(&dir.path().join("sub"));
        ex.select_dir(&dir.path().join("assets"));
        let expanded = ex.expanded_rel();
        let selected = ex.selected_rel();
        assert!(expanded.contains(&"sub".to_string()));
        assert_eq!(selected.as_deref(), Some("assets"));

        // A fresh explorer restores to the same visible state.
        let mut ex2 = FileExplorer::new(dir.path());
        ex2.restore(&expanded, selected.as_deref(), FileView::Grid);
        assert_eq!(ex2.view(), FileView::Grid);
        assert_eq!(ex2.selected_dir(), dir.path().join("assets"));
        assert!(ex2.tree_rows().iter().any(|r| r.name == "deep"));
    }

    #[test]
    fn restore_reveals_a_deep_selection_with_no_saved_expansions() {
        // Only a deep selected folder is saved (no expanded list): restore must
        // still reveal it by expanding its ancestors.
        let dir = sample_project();
        let mut ex = FileExplorer::new(dir.path());
        ex.restore(&[], Some("sub/deep"), FileView::List);
        assert_eq!(ex.selected_dir(), dir.path().join("sub/deep"));
        let rows = ex.tree_rows();
        assert!(rows.iter().any(|r| r.name == "deep" && r.selected));
    }

    #[test]
    fn restore_drops_paths_that_no_longer_resolve() {
        // Saved paths pointing at folders that are gone (or were never dirs) are
        // silently ignored rather than resurrecting phantom tree state.
        let dir = sample_project();
        let mut ex = FileExplorer::new(dir.path());
        ex.restore(
            &["ghost".to_string(), "orbit.toml".to_string()],
            Some("also-gone"),
            FileView::Grid,
        );
        // View still applies; bogus expansions/selection are dropped.
        assert_eq!(ex.view(), FileView::Grid);
        assert!(
            !ex.expanded_rel()
                .iter()
                .any(|p| p == "ghost" || p == "orbit.toml")
        );
        assert_eq!(
            ex.selected_dir(),
            dir.path(),
            "bad selection falls back to root"
        );
    }

    #[test]
    fn rescan_prunes_vanished_expanded_and_selected_paths() {
        let dir = sample_project();
        let mut ex = FileExplorer::new(dir.path());
        ex.toggle_expand(&dir.path().join("sub"));
        ex.select_dir(&dir.path().join("sub/deep"));
        assert_eq!(ex.selected_dir(), dir.path().join("sub/deep"));

        std::fs::remove_dir_all(dir.path().join("sub")).unwrap();
        ex.rescan();
        // The deleted selection falls back to the root, and the stale expanded
        // path is dropped.
        assert_eq!(ex.selected_dir(), dir.path());
        assert!(!ex.expanded_rel().iter().any(|p| p.starts_with("sub")));
    }

    #[test]
    fn project_relative_uses_forward_slashes_and_rejects_outsiders() {
        let dir = sample_project();
        let ex = FileExplorer::new(dir.path());
        assert_eq!(
            ex.project_relative(&dir.path().join("assets/a.png"))
                .as_deref(),
            Some("assets/a.png")
        );
        assert_eq!(ex.project_relative(Path::new("/etc/passwd")), None);
    }
}
