// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The command line being completed, read by a completer.
//!
//! clap hands a value completer the word under the cursor alone. A
//! completion that depends on another option (`--descriptor-set`'s file,
//! `-I`'s roots) reads the words already typed itself. zsh and fish pass
//! them as argv, after a `--` sentinel; bash passes the raw line instead
//! (spec 0406), which [`Bash`](crate::Bash) splits and records here first.
//! Shared by prototext, protolens and protoback.

use std::ffi::{OsStr, OsString};
use std::sync::OnceLock;

/// The words of the line bash completes, as [`Bash`](crate::Bash) split
/// them.
static WORDS: OnceLock<Vec<OsString>> = OnceLock::new();

/// Record the words of the line being completed (bash's raw line, split).
pub(crate) fn set_words(words: Vec<OsString>) {
    let _ = WORDS.set(words);
}

/// Every value of the option `short` or `long` on the command line being
/// completed, in order, each [expanded](expand_shell_word). Spelled
/// `--long VALUE`, `--long=VALUE`, `-S VALUE` or `-SVALUE`.
pub fn flag_values(short: Option<&str>, long: &str) -> Vec<OsString> {
    flag_values_in(completion_words(), short, long)
}

/// The last value of the option, as [`flag_values`] reads it.
pub fn flag_value(short: Option<&str>, long: &str) -> Option<OsString> {
    flag_values(short, long).pop()
}

/// The words typed: bash's line as split, else argv after the `--` the
/// completion script passes.
fn completion_words() -> Vec<OsString> {
    if let Some(words) = WORDS.get() {
        return words.clone();
    }
    std::env::args_os()
        .skip_while(|a| a != "--")
        .skip(1)
        .collect()
}

fn flag_values_in(words: Vec<OsString>, short: Option<&str>, long: &str) -> Vec<OsString> {
    let mut found = Vec::new();
    let mut words = words.into_iter();
    let long_eq = format!("{long}=");
    while let Some(w) = words.next() {
        let s = w.to_string_lossy();
        if s == long || short.is_some_and(|sh| s == sh) {
            if let Some(v) = words.next() {
                found.push(expand_shell_word(&v));
            }
        } else if let Some(rest) = s.strip_prefix(&long_eq) {
            found.push(expand_shell_word(OsStr::new(rest)));
        } else if let Some(rest) = short.and_then(|sh| s.strip_prefix(sh)) {
            if !rest.is_empty() && !s.starts_with("--") {
                found.push(expand_shell_word(OsStr::new(rest)));
            }
        }
    }
    found
}

/// Expand the shell forms a completion argv can still contain: `$NAME`,
/// `${NAME}`, and a leading `~`.
///
/// The words a completer is handed are the words as *typed*. Nothing
/// downstream expands them, so a value naming a variable has to be
/// resolved here or the file simply cannot be opened — and since a
/// completer that finds nothing looks exactly like one that is not
/// installed, this failed silently.
///
/// Quoting, command substitution and globbing are deliberately left
/// alone. They are the shell's, and guessing at them would be wrong in
/// ways nobody could see either. An unset variable expands to nothing,
/// which is what the shell does.
pub fn expand_shell_word(raw: &OsStr) -> OsString {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &[&str]) -> Vec<OsString> {
        s.iter().map(OsString::from).collect()
    }

    #[test]
    fn every_spelling_of_an_option_is_read_in_order() {
        let got = flag_values_in(
            words(&[
                "-I",
                "a",
                "--desc-root",
                "b",
                "--desc-root=c",
                "-Id",
                "x.pb",
                "-O",
                "o",
            ]),
            Some("-I"),
            "--desc-root",
        );
        assert_eq!(got, words(&["a", "b", "c", "d"]));
    }

    #[test]
    fn a_long_option_is_not_taken_for_its_short_spelling() {
        // `--input` is not `-I` followed by `nput`.
        let got = flag_values_in(words(&["--input", "x"]), Some("-I"), "--desc-root");
        assert!(got.is_empty());
        let got = flag_values_in(
            words(&["--descriptor-set", "a.desc"]),
            None,
            "--descriptor-set",
        );
        assert_eq!(got, words(&["a.desc"]));
    }

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

        // A value read from the command line is expanded.
        let got = flag_values_in(words(&["-I", "$HOME"]), Some("-I"), "--desc-root");
        assert_eq!(got, vec![OsString::from(home)]);
    }
}
