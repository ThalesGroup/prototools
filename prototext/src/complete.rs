// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Dynamic shell completion.
//!
//! `protolens/src/complete.rs` is this file's near-twin and carries the
//! same `expand_shell_word` and the same argv scanner. They are separate
//! crates and the duplication is deliberate — one shared helper crate for
//! forty lines would be a workspace-wide dependency edge to save a copy.
//! What is *not* optional is that the two stay honest about the same two
//! facts, so both are recorded in each:
//!
//! - The words a completer is handed are `COMP_WORDS`, the command line
//!   as **typed**. `--descriptor-set $PROTOTEXT_DESCRIPTOR_SET` arrives
//!   as those literal characters and names no file until
//!   `expand_shell_word` has run.
//! - A clap `env = ...` fallback is applied while *parsing*, a step the
//!   completion path returns before reaching, so every variable the flag
//!   resolves through has to be read here as well.

use std::ffi::{OsStr, OsString};
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
pub fn flag_value_from_args(short: &str, long: &str) -> Option<OsString> {
    let mut args = std::env::args_os().peekable();
    // Skip everything up to and including the "--" separator inserted by the
    // shell completion wrapper.
    for a in args.by_ref() {
        if a == "--" {
            break;
        }
    }
    let mut found: Option<OsString> = None;
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
            found = Some(OsString::from(rest));
        }
    }
    found.map(|v| expand_shell_word(&v))
}

/// Expand the shell forms a completion argv can still contain: `$NAME`,
/// `${NAME}`, and a leading `~`.
///
/// The shell hands a completer the words as *typed* — verified against an
/// interactive bash 5.2 under a pty, where
/// `--descriptor-set $PROTOTEXT_DESCRIPTOR_SET <TAB>` reaches the
/// function as the literal `$PROTOTEXT_DESCRIPTOR_SET`. Nothing
/// downstream expands them, so a value naming a variable has to be
/// resolved here or the file cannot be opened — and a completer that
/// finds nothing looks exactly like one that is not installed, which is
/// how this stayed hidden.
///
/// Quoting, command substitution and globbing are deliberately left
/// alone: they are the shell's, and guessing at them would be wrong in
/// ways nobody could see either. An unset variable expands to nothing,
/// which is what the shell does.
fn expand_shell_word(raw: &OsStr) -> OsString {
    let s = raw.to_string_lossy();
    if !s.starts_with('~') && !s.contains('$') {
        return raw.to_os_string();
    }

    let mut out = String::new();
    let mut rest = &s[..];
    if let Some(tail) = rest.strip_prefix('~') {
        if tail.is_empty() || tail.starts_with('/') {
            if let Some(home) = std::env::var_os("HOME") {
                out.push_str(&home.to_string_lossy());
                rest = tail;
            }
        }
    }

    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = chars.peek() == Some(&'{');
        if braced {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&n) = chars.peek() {
            if n.is_ascii_alphanumeric() || n == '_' {
                name.push(n);
                chars.next();
            } else {
                break;
            }
        }
        if braced && chars.peek() == Some(&'}') {
            chars.next();
        }
        if name.is_empty() && !braced {
            // A bare `$`, which the shell leaves alone.
            out.push('$');
            continue;
        }
        if let Ok(value) = std::env::var(&name) {
            out.push_str(&value);
        }
    }

    OsString::from(out)
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
    let bytes: std::borrow::Cow<[u8]> = if let Some(path) =
        flag_value_from_args("", "--descriptor-set")
            .or_else(|| flag_value_from_args("", "--descriptor"))
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
    let base = flag_value_from_args("-I", "--input-root")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    complete_path_under(incomplete, &base, None, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The twin of protolens's
    /// `a_completion_argv_value_is_expanded_before_it_names_a_file`.
    ///
    /// `--descriptor-set $PROTOTEXT_DESCRIPTOR_SET -t <TAB>` completed
    /// against the *embedded* descriptor, because the flag's value was
    /// taken as a filename of nineteen literal characters and the read
    /// failed into the fallback. Wrong type list, no error, and the
    /// fallback made it look like completion was working.
    ///
    /// Asserted against `HOME` and `PATH`, which any environment running
    /// this test has, so nothing here mutates the process environment.
    #[test]
    fn a_completion_argv_value_is_expanded_before_it_names_a_file() {
        let home = std::env::var("HOME").expect("a test environment has HOME");
        let path = std::env::var("PATH").expect("a test environment has PATH");

        let expand = |s: &str| {
            expand_shell_word(OsStr::new(s))
                .to_string_lossy()
                .into_owned()
        };

        assert_eq!(expand("$HOME"), home);
        assert_eq!(expand("${HOME}"), home);
        assert_eq!(expand("$HOME/wkt.desc"), format!("{home}/wkt.desc"));
        assert_eq!(
            expand("${HOME}x"),
            format!("{home}x"),
            "a brace ends the name"
        );
        assert_eq!(expand("~/wkt.desc"), format!("{home}/wkt.desc"));
        assert_eq!(expand("$PATH"), path);

        // Left alone: an ordinary path, a `~` that is not the whole first
        // component, and a bare `$`.
        assert_eq!(expand("/nix/store/abc/wkt.desc"), "/nix/store/abc/wkt.desc");
        assert_eq!(expand("~notauser/x"), "~notauser/x");
        assert_eq!(expand("a$b"), "a");
        assert_eq!(expand("costs $"), "costs $");

        // An unset variable expands to nothing, as in the shell.
        assert_eq!(expand("$PROTOTEXT_NO_SUCH_VAR_0197/x"), "/x");
    }

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
