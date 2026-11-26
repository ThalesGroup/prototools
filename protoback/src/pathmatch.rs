// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! FQDN and path patterns, matched as reproto matches them.
//!
//! reproto matches `-s`/`-p` patterns, and path patterns, with
//! `PurePosixPath('/' + subject).full_match('/' + pattern)` (Python 3.13,
//! `phases.fqdn_match`): both sides as `PurePosixPath` writes them, the
//! pattern compiled by `glob.translate(recursive=True,
//! include_hidden=True, seps='/')`, anchored at both ends. `*` matches
//! within a segment, `**` as a whole segment any number of segments.
//! [`translate`] is that function, ported; `tests` holds Python's own
//! answers on a table of cases.

use crate::hash::HashMap;
use std::collections::BTreeSet;
use std::sync::Mutex;

use regex::Regex;

/// A path as `PurePosixPath` writes it: separators collapsed (but two
/// leading ones kept, as POSIX allows), `.` segments and a trailing `/`
/// dropped; `.` for an empty path.
pub fn posix_str(path: &str) -> String {
    let root = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else if path.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let joined = parts.join("/");
    if root.is_empty() && joined.is_empty() {
        ".".to_string()
    } else {
        format!("{root}{joined}")
    }
}

/// `fnmatch._translate(pat, STAR, QUESTION_MARK)`, appended to `out`.
fn fnmatch_translate(pat: &[char], star: &str, question: &str, out: &mut Vec<String>) {
    let start = out.len();
    let n = pat.len();
    let mut i = 0;
    while i < n {
        let c = pat[i];
        i += 1;
        match c {
            '*' => {
                // Consecutive `*`s compress into one.
                if out.len() == start || out.last().map(String::as_str) != Some(star) {
                    out.push(star.to_string());
                }
            }
            '?' => out.push(question.to_string()),
            '[' => {
                let mut j = i;
                if j < n && pat[j] == '!' {
                    j += 1;
                }
                if j < n && pat[j] == ']' {
                    j += 1;
                }
                while j < n && pat[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    out.push("\\[".to_string());
                    continue;
                }
                let stuff_raw: String = pat[i..j].iter().collect();
                let stuff = if !stuff_raw.contains('-') {
                    stuff_raw.replace('\\', "\\\\")
                } else {
                    let mut chunks: Vec<String> = Vec::new();
                    let mut ii = i;
                    let mut k = if pat[i] == '!' { i + 2 } else { i + 1 };
                    loop {
                        let found = (k..j).find(|&x| pat[x] == '-');
                        let Some(kk) = found else { break };
                        chunks.push(pat[ii..kk].iter().collect());
                        ii = kk + 1;
                        k = kk + 3;
                    }
                    let chunk: String = pat[ii..j].iter().collect();
                    if !chunk.is_empty() {
                        chunks.push(chunk);
                    } else if let Some(last) = chunks.last_mut() {
                        last.push('-');
                    }
                    // Remove empty ranges, invalid in a regex.
                    let mut idx = chunks.len();
                    while idx > 1 {
                        idx -= 1;
                        let prev_last = chunks[idx - 1].chars().last();
                        let cur_first = chunks[idx].chars().next();
                        if let (Some(a), Some(b)) = (prev_last, cur_first) {
                            if a > b {
                                let mut merged: String = chunks[idx - 1]
                                    .chars()
                                    .take(chunks[idx - 1].chars().count() - 1)
                                    .collect();
                                merged.extend(chunks[idx].chars().skip(1));
                                chunks[idx - 1] = merged;
                                chunks.remove(idx);
                            }
                        }
                    }
                    chunks
                        .iter()
                        .map(|s| s.replace('\\', "\\\\").replace('-', "\\-"))
                        .collect::<Vec<_>>()
                        .join("-")
                };
                // Set operations escaped, as Python escapes them.
                let mut escaped = String::new();
                for ch in stuff.chars() {
                    if matches!(ch, '&' | '~' | '|') {
                        escaped.push('\\');
                    }
                    escaped.push(ch);
                }
                i = j + 1;
                if escaped.is_empty() {
                    // An empty range never matches (`(?!)` in Python).
                    out.push("\\b\\B".to_string());
                } else if escaped == "!" {
                    // A negated empty range matches any character.
                    out.push(".".to_string());
                } else {
                    let class = if let Some(rest) = escaped.strip_prefix('!') {
                        format!("^{rest}")
                    } else if escaped.starts_with('^') || escaped.starts_with('[') {
                        format!("\\{escaped}")
                    } else {
                        escaped
                    };
                    out.push(format!("[{class}]"));
                }
            }
            c => out.push(regex::escape(&c.to_string())),
        }
    }
}

