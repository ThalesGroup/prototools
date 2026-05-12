// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use clap_complete::engine::CompletionCandidate;
use prost::Message as ProstMessage;
use prost_types::{FileDescriptorProto, FileDescriptorSet};

use crate::EMBEDDED_DESCRIPTOR;

// ── Partial-args scanner ──────────────────────────────────────────────────────

/// Scan the partial command line (`std::env::args_os`, post `--`) for a flag
/// with a value.  Returns the value of the last occurrence of `short` or
/// `long`, e.g. `flag_value_from_args("-D", "--descriptor")`.
///
/// When the shell invokes the binary for completion it passes all already-typed
/// words as argv after a `--` sentinel, so `args_os()` is safe to read here.
pub fn flag_value_from_args(short: &str, long: &str) -> Option<std::ffi::OsString> {
    let mut args = std::env::args_os().peekable();
    // Skip everything up to and including the "--" separator inserted by the
    // shell completion wrapper.
    for a in args.by_ref() {
        if a == "--" {
            break;
        }
    }
    let mut found: Option<std::ffi::OsString> = None;
    while let Some(a) = args.next() {
        let s = a.to_string_lossy();
        // --flag VALUE  or  -F VALUE  (value as a separate token)
        if s == long || s == short {
            if let Some(val) = args.next() {
                found = Some(val);
            }
        }
        // --flag=VALUE
        else if let Some(rest) = s.strip_prefix(&format!("{long}=")) {
            found = Some(std::ffi::OsString::from(rest));
        }
    }
    found
}

// ── Descriptor helpers ────────────────────────────────────────────────────────

/// Enumerate fully-qualified message type names from raw descriptor bytes
/// (`FileDescriptorSet` or `FileDescriptorProto`).
pub fn message_names_from_descriptor(bytes: &[u8]) -> Vec<String> {
    fn collect(pkg_prefix: &str, messages: &[prost_types::DescriptorProto], out: &mut Vec<String>) {
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
    incomplete: &std::ffi::OsStr,
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
            .unwrap_or(std::ffi::OsStr::new(""))
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

/// Complete `.pb` descriptor files relative to cwd.
pub fn complete_pb_files(incomplete: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    complete_path_under(incomplete, &base, Some(".pb"), false)
}

/// Complete `.rkyv` schema DB files relative to cwd.
pub fn complete_db_path(incomplete: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    complete_path_under(incomplete, &base, Some(".rkyv"), false)
}

/// Complete any file or directory path relative to cwd.
pub fn complete_any_path(incomplete: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    complete_path_under(incomplete, &base, None, false)
}

/// Complete directory paths only relative to cwd.
pub fn complete_dir_path(incomplete: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    complete_path_under(incomplete, &base, None, true)
}

/// Complete message type names.
///
/// Priority:
/// 1. `--descriptor`/`-D` on the partial command line → read that file.
/// 2. `--db`/`PROTOTEXT_DB` → load the sibling `<stem>/schemas.pb`.
/// 3. Embedded descriptor (all `google.protobuf.*` types).
pub fn complete_type_names(incomplete: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    use crate::run::schemas_pb_from_db;

    let bytes: std::borrow::Cow<[u8]> =
        if let Some(path) = flag_value_from_args("-D", "--descriptor") {
            match std::fs::read(&path) {
                Ok(b) => std::borrow::Cow::Owned(b),
                Err(_) => return vec![],
            }
        } else if let Some(db_path) =
            flag_value_from_args("", "--db").or_else(|| std::env::var_os("PROTOTEXT_DB"))
        {
            let schemas_pb = schemas_pb_from_db(Path::new(&db_path));
            if schemas_pb.exists() {
                match std::fs::read(&schemas_pb) {
                    Ok(b) => std::borrow::Cow::Owned(b),
                    Err(_) => std::borrow::Cow::Borrowed(EMBEDDED_DESCRIPTOR),
                }
            } else {
                std::borrow::Cow::Borrowed(EMBEDDED_DESCRIPTOR)
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
pub fn complete_input_paths(incomplete: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let base = flag_value_from_args("-I", "--input-root")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    complete_path_under(incomplete, &base, None, false)
}
