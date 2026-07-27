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
use std::time::SystemTime;

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
    /// The file's size in bytes (0 for a directory).
    pub size: u64,
    /// Last-modified time, if the filesystem reported one.
    pub modified: Option<SystemTime>,
    /// Child entries, directories first then files, each group alphabetical
    /// (case-insensitive). Always empty for a file.
    pub children: Vec<Entry>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }

    /// A short human label for the entry's type: "Folder", or the file's
    /// uppercased extension (e.g. "PNG"), or "File" when it has none.
    pub fn type_label(&self) -> String {
        if self.is_dir() {
            return "Folder".to_string();
        }
        match extension(&self.path) {
            Some(ext) => ext.to_ascii_uppercase(),
            None => "File".to_string(),
        }
    }

    /// A human size: a folder's child count ("3 items"), or a file's byte size
    /// ("2.0 KB").
    pub fn size_label(&self) -> String {
        if self.is_dir() {
            let n = self.children.len();
            return format!("{n} item{}", if n == 1 { "" } else { "s" });
        }
        format_size(self.size)
    }

    /// A relative last-modified label ("3 days ago"), or empty if unknown.
    pub fn modified_label(&self) -> String {
        self.modified.map(format_relative_time).unwrap_or_default()
    }
}

/// A byte count as a short human string (B / KB / MB / GB).
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else if b < GB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.1} GB", b / GB)
    }
}

/// A coarse "N units ago" label for a past time relative to now (crate-free, so
/// no timezone handling and no calendar math). Future times (clock skew) read as
/// "just now".
pub fn format_relative_time(modified: SystemTime) -> String {
    relative_time_between(modified, SystemTime::now())
}