/// `glob.translate(pat, recursive=True, include_hidden=True, seps='/')`.
pub fn translate(pat: &str) -> String {
    let any_sep = "/";
    let not_sep = "[^/]";
    let one_last_segment = format!("{not_sep}+");
    let one_segment = format!("{one_last_segment}{any_sep}");
    let any_segments = format!("(?:.+{any_sep})?");
    let any_last_segments = ".*".to_string();
    let parts: Vec<&str> = pat.split('/').collect();
    let last = parts.len() - 1;
    let mut results: Vec<String> = Vec::new();
    for (idx, part) in parts.iter().enumerate() {
        if *part == "*" {
            results.push(if idx < last {
                one_segment.clone()
            } else {
                one_last_segment.clone()
            });
        } else if *part == "**" {
            if idx < last {
                if parts[idx + 1] != "**" {
                    results.push(any_segments.clone());
                }
            } else {
                results.push(any_last_segments.clone());
            }
        } else {
            if !part.is_empty() {
                let chars: Vec<char> = part.chars().collect();
                fnmatch_translate(&chars, &format!("{not_sep}*"), not_sep, &mut results);
            }
            if idx < last {
                results.push(any_sep.to_string());
            }
        }
    }
    format!("^(?s:{})\\z", results.concat())
}

/// `PurePosixPath(subject).full_match(pattern)`.
pub fn full_match(subject: &str, pattern: &str) -> bool {
    static CACHE: Mutex<Option<HashMap<String, Regex>>> = Mutex::new(None);
    let pattern = posix_str(pattern);
    let subject = posix_str(subject);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = cache.get_or_insert_with(HashMap::default);
    let re = map
        .entry(pattern.clone())
        .or_insert_with(|| Regex::new(&translate(&pattern)).expect("a translated glob is a regex"));
    re.is_match(&subject)
}

/// `phases._normalise_fqdn_name`: `.` to `/` and no leading `/`, except for
/// `file:` and `path:`, which use `/` already.
fn normalise_fqdn_name<'a>(prefix: &str, name: &'a str) -> std::borrow::Cow<'a, str> {
    if prefix == "file" || prefix == "path" {
        std::borrow::Cow::Borrowed(name)
    } else {
        std::borrow::Cow::Owned(name.replace('.', "/").trim_start_matches('/').to_string())
    }
}

/// `phases.fqdn_match(pattern, subject)`.
pub fn fqdn_match(pattern: &str, subject: &str) -> bool {
    let (Some((p_prefix, p_name)), Some((s_prefix, s_name))) =
        (pattern.split_once(':'), subject.split_once(':'))
    else {
        return pattern == subject;
    };
    if p_prefix != s_prefix {
        return false;
    }
    let p = normalise_fqdn_name(p_prefix, p_name);
    let s = normalise_fqdn_name(s_prefix, s_name);
    full_match(&format!("/{s}"), &format!("/{p}"))
}

/// `load.PathPatterns`: root-relative path patterns, literal ones looked
/// up, glob ones matched as `path:` FQDNs.
#[derive(Debug, Default, Clone)]
pub struct PathPatterns {
    literals: BTreeSet<String>,
    globs: Vec<String>,
}

impl PathPatterns {
    pub fn new(patterns: &[String]) -> PathPatterns {
        // A set in reproto: each pattern once.
        let unique: BTreeSet<&String> = patterns.iter().collect();
        let mut p = PathPatterns::default();
        for pat in unique {
            if pat.contains(['*', '?', '[']) {
                p.globs.push(pat.clone());
            } else {
                p.literals.insert(pat.clone());
            }
        }
        p
    }

    /// `rel_path` as `Path.as_posix()` writes it.
    pub fn matches(&self, rel_path: &str) -> bool {
        self.matching(rel_path).is_some()
    }

    /// The pattern `rel_path` matches, if any: a literal first, then the
    /// first glob, as `matches` tries them.
    pub fn matching(&self, rel_path: &str) -> Option<&str> {
        let s = posix_str(rel_path);
        if let Some(l) = self.literals.get(&s) {
            return Some(l);
        }
        self.globs
            .iter()
            .find(|p| fqdn_match(&format!("path:{p}"), &format!("path:{s}")))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod cases {
        include!("pathmatch/cases.rs");
    }

    #[test]
    fn full_match_answers_as_python() {
        let wrong: Vec<_> = cases::CASES
            .iter()
            .filter(|(p, s, want)| full_match(&format!("/{s}"), &format!("/{p}")) != *want)
            .collect();
        assert!(
            wrong.is_empty(),
            "{} of {} differ: {:?}",
            wrong.len(),
            cases::CASES.len(),
            wrong
        );
    }

    #[test]
    fn fqdns_match_by_segment() {
        assert!(fqdn_match("serv:dg/Svc", "serv:.dg.Svc"));
        assert!(fqdn_match("desc:.dg.*", "desc:.dg.Holder"));
        assert!(!fqdn_match("desc:.dg.*", "desc:.dg.Holder.Inner"));
        assert!(fqdn_match("desc:dg/**", "desc:.dg.Holder.Inner"));
        assert!(!fqdn_match("enum:dg/**", "desc:.dg.Holder"));
        assert!(fqdn_match("file:a/*.proto", "file:a/b.proto"));
    }

    #[test]
    fn paths_print_as_pathlib_does() {
        assert_eq!(posix_str("a//b/./c/"), "a/b/c");
        assert_eq!(posix_str(""), ".");
        assert_eq!(posix_str("./"), ".");
        assert_eq!(posix_str("//x"), "//x");
        assert_eq!(posix_str("///x"), "/x");
    }
}

/// The unit tests of `test_path_patterns.py` (spec 0488 S2): U2 on
/// `PathPatterns` (reproto's `load.PathPatterns`), U3 on `fqdn_match`
/// (`phases.fqdn_match`); names lower-cased (`test_U2_` is `test_u2_`). The
/// original's `Fqdn("path:…")` is the string itself here.
#[cfg(test)]
mod test_path_patterns {
    use super::*;

