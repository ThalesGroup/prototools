// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Dynamic shell completion.
//!
//! `protolens/src/complete.rs` is this file's near-twin. The command line
//! being completed is read with `prototools_complete::args`, which both
//! (and protoback) share: they all depend on `prototools-complete` already,
//! so sharing the scanner adds no dependency edge. Two facts the
//! completers have to stay honest about:
//!
//! - The words a completer is handed are the command line as **typed**.
//!   `--descriptor-set $PROTOTEXT_DESCRIPTOR_SET` arrives as those literal
//!   characters and names no file until it is expanded, which
//!   `args::flag_value` does.
//! - A clap `env = ...` fallback is applied while *parsing*, a step the
//!   completion path returns before reaching, so every variable the flag
//!   resolves through has to be read here as well.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use clap_complete::engine::CompletionCandidate;
use prototext_core::schema::descriptor::Encode;
use prototext_core::schema::descriptor::{DescriptorProto, FileDescriptorProto, FileDescriptorSet};

use prototools_complete::args::flag_value;

use crate::EMBEDDED_DESCRIPTOR;

// ── Descriptor helpers ────────────────────────────────────────────────────────

/// Enumerate fully-qualified message type names from raw descriptor bytes
/// (`FileDescriptorSet` or `FileDescriptorProto`).
pub fn message_names_from_descriptor(bytes: &[u8]) -> Vec<String> {
    fn collect(pkg_prefix: &str, messages: &[DescriptorProto], out: &mut Vec<String>) {
        for msg in messages {
            let name = msg.name.as_deref().unwrap_or("");
            let fqn = if pkg_prefix.is_empty() {
                name.to_string()
            } else {
                format!("{pkg_prefix}.{name}")
            };
            out.push(fqn.clone());
            collect(&fqn, &msg.nested_type, out);
        }
    }

    let files: Vec<FileDescriptorProto> = if let Ok(fds) = FileDescriptorSet::decode(bytes) {
        fds.file
    } else if let Ok(fdp) = FileDescriptorProto::decode(bytes) {
        vec![fdp]
    } else {
        return vec![];
    };

    let mut names = Vec::new();
    for file in &files {
        let pkg = file.package.as_deref().unwrap_or("");
        collect(pkg, &file.message_type, &mut names);
    }
    names
}

// ── Filesystem listing helpers ────────────────────────────────────────────────