fn relative_time_between(modified: SystemTime, now: SystemTime) -> String {
    let secs = now
        .duration_since(modified)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ago = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    match secs {
        0..MIN => "just now".to_string(),
        MIN..HOUR => ago(secs / MIN, "minute"),
        HOUR..DAY => ago(secs / HOUR, "hour"),
        DAY..MONTH => ago(secs / DAY, "day"),
        MONTH..YEAR => ago(secs / MONTH, "month"),
        _ => ago(secs / YEAR, "year"),
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

/// A project image asset, for the asset chooser: the absolute path (to look up
/// its thumbnail), the project-relative path (the value written into a field),
/// and the display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRef {
    pub abs: PathBuf,
    pub rel: String,
    pub name: String,
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
        // Entry metadata (does not follow symlinks); missing is not fatal.
        let meta = dirent.metadata().ok();
        let modified = meta.as_ref().and_then(|m| m.modified().ok());
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
                size: 0,
                modified,
                children,
            });
        } else {
            entries.push(Entry {
                path,
                name,
                kind: EntryKind::File,
                size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                modified,
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
    /// The highlighted entries in the contents pane (files and sub-folders of
    /// `selected_dir`). Multi-select via ctrl/shift; the last is the primary
    /// (what a rename targets).
    selected: Vec<PathBuf>,
    /// The fixed end of a shift-range selection (the last plain/ctrl click).
    anchor: Option<PathBuf>,
    /// Compact list vs. large-preview grid.
    view: FileView,
    /// The folder tree's scroll offset, kept here (not in the per-pane scroll
    /// map, which the pane spends on its contents area) so it survives the shell
    /// rebuilds that a selection or a landing thumbnail trigger. Transient - not
    /// persisted across restarts.
    tree_scroll: f32,
    /// The contents entry being inline-renamed, if any (its row shows an edit
    /// field instead of a label). Held here so the renderer sees it via the same
    /// reference the pane already threads.
    renaming: Option<PathBuf>,
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
            selected: Vec::new(),
            anchor: None,
            view: FileView::default(),
            tree_scroll: 0.0,
            renaming: None,
        }
    }

    /// The contents entry currently being inline-renamed, if any.
    pub fn renaming(&self) -> Option<&Path> {
        self.renaming.as_deref()
    }

    /// Begin (or, with `None`, cancel) an inline rename of a contents entry.
    pub fn set_renaming(&mut self, path: Option<PathBuf>) {
        self.renaming = path;
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
        // Drop highlighted / renaming entries that no longer exist (deleted,
        // moved, renamed).
        let present: Vec<PathBuf> = self.contents().iter().map(|e| e.path.clone()).collect();
        self.selected.retain(|p| present.contains(p));
        if self.anchor.as_ref().is_some_and(|a| !present.contains(a)) {
            self.anchor = None;
        }
        if self.renaming.as_ref().is_some_and(|p| !present.contains(p)) {
            self.renaming = None;
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

    /// Every previewable image in the project, sorted by relative path, for the
    /// asset chooser. Only the decodable kind (PNG) is listed - those are the
    /// images that both preview and actually load as a texture.
    pub fn all_images(&self) -> Vec<AssetRef> {
        let mut out = Vec::new();
        self.collect_images(&self.root, &mut out);
        out.sort_by(|a, b| a.rel.cmp(&b.rel));
        out
    }

    fn collect_images(&self, dir: &Entry, out: &mut Vec<AssetRef>) {
        for child in &dir.children {
            if child.is_dir() {
                self.collect_images(child, out);
            } else if can_thumbnail(&child.path)
                && let Some(rel) = self.project_relative(&child.path)
            {
                out.push(AssetRef {
                    abs: child.path.clone(),
                    rel,
                    name: child.name.clone(),
                });
            }
        }
    }

    /// Resolve a project-relative path (as stored in an asset field) to its
    /// absolute path - e.g. to look up a field's thumbnail.
    pub fn resolve_relative(&self, rel: &str) -> PathBuf {
        resolve(&self.root.path, rel)
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

    /// Whether `path` is one of the highlighted contents entries.
    pub fn is_selected(&self, path: &Path) -> bool {
        self.selected.iter().any(|p| p == path)
    }

    /// The highlighted contents entries (files and sub-folders).
    pub fn selected(&self) -> &[PathBuf] {
        &self.selected
    }

    /// The primary selection - the most recently clicked entry, what a rename
    /// targets.
    pub fn primary(&self) -> Option<&Path> {
        self.selected.last().map(|p| p.as_path())
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
        self.clear_selection();
        // A rename in progress in the old folder is abandoned (its field is no
        // longer rendered), so drop the state or it would swallow the next Escape.
        self.renaming = None;
        self.expand_ancestors(dir);
    }

    /// The single selected entry when it is exactly one directory, else `None`
    /// (used to target a paste / new folder at a highlighted sub-folder).
    pub fn selected_single_dir(&self) -> Option<&Path> {
        match self.selected.as_slice() {
            [only] if self.find(only).is_some_and(Entry::is_dir) => Some(only),
            _ => None,
        }
    }

    /// Replace the contents selection with just `path` (a plain click).
    pub fn select_one(&mut self, path: &Path) {
        self.selected = vec![path.to_path_buf()];
        self.anchor = Some(path.to_path_buf());
    }

    /// Toggle `path` in/out of the selection (a ctrl-click), keeping the rest.
    pub fn toggle_selected(&mut self, path: &Path) {
        if let Some(i) = self.selected.iter().position(|p| p == path) {
            self.selected.remove(i);
        } else {
            self.selected.push(path.to_path_buf());
        }
        self.anchor = Some(path.to_path_buf());
    }

    /// Select the contiguous range of contents entries between the anchor and
    /// `path` in display order (a shift-click); falls back to a plain select if
    /// there is no anchor or either end is not in the current folder.
    pub fn select_range(&mut self, path: &Path) {
        let order: Vec<PathBuf> = self.contents().iter().map(|e| e.path.clone()).collect();
        let anchor = self.anchor.clone();
        let ends = anchor
            .as_deref()
            .and_then(|a| order.iter().position(|p| p == a))
            .zip(order.iter().position(|p| p == path));
        match ends {
            Some((i, j)) => {
                let (lo, hi) = if i <= j { (i, j) } else { (j, i) };
                self.selected = order[lo..=hi].to_vec();
                // The anchor stays put so the range can be resized.
            }
            None => self.select_one(path),
        }
    }

    /// Select every entry in the current folder (ctrl+a).
    pub fn select_all(&mut self) {
        self.selected = self.contents().iter().map(|e| e.path.clone()).collect();
        self.anchor = self.selected.first().cloned();
    }

    /// Clear the contents selection.
    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
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
    let modified = std::fs::metadata(root).ok().and_then(|m| m.modified().ok());
    Entry {
        path: root.to_path_buf(),
        name,
        kind: EntryKind::Dir,
        size: 0,
        modified,
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
    fn contents_selection_supports_single_toggle_range_and_all() {
        let dir = sample_project();
        let mut ex = FileExplorer::new(dir.path());
        ex.select_dir(&dir.path().join("assets")); // contents: [a.png, b.png]
        let a = dir.path().join("assets/a.png");
        let b = dir.path().join("assets/b.png");

        // Plain select replaces; is the primary.
        ex.select_one(&a);
        assert!(ex.is_selected(&a) && !ex.is_selected(&b));
        assert_eq!(ex.primary(), Some(a.as_path()));
        // Ctrl-toggle adds then removes, keeping the rest.
        ex.toggle_selected(&b);
        assert_eq!(ex.selected(), &[a.clone(), b.clone()]);
        ex.toggle_selected(&a);
        assert_eq!(ex.selected(), std::slice::from_ref(&b));
        // Shift-range from the anchor covers the span in display order.
        ex.select_one(&a); // anchor = a
        ex.select_range(&b);
        assert_eq!(ex.selected(), &[a.clone(), b.clone()]);
        // Select-all, then clear.
        ex.clear_selection();
        assert!(ex.selected().is_empty());
        ex.select_all();
        assert_eq!(ex.selected().len(), 2);
        // Navigating to another folder drops the selection.
        ex.select_dir(dir.path());
        assert!(ex.selected().is_empty());
    }

    #[test]
    fn size_and_relative_time_format_readably() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2.0 KB");
        assert_eq!(format_size(1_500_000), "1.4 MB");
        assert_eq!(format_size(3_221_225_472), "3.0 GB");

        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        let ago = |secs| relative_time_between(now - std::time::Duration::from_secs(secs), now);
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(3 * 3600), "3 hours ago");
        assert_eq!(ago(2 * 86400), "2 days ago");
        assert_eq!(ago(40 * 86400), "1 month ago");
        assert_eq!(ago(800 * 86400), "2 years ago");
        // A future time (clock skew) reads as "just now", never a huge number.
        assert_eq!(
            relative_time_between(now + std::time::Duration::from_secs(99), now),
            "just now"
        );
    }

    #[test]
    fn all_images_flattens_previewable_images_project_wide() {
        let dir = sample_project();
        // Add a nested png and a non-png image, plus the existing assets/*.png.
        std::fs::write(dir.path().join("sub/nested.png"), []).unwrap();
        std::fs::write(dir.path().join("assets/photo.jpg"), []).unwrap();
        let ex = FileExplorer::new(dir.path());
        let rels: Vec<String> = ex.all_images().into_iter().map(|a| a.rel).collect();
        // PNGs anywhere in the tree, sorted, project-relative; the .jpg is
        // excluded (only PNG previews/loads) and .ron is not an image.
        assert_eq!(rels, ["assets/a.png", "assets/b.png", "sub/nested.png"]);
    }

    #[test]
    fn selected_single_dir_targets_a_lone_selected_folder() {
        let dir = sample_project();
        let mut ex = FileExplorer::new(dir.path()); // root: [assets, scenes, sub, orbit.toml]
        // One folder selected -> that folder.
        ex.select_one(&dir.path().join("sub"));
        assert_eq!(
            ex.selected_single_dir(),
            Some(dir.path().join("sub").as_path())
        );
        // A file, or a multi-selection, is not a single-folder target.
        ex.select_one(&dir.path().join("orbit.toml"));
        assert_eq!(ex.selected_single_dir(), None);
        ex.select_one(&dir.path().join("sub"));
        ex.toggle_selected(&dir.path().join("assets"));
        assert_eq!(ex.selected_single_dir(), None);
    }

    #[test]
    fn an_in_progress_rename_is_dropped_on_navigation_and_rescan() {
        let dir = sample_project();
        let mut ex = FileExplorer::new(dir.path());
        ex.select_dir(&dir.path().join("assets"));
        let a = dir.path().join("assets/a.png");
        ex.set_renaming(Some(a.clone()));
        assert_eq!(ex.renaming(), Some(a.as_path()));
        // Navigating away abandons the rename (else it swallows the next Escape).
        ex.select_dir(dir.path());
        assert_eq!(ex.renaming(), None);
        // A rescan that loses the renamed entry also clears it.
        ex.select_dir(&dir.path().join("assets"));
        ex.set_renaming(Some(a.clone()));
        std::fs::remove_file(&a).unwrap();
        ex.rescan();
        assert_eq!(ex.renaming(), None);
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