    fn patterns(p: &[&str]) -> PathPatterns {
        PathPatterns::new(&p.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn test_u2_partition_literal_vs_glob() {
        let pp = patterns(&["foo/bar.pb", "foo/*.pb", "foo?.pb", "foo[0].pb"]);
        assert_eq!(pp.literals, BTreeSet::from(["foo/bar.pb".to_string()]));
        let globs: BTreeSet<&str> = pp.globs.iter().map(String::as_str).collect();
        assert_eq!(globs, BTreeSet::from(["foo/*.pb", "foo?.pb", "foo[0].pb"]));
    }

    #[test]
    fn test_u2_literal_matches_exact_string_only() {
        let pp = patterns(&["foo/bar.pb"]);
        assert!(pp.matches("foo/bar.pb"));
        assert!(!pp.matches("foo/barx.pb"));
        assert!(!pp.matches("foo/ba"));
        assert!(!pp.matches("foo/bar.pbx"));
    }

    #[test]
    fn test_u2_glob_matches_via_fqdn_match() {
        let pp = patterns(&["foo/*.pb"]);
        assert!(pp.matches("foo/bar.pb"));
        assert!(!pp.matches("foo/sub/bar.pb"), "* must not cross /");
    }

    #[test]
    fn test_u2_empty_pattern_set_matches_nothing() {
        assert!(!patterns(&[]).matches("anything.pb"));
    }

    #[test]
    fn test_u3_doublestar_matches_top_level_and_nested() {
        assert!(fqdn_match("path:**/*~2.pb", "path:foo~2.pb"));
        assert!(fqdn_match("path:**/*~2.pb", "path:bar/foo~2.pb"));
    }

    #[test]
    fn test_u3_bare_star_matches_only_top_level() {
        assert!(fqdn_match("path:*~2.pb", "path:foo~2.pb"));
        assert!(!fqdn_match("path:*~2.pb", "path:bar/foo~2.pb"));
    }
}

/// The unit tests of `test_prune_seed_patterns.py` (spec 0488 S2), T2's and
/// T5's, on `fqdn_match` (reproto's `phases.fqdn_match`); names
/// lower-cased. reproto's `fqdn_matches_any` was `any(fqdn_match(p, s) for
/// p in patterns)`, which protoback writes where it needs it: its test is
/// that `any`.
#[cfg(test)]
mod test_prune_seed_patterns {
    use super::*;

    /// `*` does not cross `/`; `**` does (file: FQDNs use `/`).
    #[test]
    fn test_t2_star_does_not_cross_slash() {
        assert!(!fqdn_match("file:subdir/*", "file:subdir/nested/foo.proto"));
        assert!(fqdn_match("file:subdir/**", "file:subdir/nested/foo.proto"));
        assert!(fqdn_match("file:subdir/*", "file:subdir/foo.proto"));
    }

    #[test]
    fn test_t5_fqdn_match_exact() {
        assert!(fqdn_match("file:foo.proto", "file:foo.proto"));
        assert!(!fqdn_match("file:foo.proto", "file:bar.proto"));
    }

    #[test]
    fn test_t5_fqdn_match_star() {
        assert!(fqdn_match("file:dir/*.proto", "file:dir/foo.proto"));
        assert!(!fqdn_match("file:dir/*.proto", "file:dir/sub/foo.proto"));
    }

    #[test]
    fn test_t5_fqdn_match_doublestar() {
        assert!(fqdn_match("file:dir/**", "file:dir/sub/foo.proto"));
        assert!(fqdn_match("file:dir/**", "file:dir/foo.proto"));
    }

    /// `.` in desc: names is treated as `/` for matching.
    #[test]
    fn test_t5_fqdn_match_desc_dot_notation() {
        assert!(fqdn_match("desc:my.pkg.*", "desc:.my.pkg.MyMsg"));
        assert!(!fqdn_match("desc:my.pkg.*", "desc:.my.pkg.sub.MyMsg"));
        assert!(fqdn_match("desc:my.pkg.**", "desc:.my.pkg.sub.MyMsg"));
    }

    #[test]
    fn test_t5_fqdn_match_prefix_mismatch() {
        assert!(!fqdn_match("file:foo.proto", "desc:foo.proto"));
    }

    #[test]
    fn test_t5_fqdn_matches_any() {
        let patterns = ["file:a.proto", "file:b*"];
        let any = |s: &str| patterns.iter().any(|p| fqdn_match(p, s));
        assert!(any("file:a.proto"));
        assert!(any("file:baz.proto"));
        assert!(!any("file:c.proto"));
    }
}
