// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0406: bash completion from the raw command line.
//!
//! clap_complete's own bash script hands the binary `COMP_WORDS`, which bash
//! has already split at `COMP_WORDBREAKS` (`=`, `:` and others): the engine
//! sees `--descriptor-set`, `=`, `da` where the user typed
//! `--descriptor-set=da`. The script here never uses `COMP_WORDS`. It passes
//! the raw line up to the cursor and readline's current word (`$2`); this
//! crate splits the line as the shell would, asks clap's engine for whole
//! words, and keeps of each candidate only the part readline replaces.
//!
//! Only bash is replaced: [`shells`] lists clap_complete's other shells
//! unchanged.

pub mod args;

use std::borrow::Cow;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use clap_complete::env::{Elvish, EnvCompleter, Fish, Powershell, Shells, Zsh};

/// clap_complete's built-in shells, with bash replaced by [`Bash`]. For
/// `CompleteEnv::shells`.
pub const fn shells() -> Shells<'static> {
    Shells(&[&Bash, &Elvish, &Fish, &Powershell, &Zsh])
}

/// The registration script. `@NAME@`, `@BIN@`, `@COMPLETER@` and `@VAR@` are
/// filled in as clap_complete fills its own.
///
/// - The completer gets two arguments after `--`: the line up to the cursor
///   and readline's word. `CompleteEnv` passes them to
///   [`EnvCompleter::write_complete`] as they are.
/// - `-o filenames`: completed paths are escaped, and directories are marked.
/// - A single directory gets no trailing space, so the next Tab completes
///   inside it.
const REGISTRATION: &str = r#"
_prototools_complete_@NAME@() {
    local IFS=$'\013'
    COMPREPLY=( $( \
        _CLAP_IFS="$IFS" \
        @VAR@="bash" \
        @COMPLETER@ -- "${COMP_LINE:0:$COMP_POINT}" "$2" \
    ) )
    if [[ $? != 0 ]]; then
        unset COMPREPLY
    elif [[ ${#COMPREPLY[@]} -eq 1 && ${COMPREPLY[0]} == */ ]]; then
        compopt -o nospace
    fi
}
if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -o filenames -o bashdefault -o nosort -F _prototools_complete_@NAME@ @BIN@
else
    complete -o filenames -o bashdefault -F _prototools_complete_@NAME@ @BIN@
fi
"#;

/// Bash completion from the raw command line (spec 0406 S1, S2).
pub struct Bash;

impl EnvCompleter for Bash {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn is(&self, name: &str) -> bool {
        name == "bash"
    }

    fn write_registration(
        &self,
        var: &str,
        name: &str,
        bin: &str,
        completer: &str,
        buf: &mut dyn Write,
    ) -> Result<(), std::io::Error> {
        let completer = shlex::try_quote(completer).unwrap_or(Cow::Borrowed(completer));
        let script = REGISTRATION
            .replace("@NAME@", &name.replace('-', "_"))
            .replace("@BIN@", bin)
            .replace("@COMPLETER@", &completer)
            .replace("@VAR@", var);
        writeln!(buf, "{script}")
    }

    fn write_complete(
        &self,
        cmd: &mut clap::Command,
        args: Vec<OsString>,
        current_dir: Option<&Path>,
        buf: &mut dyn Write,
    ) -> Result<(), std::io::Error> {
        let arg = |i: usize| {
            args.get(i)
                .map(|a| a.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let candidates = complete_line(cmd, &arg(0), &arg(1), current_dir)?;
        let ifs = std::env::var("_CLAP_IFS").unwrap_or_else(|_| "\n".to_owned());
        write!(buf, "{}", candidates.join(&ifs))
    }
}

/// The candidates for the last word of `line`, each cut to the part readline
/// replaces.
///
/// `line` is the command line up to the cursor; `word` is readline's own
/// current word, `$2`. readline replaces only that word, which starts after
/// the last `COMP_WORDBREAKS` character or after an opening quote, so each
/// whole-word candidate from clap loses the part of the shell word before
/// it.
pub fn complete_line(
    cmd: &mut clap::Command,
    line: &str,
    word: &str,
    current_dir: Option<&Path>,
) -> Result<Vec<String>, std::io::Error> {
    let split = split_line(line);
    let index = split.words.len() - 1;
    let current = split.words[index].clone();
    let args: Vec<OsString> = split.words.into_iter().map(OsString::from).collect();
    // For completers that read the other words (`args::flag_values`): bash
    // hands the binary the raw line, not its words.
    args::set_words(args.clone());
    let candidates = clap_complete::engine::complete(cmd, args, index, current_dir)?;
    let values = candidates
        .iter()
        .map(|c| mark_directory(c.get_value().to_string_lossy().into_owned(), current_dir));
    Ok(trim_candidates(values, &current, word, split.open_quote))
}

/// A candidate naming a directory ends in `/`.
///
/// clap's engine returns directories without one. readline's `-o filenames`
/// adds it only when the text it inserts is itself a directory: right for
/// `da` → `data`, wrong after a `:` or `=`, where it inserts `b` of `a:b`
/// and finds no `b`. For `--opt=value` and `-o=value` candidates, the value
/// is what is tested.
fn mark_directory(mut value: String, current_dir: Option<&Path>) -> String {
    let path = match value.split_once('=') {
        Some((flag, path)) if flag.starts_with('-') => path,
        _ => value.as_str(),
    };
    let is_dir = !path.is_empty()
        && match current_dir {
            Some(dir) => dir.join(path).is_dir(),
            None => Path::new(path).is_dir(),
        };
    if is_dir && !value.ends_with('/') {
        value.push('/');
    }
    value
}

/// Spec 0406 S2 step 3: keep of each candidate what readline replaces.
///
/// Inside an open quote readline's word is the literal text after the quote;
/// elsewhere it is shell text, unquoted here as the line is. Inside a quote a
/// directory loses its trailing `/`: readline adds one itself after closing
/// the quote.
fn trim_candidates(
    candidates: impl Iterator<Item = String>,
    current: &str,
    word: &str,
    open_quote: bool,
) -> Vec<String> {
    let kept = if open_quote {
        word.chars().count()
    } else {
        split_line(word)
            .words
            .last()
            .map_or(0, |w| w.chars().count())
    };
    let lead_len = current.chars().count().saturating_sub(kept);
    let lead: String = current.chars().take(lead_len).collect();
    candidates
        .filter_map(|mut value| {
            if open_quote && value.ends_with('/') {
                value.pop();
            }
            value.strip_prefix(&lead).map(str::to_owned)
        })
        .collect()
}

/// A command line split into shell words.
#[derive(Debug, PartialEq)]
struct Split {
    /// The words, unquoted. Never empty: the last is the word at the cursor,
    /// empty when the line ends in unquoted whitespace.
    words: Vec<String>,
    /// Whether the cursor is inside a quote that the line leaves open.
    open_quote: bool,
}

/// Split `line` into words as bash would: whitespace separates them; single
/// quotes are literal; inside double quotes a backslash escapes only `"`,
/// `\`, `` ` ``, `$` and a newline; outside quotes it escapes any character.
/// Anything else (`$'…'`, variables, redirections) is taken literally (spec
/// 0406 N3).
fn split_line(line: &str) -> Split {
    #[derive(Clone, Copy, PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote = Quote::None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Quote::Single => match c {
                '\'' => quote = Quote::None,
                _ => current.push(c),
            },
            Quote::Double => match c {
                '"' => quote = Quote::None,
                '\\' => match chars.peek() {
                    Some('"' | '\\' | '`' | '$') => current.push(chars.next().unwrap_or(c)),
                    Some('\n') => {
                        chars.next();
                    }
                    _ => current.push(c),
                },
                _ => current.push(c),
            },
            Quote::None => match c {
                ' ' | '\t' | '\n' => {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
                '\'' => {
                    quote = Quote::Single;
                    in_word = true;
                }
                '"' => {
                    quote = Quote::Double;
                    in_word = true;
                }
                '\\' => {
                    in_word = true;
                    match chars.next() {
                        Some('\n') | None => {}
                        Some(next) => current.push(next),
                    }
                }
                _ => {
                    current.push(c);
                    in_word = true;
                }
            },
        }
    }
    words.push(current);
    Split {
        words,
        open_quote: quote != Quote::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(line: &str) -> (Vec<String>, bool) {
        let s = split_line(line);
        (s.words, s.open_quote)
    }

    fn words(words: &[&str], open_quote: bool) -> (Vec<String>, bool) {
        (words.iter().map(|w| w.to_string()).collect(), open_quote)
    }

    #[test]
    fn whitespace_separates_words_and_a_trailing_space_starts_an_empty_one() {
        assert_eq!(
            split("prototext decode da"),
            words(&["prototext", "decode", "da"], false)
        );
        assert_eq!(
            split("prototext  decode\t"),
            words(&["prototext", "decode", ""], false)
        );
        assert_eq!(split(""), words(&[""], false));
    }

    #[test]
    fn quotes_and_backslashes_are_removed() {
        assert_eq!(split(r#"p "dir w"#), words(&["p", "dir w"], true));
        assert_eq!(split("p 'dir w"), words(&["p", "dir w"], true));
        assert_eq!(split(r"p dir\ w"), words(&["p", "dir w"], false));
        assert_eq!(
            split(r#"p "a\"b" 'c\d'"#),
            words(&["p", "a\"b", r"c\d"], false)
        );
        assert_eq!(split(r#"p "x\y""#), words(&["p", r"x\y"], false));
        assert_eq!(
            split(r#"p --set="dir w"#),
            words(&["p", "--set=dir w"], true)
        );
    }

    #[test]
    fn breaks_bash_would_split_at_stay_inside_the_word() {
        assert_eq!(
            split("p --descriptor-set=da"),
            words(&["p", "--descriptor-set=da"], false)
        );
        assert_eq!(
            split("p decode a:b/"),
            words(&["p", "decode", "a:b/"], false)
        );
    }

    #[test]
    fn utf8_is_kept_whole() {
        assert_eq!(split("p decode ét"), words(&["p", "decode", "ét"], false));
    }

    fn trim(candidates: &[&str], current: &str, word: &str, open_quote: bool) -> Vec<String> {
        trim_candidates(
            candidates.iter().map(|c| c.to_string()),
            current,
            word,
            open_quote,
        )
    }

    /// One case per row of spec 0406's table: `current` is the shell word as
    /// split, `word` readline's `$2`.
    #[test]
    fn a_candidate_keeps_what_readline_replaces() {
        assert_eq!(trim(&["data/"], "da", "da", false), ["data/"]);
        assert_eq!(
            trim(&["dir with space/"], "dir w", r"dir\ w", false),
            ["dir with space/"]
        );
        assert_eq!(
            trim(&["dir with space/"], "dir w", "dir w", true),
            ["dir with space"]
        );
        // readline's word after `a:` is empty: it inserts `b/` after the `:`.
        assert_eq!(trim(&["a:b/"], "a:", "", false), ["b/"]);
        assert_eq!(trim(&["a:b/z.pb"], "a:b/", "b/", false), ["b/z.pb"]);
        assert_eq!(trim(&["ka:li.pb"], "ka:", "", false), ["li.pb"]);
        assert_eq!(
            trim(
                &["--descriptor-set=data/"],
                "--descriptor-set=da",
                "da",
                false
            ),
            ["data/"]
        );
        assert_eq!(
            trim(
                &["--descriptor-set=dir with space/"],
                "--descriptor-set=dir w",
                r"dir\ w",
                false
            ),
            ["dir with space/"]
        );
        assert_eq!(
            trim(
                &["--descriptor-set=dir with space/"],
                "--descriptor-set=dir w",
                "dir w",
                true
            ),
            ["dir with space"]
        );
        assert_eq!(trim(&["été/"], "ét", "ét", false), ["été/"]);
        assert_eq!(
            trim(&["--type", "--help"], "--t", "--t", false),
            ["--type", "--help"]
        );
    }

    #[test]
    fn a_directory_candidate_gets_its_slash() {
        let root = std::env::temp_dir().join(format!("prototools-complete-{}", std::process::id()));
        std::fs::create_dir_all(root.join("a:b")).unwrap();
        std::fs::write(root.join("f.pb"), b"").unwrap();
        let mark = |v: &str| mark_directory(v.to_owned(), Some(&root));
        assert_eq!(mark("a:b"), "a:b/");
        assert_eq!(mark("a:b/"), "a:b/");
        assert_eq!(mark("--descriptor-set=a:b"), "--descriptor-set=a:b/");
        assert_eq!(mark("f.pb"), "f.pb");
        assert_eq!(mark("--type"), "--type");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_candidate_not_extending_the_typed_text_is_dropped() {
        assert_eq!(
            trim(
                &["--descriptor-set=data/", "--other"],
                "--descriptor-set=da",
                "da",
                false
            ),
            ["data/"]
        );
    }
}
