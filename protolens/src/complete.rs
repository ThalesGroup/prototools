// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Dynamic shell completion — mirrors `prototext`'s own `complete.rs`
//! (`prototext/src/complete.rs`).
//!
//! `complete_type_names` scans the partially-typed command line for
//! `--descriptor-set`, reads that descriptor file, and lists its message
//! type names filtered by the prefix typed so far.
//!
//! Two things about that scan are easy to get wrong, and both were. The
//! words the shell hands a completer are **unexpanded** — the value of
//! `--descriptor-set $PROTOTEXT_WKT_SET` arrives as those nineteen
//! literal characters — so a value has to go through `expand_shell_word`
//! before it names a file. And `--descriptor-set` may not be on the line
//! at all: it carries `env = DESCRIPTOR_SET_ENV`, which clap applies at
//! parse time and a completer sees nothing of, so the env var has to be
//! consulted here too or a session set up entirely through it completes
//! `--type` to nothing.
//!
//! `complete_path_under` (and its thin wrappers) provide path completion
//! for `--descriptor-set`, `--proto-root`, and the positional `blob`
//! argument, in place of clap_complete's built-in default path completer.
//! That built-in completer self-appends a trailing `/` to directory
//! candidates (`clap_complete::engine::custom::complete_path`), which
//! collides with the shell's own directory marking and produces a
//! doubled `//`. Omitting the trailing slash here — letting the shell add
//! it alone — avoids the stutter.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use clap_complete::engine::CompletionCandidate;
use prototext_core::decode_pool;
use prototext_graph::score::load::load_graph;

use crate::decode::read_descriptor_file;

/// The env var `--descriptor-set` falls back to, named once so the flag
/// declaration in `main.rs` and the completer below cannot drift apart.
/// Shared with `prototext`/`reproto` (spec 0090).
pub const DESCRIPTOR_SET_ENV: &str = "PROTOTEXT_DESCRIPTOR_SET";

// ── Partial-args scanner ──────────────────────────────────────────────────────

/// Scan the partial command line (`std::env::args_os`, post `--`) for a flag
/// with a value. Returns the value of the last occurrence of `long`, e.g.
/// `flag_value_from_args("--descriptor-set")`.
///
/// When the shell invokes the binary for completion it passes all
/// already-typed words as argv after a `--` sentinel, so `args_os()` is
/// safe to read here.
fn flag_value_from_args(long: &str) -> Option<OsString> {
    let mut args = std::env::args_os().peekable();
    for a in args.by_ref() {
        if a == "--" {
            break;
        }
    }
    let mut found = None;
    while let Some(a) = args.next() {
        let s = a.to_string_lossy();
        // --flag VALUE (value as a separate token)
        if s == long {
            if let Some(val) = args.next() {
                found = Some(val);
            }
        }
        // --flag=VALUE
        else if let Some(rest) = s.strip_prefix(&format!("{long}=")) {
            found = Some(OsString::from(rest));
        }
    }
    found
}

/// Expand the shell forms a completion argv can still contain: `$NAME`,
/// `${NAME}`, and a leading `~`.
///
/// The words a completer is handed are `COMP_WORDS`, which are the words
/// as *typed*. Nothing downstream expands them, so a value naming a
/// variable has to be resolved here or the file simply cannot be opened —
/// and since a completer that finds nothing looks exactly like one that
/// is not installed, this failed silently.
///
/// Quoting, command substitution and globbing are deliberately left
/// alone. They are the shell's, and guessing at them would be wrong in
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

/// The descriptor set a `--type` completion resolves against: whichever
/// `--descriptor-set` is already on the partial command line, else
/// [`DESCRIPTOR_SET_ENV`].
///
/// A typed flag wins even when it turns out to be unreadable — the user
/// named a file, and quietly completing against a different one would be
/// worse than completing against nothing.
fn descriptor_set_for_completion() -> Option<PathBuf> {
    flag_value_from_args("--descriptor-set")
        .map(|v| expand_shell_word(&v))
        .or_else(|| std::env::var_os(DESCRIPTOR_SET_ENV))
        .map(PathBuf::from)
}

/// Complete message type names for `--type`, reading whichever
/// `--descriptor-set` is in effect (see `descriptor_set_for_completion`).
/// Empty if there is none, or it is unreadable, or invalid.
///
/// Prefers the `hopcroft.rkyv` sidecar, whose root list is exactly the
/// descriptor set's message types — the set `--type` accepts, enums
/// excluded — and which costs a mmap rather than a full `decode_pool`
/// (25 MB on googleapis, per TAB). Spec 0197 §S4. The fallback to
/// decoding is silent here: a completion subprocess has no stderr a user
/// reads and no `App` to hold a message, and the same descriptor set
/// produces the ordinary warning on the next real launch.
pub fn complete_type_names(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    let Some(path) = descriptor_set_for_completion() else {
        return vec![];
    };
    type_names_from_descriptor(&path, &incomplete.to_string_lossy())
        .into_iter()
        .map(CompletionCandidate::new)
        .collect()
}

