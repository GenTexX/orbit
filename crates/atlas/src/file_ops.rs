//! atlas file operations: the disk-mutating actions the explorer's context menu
//! and keyboard shortcuts drive - new folder, rename, duplicate, copy/move, and
//! delete. Kept apart from the explorer's view model so the (irreversible) file
//! I/O is small, self-contained, and unit-tested against real temp directories.
//!
//! Delete does not destroy anything: it moves the target into a project-local
//! `.orbit/trash/` folder, so a mistaken delete is always recoverable by hand.
//! Every operation avoids clobbering an existing file by picking a free name
//! (`name copy`, `name copy 2`, ...).

use std::io;
use std::path::{Path, PathBuf};

/// Split a file name into its stem and extension (including the dot), e.g.
/// `"sprite.png" -> ("sprite", ".png")`. A leading dot (a dotfile) or no dot
/// yields an empty extension, so the whole name is treated as the stem.
fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// A path in `dir` for `name` that does not already exist: `name` itself if free,
/// else `name copy`, `name copy 2`, ... (the suffix inserted before the
/// extension). Bounded so a truly pathological directory cannot loop forever.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = split_name(name);
    for n in 1..10_000 {
        let suffix = if n == 1 {
            " copy".to_string()
        } else {
            format!(" copy {n}")
        };
        let candidate = dir.join(format!("{stem}{suffix}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Effectively unreachable; fall back to the plain path rather than panic.
    first
}

/// Whether `name` is a safe single path component (no separators, not empty,
/// not a `.`/`..` traversal). User-entered names for rename / new-folder must
/// pass this before touching disk.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// Create a new folder in `dir` (named "New Folder", or a free variant), and
/// return its path. Atomic: `create_dir`'s own `AlreadyExists` drives the search
/// for a free name, so a racing creation cannot make it clobber or spuriously
/// fail (no check-then-act gap).
pub fn create_folder(dir: &Path) -> io::Result<PathBuf> {
    for n in 0..10_000 {
        let name = match n {
            0 => "New Folder".to_string(),
            1 => "New Folder copy".to_string(),
            _ => format!("New Folder copy {n}"),
        };
        let path = dir.join(name);
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many New Folder copies",
    ))
}

/// Rename `path` to a sibling named `new_name`, returning the new path. A no-op
/// (returns the original) if the name is unchanged; refuses an invalid name or
/// one that would overwrite a different existing entry.
pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    if !is_valid_name(new_name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid name"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent"))?;
    let target = parent.join(new_name);
    if target == path {
        return Ok(path.to_path_buf());
    }
    // Refuse to overwrite a DIFFERENT existing entry. A case-only rename
    // ("a.png" -> "A.png") on a case-insensitive filesystem sees the target as
    // existing though it is the very same file; canonicalizing both tells them
    // apart, so a legitimate case change is allowed through.
    if target.exists() {
        let same_file = std::fs::canonicalize(&target).ok() == std::fs::canonicalize(path).ok();
        if !same_file {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a file with that name already exists",
            ));
        }
    }
    std::fs::rename(path, &target)?;
    Ok(target)
}

/// Copy `path` to a uniquely-named sibling ("name copy"), returning the copy's
/// path. Recurses for a directory.
pub fn duplicate(path: &Path) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent"))?;
    let name = file_name(path)?;
    let dest = unique_path(parent, &name);
    copy_recursive(path, &dest)?;
    Ok(dest)
}

/// Copy `src` into directory `dst_dir` (unique name on collision), returning the
/// new path. Refuses to copy a directory into itself or its own subtree.
pub fn copy_into(src: &Path, dst_dir: &Path) -> io::Result<PathBuf> {
    let dest = destination_in(src, dst_dir)?;
    copy_recursive(src, &dest)?;
    Ok(dest)
}

/// Move `src` into directory `dst_dir` (unique name on collision), returning the
/// new path. Refuses to move a directory into itself or its own subtree.
pub fn move_into(src: &Path, dst_dir: &Path) -> io::Result<PathBuf> {
    let dest = destination_in(src, dst_dir)?;
    // A plain rename works within one filesystem (the common case); fall back to
    // copy-then-remove across devices.
    if std::fs::rename(src, &dest).is_err() {
        copy_recursive(src, &dest)?;
        remove(src)?;
    }
    Ok(dest)
}

/// Move `path` into the project's `.orbit/trash/` (created on demand), returning
/// where it landed. Recoverable by hand; nothing is destroyed.
pub fn delete_to_trash(project_root: &Path, path: &Path) -> io::Result<PathBuf> {
    let trash = project_root.join(".orbit").join("trash");
    // Deleting something already in the trash is a no-op (defensive; not
    // reachable from the UI, which never scans the trash into the tree).
    if path.starts_with(&trash) {
        return Ok(path.to_path_buf());
    }
    std::fs::create_dir_all(&trash)?;
    let name = file_name(path)?;
    let dest = unique_path(&trash, &name);
    if std::fs::rename(path, &dest).is_err() {
        copy_recursive(path, &dest)?;
        remove(path)?;
    }
    Ok(dest)
}

/// The (unique) destination path for placing `src` inside `dst_dir`, refusing a
/// directory copied into itself or its own subtree (which would recurse away).
fn destination_in(src: &Path, dst_dir: &Path) -> io::Result<PathBuf> {
    if dst_dir == src || dst_dir.starts_with(src) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot place a folder inside itself",
        ));
    }
    let name = file_name(src)?;
    Ok(unique_path(dst_dir, &name))
}

/// A path's final component as a string, or an error for a path that has none.
fn file_name(path: &Path) -> io::Result<String> {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no name"))
}

/// A depth cap on the recursive copy, matching the scan's, as a backstop against
/// a pathologically deep tree overflowing the stack.
const MAX_COPY_DEPTH: usize = 64;

