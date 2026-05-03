// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSetBuilder};
use walkdir::WalkDir;

// ── Types ─────────────────────────────────────────────────────────────────────

/// A resolved input file together with its relative path used to compute the
/// output path in batch mode.
pub struct InputFile {
    /// Absolute path to the input file.
    pub abs: PathBuf,
    /// Path relative to `--input-root` (or cwd), used to build the output path.
    pub rel: PathBuf,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Expand a single positional PATH (file, glob, or directory) into one or more
/// [`InputFile`]s.  `base` is the `--input-root` directory (or cwd when absent).
pub fn expand_path(raw: &str, base: &Path) -> Result<Vec<InputFile>, String> {
    let raw_path = Path::new(raw);

    if raw_path.is_absolute() {
        // Absolute paths are accepted only when --input-root is absent (the
        // caller enforces the restriction before calling this function).
        if raw_path.is_dir() {
            return expand_dir(raw_path, raw_path.parent().unwrap_or(raw_path));
        }
        if raw_path.exists() {
            return Ok(vec![InputFile {
                rel: raw_path
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| raw_path.to_path_buf()),
                abs: raw_path.to_path_buf(),
            }]);
        }
        return Err(format!("input not found: {}", raw_path.display()));
    }

    let candidate = base.join(raw_path);

    // Directory → recurse.
    if candidate.is_dir() {
        return expand_dir(&candidate, base);
    }

    // Existing file → single entry.
    if candidate.is_file() {
        return Ok(vec![InputFile {
            rel: raw_path.to_path_buf(),
            abs: candidate,
        }]);
    }

    // Fall through to glob expansion.
    expand_glob(raw, base)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Walk `root`, yielding one [`InputFile`] per regular file.  `base` is the
/// anchor used to compute relative paths (typically `--input-root` or cwd).
fn walk_files<'a>(root: &'a Path, base: &'a Path) -> impl Iterator<Item = InputFile> + use<'a> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(move |entry| {
            let abs = entry.path().to_path_buf();
            let rel = abs
                .strip_prefix(base)
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|_| {
                    abs.file_name()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| abs.clone())
                });
            InputFile { abs, rel }
        })
}

fn expand_dir(dir: &Path, base: &Path) -> Result<Vec<InputFile>, String> {
    let files: Vec<InputFile> = walk_files(dir, base).collect();
    if files.is_empty() {
        Err(format!("directory is empty: {}", dir.display()))
    } else {
        Ok(files)
    }
}

fn expand_glob(pattern: &str, base: &Path) -> Result<Vec<InputFile>, String> {
    let glob =
        Glob::new(pattern).map_err(|e| format!("invalid glob pattern '{}': {}", pattern, e))?;
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    let set = builder
        .build()
        .map_err(|e| format!("glob build error: {}", e))?;

    let files: Vec<InputFile> = walk_files(base, base)
        .filter(|f| set.is_match(&f.rel))
        .collect();

    if files.is_empty() {
        Err(format!("glob '{}' matched no files", pattern))
    } else {
        Ok(files)
    }
}