/// The candidate names themselves, split out from the argv scan above so
/// they can be tested — a completion subprocess's input is `args_os()`,
/// which a unit test cannot supply.
fn type_names_from_descriptor(path: &Path, prefix: &str) -> Vec<String> {
    let rkyv_path = path.with_extension("").join("hopcroft.rkyv");
    if rkyv_path.exists() {
        if let Ok(graph) = load_graph(&rkyv_path) {
            return graph
                .graph()
                .roots
                .iter()
                .map(|r| r.fqdn.as_str().to_string())
                .filter(|name| name.starts_with(prefix))
                .collect();
        }
    }

    let Ok(bytes) = read_descriptor_file(path) else {
        return vec![];
    };
    let Ok(pool) = decode_pool(&bytes) else {
        return vec![];
    };

    pool.all_messages()
        .map(|m| m.full_name().to_string())
        .filter(|name| name.starts_with(prefix))
        .collect()
}

// ── Filesystem listing helpers ────────────────────────────────────────────────

/// Complete paths under `base` (the effective root directory), mirroring
/// clap_complete's own `complete_path` logic.
///
/// - `incomplete` is the raw token the user has typed so far.
/// - Its directory part is resolved relative to `base`; its filename part
///   is used as a prefix filter on directory entries.
/// - Directories and files are both returned, without a trailing `/` on
///   directories — the shell adds it on its own.
/// - Each candidate is the full value to insert (same format as the
///   built-in `PathCompleter`).
fn complete_path_under(
    incomplete: &OsStr,
    base: &Path,
    dirs_only: bool,
) -> Vec<CompletionCandidate> {
    let s = incomplete.to_string_lossy();
    let incomplete_path = Path::new(incomplete);

    // Split into the already-typed directory prefix and the partial
    // filename. A trailing `/` means the user has completed a directory
    // name and wants its contents — treat the whole token as the prefix
    // with an empty stem.
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
                // No trailing slash — the shell adds it.
                let p = typed_prefix.join(&name);
                Some(CompletionCandidate::new(p.as_os_str().to_os_string()))
            } else if ft.is_file() && !dirs_only {
                let p = typed_prefix.join(&name);
                Some(CompletionCandidate::new(p.as_os_str().to_os_string()))
            } else {
                None
            }
        })
        .collect();

    completions.sort_by(|a, b| a.get_value().cmp(b.get_value()));
    completions
}

/// Complete any file or directory path relative to cwd.
pub fn complete_any_path(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    complete_path_under(incomplete, &base, false)
}

/// Complete directory paths only relative to cwd.
pub fn complete_dir_path(incomplete: &OsStr) -> Vec<CompletionCandidate> {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    complete_path_under(incomplete, &base, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 0197 test 15 (§S4). Completion runs once per TAB, so
    /// decoding the whole descriptor set — 25 MB on googleapis — each
    /// time is not affordable.
    ///
    /// That the pool is never touched is proved by making it undecodable:
    /// the descriptor here is not a `FileDescriptorSet` at all, so any
    /// path through `decode_pool` yields an empty list. Getting names back
    /// means they came from the sidecar.
    #[test]
    fn completion_reads_the_sidecar_and_never_decodes_the_pool() {
        use prototext_graph::build_scoring_graph::build_from_strings;

        let dir = std::env::temp_dir().join("protolens-0197-completion");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("schema")).unwrap();
        std::fs::write(dir.join("schema.pb"), b"not a descriptor set").unwrap();

        let yaml = "entries:\n- alpha.Msg\n- beta.Other\nmessages:\n  alpha.Msg:\n    \
                    fields:\n    - number: 1\n      type: uint64\n  beta.Other:\n    \
                    fields:\n    - number: 1\n      type: uint64\n"
            .to_string();
        let (bytes, _, _) =
            build_from_strings(&[yaml], false, false, |_, _| {}).expect("test graph must build");
        std::fs::write(dir.join("schema").join("hopcroft.rkyv"), &bytes).unwrap();

        let pb = dir.join("schema.pb");
        let mut all = type_names_from_descriptor(&pb, "");
        all.sort();
        assert_eq!(all, vec!["alpha.Msg".to_string(), "beta.Other".to_string()]);

        assert_eq!(
            type_names_from_descriptor(&pb, "alpha."),
            vec!["alpha.Msg".to_string()],
            "the typed prefix still filters"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reported bug: `--descriptor-set $PROTOTEXT_WKT_SET --type
    /// google.proto<TAB>` completed to nothing.
    ///
    /// The shell hands a completer `COMP_WORDS` — the words as typed, not
    /// as expanded — so the value arrived as the literal `$PROTOTEXT_WKT_SET`
    /// and `read_descriptor_file` was being asked for a file of that name.
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
        assert_eq!(expand("$PROTOLENS_NO_SUCH_VAR_0197/x"), "/x");
    }

    /// The other half of the same report: `protolens --type
    /// google.proto<TAB>`, with no `--descriptor-set` typed at all,
    /// completed to nothing even though `PROTOTEXT_DESCRIPTOR_SET` was set
    /// and the very same launch would have opened a blob happily.
    ///
    /// clap applies `env` at parse time, which a completion subprocess
    /// never reaches. The const is what keeps the flag and the completer
    /// naming one variable.
    #[test]
    fn the_completer_falls_back_to_the_same_env_var_the_flag_declares() {
        use clap::CommandFactory;

        let cmd = crate::Cli::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_long() == Some("descriptor-set"))
            .expect("--descriptor-set exists");

        assert_eq!(
            arg.get_env().map(|e| e.to_string_lossy().into_owned()),
            Some(DESCRIPTOR_SET_ENV.to_string()),
            "the flag's env var must be the one the completer reads"
        );
    }
}