/// Complete paths under `base` (the effective root directory), mirroring
/// clap_complete's own `complete_path` logic.
///
/// - `incomplete` is the raw token the user has typed so far.
/// - Its directory part is resolved relative to `base`; its filename part is
///   used as a prefix filter on directory entries.
/// - Directories are returned with a trailing `/`; files as-is.
/// - Each candidate is the full value to insert (same format as the built-in
///   `PathCompleter`).
fn complete_path_under(
    incomplete: &OsStr,
    base: &Path,
    suffix_filter: Option<&str>,
    dirs_only: bool,
) -> Vec<CompletionCandidate> {
    let s = incomplete.to_string_lossy();
    let incomplete_path = Path::new(incomplete);

    // Split into the already-typed directory prefix and the partial filename.
    // A trailing `/` means the user has completed a directory name and wants
    // its contents — treat the whole token as the prefix with an empty stem.
    let (typed_prefix, filename_stem) = if s.ends_with('/') {
        (incomplete_path.to_path_buf(), String::new())
    } else {
        let parent = incomplete_path.parent().unwrap_or(Path::new(""));
        let stem = incomplete_path
            .file_name()
            .unwrap_or(OsStr::new(""))
            .to_string_lossy()
            .into_owned();
        (parent.to_path_buf(), stem)
    };

    let search_root = base.join(&typed_prefix);

    let Ok(rd) = std::fs::read_dir(&search_root) else {
        return vec![];
    };

    let mut completions: Vec<CompletionCandidate> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&filename_stem))
        .filter_map(|e| {
            let name = e.file_name();
            let ft = e.file_type().ok()?;
            if ft.is_dir() {
                // No trailing slash — compopt -o filenames adds it.
                let p = typed_prefix.join(&name);
                Some(CompletionCandidate::new(p.as_os_str().to_os_string()))
            } else if ft.is_file() && !dirs_only {
                let name_s = name.to_string_lossy();
                if suffix_filter.is_none_or(|s| name_s.ends_with(s)) {
                    let p = typed_prefix.join(&name);
                    Some(CompletionCandidate::new(p.as_os_str().to_os_string()))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    completions.sort_by(|a, b| a.get_value().cmp(b.get_value()));
    completions
}

// ── Completer functions ───────────────────────────────────────────────────────

/// Complete descriptor files (any extension) relative to cwd.
pub fn complete_descriptor_path(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    complete_path_under(incomplete, &base, None, false)
}

/// Complete any file or directory path relative to cwd.
pub fn complete_any_path(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    complete_path_under(incomplete, &base, None, false)
}

/// Complete directory paths only relative to cwd.
pub fn complete_dir_path(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    complete_path_under(incomplete, &base, None, true)
}

/// Complete message type names, against the same descriptor set a real
/// run would resolve (`run.rs`'s order, and the one `Cli::descriptor`
/// documents):
///
/// 1. `--descriptor-set` / `--descriptor` on the partial command line;
/// 2. `PROTOTEXT_DESCRIPTOR_SET`, which is what the flag declares as its
///    `env`, then the deprecated `PROTOTEXT_DEFAULT_DESCRIPTOR`;
/// 3. the embedded descriptor — all `google.protobuf.*` types.
///
/// Step 2 named only the *deprecated* variable until 2026-08-10, so a
/// session pointed at googleapis through `PROTOTEXT_DESCRIPTOR_SET`
/// completed `--type` against the built-in `google.protobuf.*` list
/// instead. The fallback made that look like a working completion rather
/// than a missing one, which is why it went unnoticed.
pub fn complete_type_names(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    let bytes: std::borrow::Cow<[u8]> = if let Some(path) = flag_value(None, "--descriptor-set")
        .or_else(|| flag_value(None, "--descriptor"))
        .or_else(|| std::env::var_os("PROTOTEXT_DESCRIPTOR_SET"))
        .or_else(|| std::env::var_os("PROTOTEXT_DEFAULT_DESCRIPTOR"))
    {
        match std::fs::read(&path) {
            Ok(b) => std::borrow::Cow::Owned(b),
            Err(_) => return vec![],
        }
    } else {
        std::borrow::Cow::Borrowed(EMBEDDED_DESCRIPTOR)
    };

    let prefix = incomplete.to_string_lossy();
    message_names_from_descriptor(&bytes)
        .into_iter()
        .filter(|name| name.starts_with(prefix.as_ref()))
        .map(CompletionCandidate::new)
        .collect()
}

/// Complete positional PATH arguments.
///
/// If `--input-root`/`-I` is already present in the partial command line,
/// complete relative to that directory.  Otherwise complete relative to cwd.
pub fn complete_input_paths(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    let base = flag_value(Some("-I"), "--input-root")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    complete_path_under(incomplete, &base, None, false)
}

#[cfg(test)]
mod tests {
    /// `--descriptor-set` declares `env = "PROTOTEXT_DESCRIPTOR_SET"`
    /// (`lib.rs`), and `run` falls back to the deprecated
    /// `PROTOTEXT_DEFAULT_DESCRIPTOR` only when the new one is absent.
    /// The completer named the deprecated one alone, so it resolved a
    /// different descriptor set than the launch it was completing for.
    #[test]
    fn the_completer_reads_the_env_var_the_flag_declares() {
        use clap::CommandFactory;

        let cmd = crate::Cli::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_long() == Some("descriptor-set"))
            .expect("--descriptor-set exists");

        assert_eq!(
            arg.get_env().map(|e| e.to_string_lossy().into_owned()),
            Some("PROTOTEXT_DESCRIPTOR_SET".to_string()),
            "the name complete_type_names must consult first"
        );
    }
}