/// Recursively copy a file or directory tree from `src` to `dst`.
fn copy_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    copy_tree(src, dst, 0)
}

fn copy_tree(src: &Path, dst: &Path, depth: usize) -> io::Result<()> {
    if depth >= MAX_COPY_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory nesting too deep to copy",
        ));
    }
    // Stat WITHOUT following symlinks: a symlink is treated as a leaf and simply
    // skipped, never descended into - otherwise a link pointing at its own
    // ancestor would recurse forever (and following one would copy a whole
    // foreign tree as real bytes).
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dst.join(entry.file_name()), depth + 1)?;
        }
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

/// Remove a file or directory tree.
fn remove(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn unique_path_inserts_copy_before_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        // No collision: the name is used as-is.
        assert_eq!(unique_path(dir.path(), "a.png"), dir.path().join("a.png"));
        // One collision -> " copy"; a second -> " copy 2", before the extension.
        touch(&dir.path().join("a.png"));
        assert_eq!(
            unique_path(dir.path(), "a.png"),
            dir.path().join("a copy.png")
        );
        touch(&dir.path().join("a copy.png"));
        assert_eq!(
            unique_path(dir.path(), "a.png"),
            dir.path().join("a copy 2.png")
        );
        // A dotfile / extension-less name keeps the whole thing as the stem.
        touch(&dir.path().join("README"));
        assert_eq!(
            unique_path(dir.path(), "README"),
            dir.path().join("README copy")
        );
    }

    #[test]
    fn create_folder_makes_a_unique_new_folder() {
        let dir = tempfile::tempdir().unwrap();
        let a = create_folder(dir.path()).unwrap();
        assert_eq!(a, dir.path().join("New Folder"));
        assert!(a.is_dir());
        let b = create_folder(dir.path()).unwrap();
        assert_eq!(b, dir.path().join("New Folder copy"));
    }

    #[test]
    fn rename_moves_and_refuses_collisions_and_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        touch(&a);
        let renamed = rename(&a, "b.png").unwrap();
        assert_eq!(renamed, dir.path().join("b.png"));
        assert!(!a.exists() && renamed.exists());
        // Renaming to the same name is a no-op.
        assert_eq!(rename(&renamed, "b.png").unwrap(), renamed);
        // Refuses overwriting an existing sibling, and invalid names.
        touch(&dir.path().join("c.png"));
        assert!(rename(&renamed, "c.png").is_err());
        assert!(rename(&renamed, "../evil").is_err());
        assert!(rename(&renamed, "").is_err());
    }

    #[test]
    fn duplicate_copies_a_file_and_a_directory_tree() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("s.png");
        touch(&f);
        let dup = duplicate(&f).unwrap();
        assert_eq!(dup, dir.path().join("s copy.png"));
        assert!(dup.exists() && f.exists());

        // A directory duplicates recursively.
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        touch(&sub.join("inner.txt"));
        let dupdir = duplicate(&sub).unwrap();
        assert_eq!(dupdir, dir.path().join("sub copy"));
        assert!(dupdir.join("inner.txt").exists());
    }

    #[test]
    fn copy_and_move_into_a_folder_with_collision_handling() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.png");
        touch(&src);
        let dst = dir.path().join("dst");
        std::fs::create_dir(&dst).unwrap();

        let copied = copy_into(&src, &dst).unwrap();
        assert_eq!(copied, dst.join("a.png"));
        assert!(src.exists() && copied.exists());
        // A second copy into the same folder gets a unique name.
        let copied2 = copy_into(&src, &dst).unwrap();
        assert_eq!(copied2, dst.join("a copy.png"));

        // Move removes the source.
        let moved = move_into(&src, &dst).unwrap();
        assert!(!src.exists() && moved.exists());
        assert_eq!(moved, dst.join("a copy 2.png"));
    }

    #[test]
    fn copy_into_refuses_a_folder_into_its_own_subtree() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("outer");
        let inner = outer.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        assert!(copy_into(&outer, &outer).is_err());
        assert!(copy_into(&outer, &inner).is_err());
        assert!(move_into(&outer, &inner).is_err());
    }

    #[test]
    fn rename_allows_a_case_only_change() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        touch(&a);
        // "a.png" -> "A.png": on a case-sensitive FS the target does not exist so
        // it renames outright; the canonicalize guard covers the case-insensitive
        // FS where the target "exists" as the same file.
        let renamed = rename(&a, "A.png").unwrap();
        assert_eq!(renamed, dir.path().join("A.png"));
    }

    #[cfg(unix)]
    #[test]
    fn copy_does_not_follow_a_symlink_loop() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        touch(&src.join("real.txt"));
        // A symlink pointing back at its own parent: descending it would recurse
        // forever. Duplicate must terminate and skip the link.
        symlink(&src, src.join("loop")).unwrap();
        let dup = duplicate(&src).unwrap();
        assert!(dup.join("real.txt").exists(), "real files are copied");
        assert!(!dup.join("loop").exists(), "the symlink loop is skipped");
    }

    #[test]
    fn delete_moves_into_the_project_trash_recoverably() {
        let root = tempfile::tempdir().unwrap();
        let asset = root.path().join("doomed.png");
        touch(&asset);
        let landed = delete_to_trash(root.path(), &asset).unwrap();
        assert!(!asset.exists(), "removed from its original place");
        assert!(landed.exists(), "still exists in the trash (recoverable)");
        assert!(landed.starts_with(root.path().join(".orbit").join("trash")));

        // Deleting another file of the same name does not clobber the first.
        touch(&asset);
        let landed2 = delete_to_trash(root.path(), &asset).unwrap();
        assert_ne!(landed, landed2);
        assert!(landed.exists() && landed2.exists());
    }
}
