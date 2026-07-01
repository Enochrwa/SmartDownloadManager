//! Automatic file renaming on conflict (Sprint 3).
//!
//! When starting a brand-new (non-resumed) job, if the destination path
//! already exists on disk, we pick the next available "name (1).ext",
//! "name (2).ext", ... instead of clobbering an unrelated file.

use std::path::{Path, PathBuf};

/// Return `path` unchanged if it doesn't exist; otherwise return the first
/// "`stem` (`n`).`ext`" variant that doesn't exist, checked via `exists_fn`
/// (injected so tests don't need real disk I/O).
pub fn unique_destination_with(path: &Path, exists_fn: impl Fn(&Path) -> bool) -> PathBuf {
    if !exists_fn(path) {
        return path.to_path_buf();
    }

    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download")
        .to_string();
    let ext = path.extension().and_then(|s| s.to_str()).map(String::from);

    for n in 1..10_000u32 {
        let candidate_name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(candidate_name);
        if !exists_fn(&candidate) {
            return candidate;
        }
    }

    // Extremely unlikely fallback: fall back to the original path rather
    // than looping forever.
    path.to_path_buf()
}

/// Real-filesystem convenience wrapper around [`unique_destination_with`].
pub fn unique_destination(path: &Path) -> PathBuf {
    unique_destination_with(path, |p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn returns_original_path_when_free() {
        let existing: HashSet<PathBuf> = HashSet::new();
        let out =
            unique_destination_with(Path::new("/downloads/movie.mkv"), |p| existing.contains(p));
        assert_eq!(out, PathBuf::from("/downloads/movie.mkv"));
    }

    #[test]
    fn appends_counter_when_taken() {
        let mut existing: HashSet<PathBuf> = HashSet::new();
        existing.insert(PathBuf::from("/downloads/movie.mkv"));
        let out =
            unique_destination_with(Path::new("/downloads/movie.mkv"), |p| existing.contains(p));
        assert_eq!(out, PathBuf::from("/downloads/movie (1).mkv"));
    }

    #[test]
    fn finds_next_free_counter() {
        let mut existing: HashSet<PathBuf> = HashSet::new();
        existing.insert(PathBuf::from("/downloads/movie.mkv"));
        existing.insert(PathBuf::from("/downloads/movie (1).mkv"));
        existing.insert(PathBuf::from("/downloads/movie (2).mkv"));
        let out =
            unique_destination_with(Path::new("/downloads/movie.mkv"), |p| existing.contains(p));
        assert_eq!(out, PathBuf::from("/downloads/movie (3).mkv"));
    }

    #[test]
    fn handles_files_without_extension() {
        let mut existing: HashSet<PathBuf> = HashSet::new();
        existing.insert(PathBuf::from("/downloads/README"));
        let out = unique_destination_with(Path::new("/downloads/README"), |p| existing.contains(p));
        assert_eq!(out, PathBuf::from("/downloads/README (1)"));
    }
}
