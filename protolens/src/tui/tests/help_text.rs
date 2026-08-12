// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The structural link `HELP_TEXT` does not otherwise have.
//!
//! `HELP_TEXT` is a flat block of prose, deliberately — it is phrased for
//! a reader, in an order the dispatchers do not have, and generating it
//! from the match arms would cost exactly that. The price is that adding
//! a binding and forgetting the help produces no error anywhere, and a
//! key nobody documents is a key nobody finds.
//!
//! So the link is made here instead: the dispatchers' own source text is
//! read back at compile time and every key literal in it is required to
//! appear in the help. This is a weaker check than generation — it says
//! a key is *mentioned*, not that what is written about it is true — but
//! it is the half that drifts silently, and it found two live gaps (`v`
//! and the management pane's `f`) plus two undocumented commands when it
//! was first written.

use super::super::*;

/// Every `.rs` file under `src/tui/` that can bind a key. Read as text,
/// because the alternative is a runtime probe of every character against
/// every pane, which cannot tell "unbound" from "bound to a no-op".
///
/// Listed explicitly rather than globbed: `include_str!` needs a literal,
/// and a file missing from this list is a file whose bindings go
/// unchecked, so it should be a visible line in a diff.
const DISPATCHER_SOURCES: &[(&str, &str)] = &[
    ("key_dispatch.rs", include_str!("../key_dispatch.rs")),
    ("command_line.rs", include_str!("../command_line.rs")),
    ("manage_pane.rs", include_str!("../manage_pane.rs")),
    ("override_select.rs", include_str!("../override_select.rs")),
    ("navigation.rs", include_str!("../navigation.rs")),
    ("mouse.rs", include_str!("../mouse.rs")),
    ("menu.rs", include_str!("../menu.rs")),
];

/// Characters that are documented as part of a chord rather than on their
/// own line, which is the right way to document them — `zc` is not a `z`
/// binding and a `c` binding, it is one thing the reader looks up as
/// `zc`. Spelled out so that the exemption is a decision someone made
/// rather than a hole in the scan.
const DOCUMENTED_AS_CHORDS: &[(char, &str)] = &[
    ('g', "gg, under Home"),
    ('c', "zc, under the fold section"),
    ('C', "zC, likewise"),
    ('b', "xb and xdb, under the export chord"),
    ('p', "xp and xdp, likewise"),
];

/// Whether `c` appears in `haystack` as a token in its own right, rather
/// than as a letter inside a word.
///
/// The distinction is the whole test. `v` occurs in "available" and in
/// "move", so a plain substring search reports the jump-to-definition
/// binding as documented when it is not — which is exactly what it did
/// before this was tightened.
///
/// Case is ignored, because crossterm reports a modified letter in the
/// case it was typed (`Ctrl-E` arrives as `Char('e')`) while help text
/// conventionally capitalizes after a modifier, as the surrounding
/// `Ctrl-O`/`Ctrl-I`/`Ctrl-Z` entries already do. Asking the two to agree
/// would be asking the help to be wrong.
fn mentioned_as_a_key(haystack: &str, c: char) -> bool {
    let chars: Vec<char> = haystack.chars().collect();
    chars.iter().enumerate().any(|(i, &ch)| {
        ch.eq_ignore_ascii_case(&c)
            && !chars
                .get(i.wrapping_sub(1))
                .is_some_and(|p| p.is_ascii_alphanumeric())
            && !chars.get(i + 1).is_some_and(|n| n.is_ascii_alphanumeric())
    })
}

/// Extracts the character of every `KeyCode::Char('x')` in `src`.
fn bound_chars(src: &str) -> Vec<char> {
    let mut found = Vec::new();
    let pattern = "KeyCode::Char('";
    let mut rest = src;
    while let Some(at) = rest.find(pattern) {
        rest = &rest[at + pattern.len()..];
        let mut chars = rest.chars();
        if let (Some(c), Some('\'')) = (chars.next(), chars.next()) {
            found.push(c);
        }
    }
    found
}

#[test]
fn every_bound_key_is_named_in_the_help() {
    let help = HELP_TEXT.join("\n");
    let exempt: Vec<char> = DOCUMENTED_AS_CHORDS.iter().map(|(c, _)| *c).collect();

    let mut undocumented: Vec<String> = Vec::new();
    for (name, src) in DISPATCHER_SOURCES {
        for c in bound_chars(src) {
            if exempt.contains(&c) || mentioned_as_a_key(&help, c) {
                continue;
            }
            let entry = format!("{c:?} (bound in {name})");
            if !undocumented.contains(&entry) {
                undocumented.push(entry);
            }
        }
    }
    assert!(
        undocumented.is_empty(),
        "these keys are bound but the F1 help never names them: {}",
        undocumented.join(", "),
    );
}

/// The other half of the same drift, and the reason `COMMANDS`' doc
/// comment no longer calls itself the only step needed.
///
/// A command in `COMMANDS` gets prefix dispatch and Tab-completion for
/// free, which is enough to make it work and not nearly enough to make
/// it discoverable: `:proto-root` shipped absent from the help this way.
#[test]
fn every_command_is_named_in_the_help() {
    let help = HELP_TEXT.join("\n");
    let missing: Vec<&str> = COMMANDS
        .iter()
        .copied()
        .filter(|cmd| !help.contains(&format!(":{cmd}")))
        .collect();
    assert!(
        missing.is_empty(),
        "these commands exist but the F1 help never names them: {missing:?}",
    );
}

/// `HELP_TEXT` is rendered into a modal that the user scrolls, and the
/// modal is centered — a line wider than the terminal is simply cut off,
/// with no wrap and no indication that anything was lost.
#[test]
fn no_help_line_is_too_wide_to_read() {
    // The narrowest terminal the help is worth promising anything on.
    // The modal takes a border and some margin off that, hence the slack.
    const LIMIT: usize = 72;
    let long: Vec<&&str> = HELP_TEXT
        .iter()
        .filter(|l| l.chars().count() > LIMIT)
        .collect();
    assert!(
        long.is_empty(),
        "help lines wider than {LIMIT} columns are cut off, not wrapped: {long:#?}",
    );
}
