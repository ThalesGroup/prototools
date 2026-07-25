// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Syntax-highlighting colorizer for rendered textproto text (spec 0116
//! §7). Parses protolens's own already-rendered textproto text (produced
//! by `decode_and_render_indexed`'s `TextSink`) with the linked
//! `tree-sitter-textproto` grammar and turns `queries/highlights.scm`'s
//! captures into `StyleHint`s — no `ratatui::style::Color`/`Style` is
//! produced here (that's `theme.rs`'s job, spec 0116 §9).

use std::ops::Range;
use std::sync::OnceLock;

use tree_sitter::Language;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

// The real symbol exported by the linked `tree-sitter-textproto` static
// library (`build.rs`) — upstream's own `bindings/rust/lib.rs` is an
// unfilled `tree-sitter-cli` scaffold template (never renamed from
// `tree_sitter_YOUR_LANGUAGE_NAME`), so this crate declares its own
// correctly-named `extern` binding directly, the same precedent already
// set by this repo's own `reproto/tree-sitter-textproto/binding.c`.
unsafe extern "C" {
    fn tree_sitter_textproto() -> Language;
}

fn language() -> Language {
    unsafe { tree_sitter_textproto() }
}

/// Compiled into the binary at build time from the Nix-built grammar's
/// own committed query file — `build.rs` forwards
/// `TREE_SITTER_TEXTPROTO_QUERIES_DIR` via `cargo:rustc-env` so this
/// `env!()` resolves at compile time.
static HIGHLIGHTS_QUERY: &str = include_str!(concat!(
    env!("TREE_SITTER_TEXTPROTO_QUERIES_DIR"),
    "/highlights.scm"
));

/// One semantic role a rendered text span can have — one variant per
/// capture name in `queries/highlights.scm`. `Copy`, tag-sized — kept
/// cheap so `StyleHint`s are inexpensive to cache (§8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyntaxRole {
    Attribute,
    Type,
    StringLiteral,
    StringEscape,
    StringSpecialUrl,
    Comment,
    Number,
    Boolean,
    Constant,
    PunctuationDelimiter,
    PunctuationBracket,
    PunctuationBracketList,
    PunctuationBracketExtension,
}

/// `queries/highlights.scm`'s capture names, in the exact order of
/// `SyntaxRole`'s discriminants — `HighlightConfiguration::configure`'s
/// `recognized_names` list. Every capture name `highlights.scm` emits is
/// present here *exactly* (not just a dotted-prefix ancestor), so
/// `configure`'s longest-match resolution never collapses a capture we
/// care about into some unrelated ancestor the way `tree-sitter
/// highlight`'s CLI default theme does (see spec 0116 §7's
/// investigation notes).
const RECOGNIZED_NAMES: [&str; 13] = [
    "attribute",
    "type",
    "string",
    "string.escape",
    "string.special.url",
    "comment",
    "number",
    "boolean",
    "constant",
    "punctuation.delimiter",
    "punctuation.bracket",
    "punctuation.bracket.list",
    "punctuation.bracket.extension",
];

impl SyntaxRole {
    fn from_highlight_index(index: usize) -> Option<Self> {
        Some(match index {
            0 => Self::Attribute,
            1 => Self::Type,
            2 => Self::StringLiteral,
            3 => Self::StringEscape,
            4 => Self::StringSpecialUrl,
            5 => Self::Comment,
            6 => Self::Number,
            7 => Self::Boolean,
            8 => Self::Constant,
            9 => Self::PunctuationDelimiter,
            10 => Self::PunctuationBracket,
            11 => Self::PunctuationBracketList,
            12 => Self::PunctuationBracketExtension,
            _ => return None,
        })
    }
}

/// A capture's span within the *rendered text*, tagged with its role —
/// deliberately not a color; `theme::style_for` resolves that
/// separately, per theme (§9).
#[derive(Clone, Debug, PartialEq)]
pub struct StyleHint {
    pub range: Range<usize>,
    pub role: SyntaxRole,
}

fn config() -> &'static HighlightConfiguration {
    static CONFIG: OnceLock<HighlightConfiguration> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let mut config =
            HighlightConfiguration::new(language(), "textproto", HIGHLIGHTS_QUERY, "", "")
                .expect("queries/highlights.scm failed to compile");
        config.configure(&RECOGNIZED_NAMES);
        config
    })
}

/// Parses `text` (protolens's own rendered textproto output) with the
/// linked `tree-sitter-textproto` grammar and turns
/// `queries/highlights.scm`'s captures into `StyleHint`s, one per
/// `HighlightEvent::Source` span, using the top of the nested highlight
/// stack (or none, if the stack is empty) — see spec 0116 §7 for why the
/// `tree-sitter-highlight` stack model (rather than raw `Query`/
/// `QueryCursor` run by hand) is required for correct overlapping-
/// capture precedence.
pub fn colorize(text: &str) -> Vec<StyleHint> {
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config(), text.as_bytes(), None, |_| None)
        .expect("tree-sitter-textproto highlighting failed to start");

    let mut hints = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for event in events {
        match event.expect("tree-sitter-textproto highlighting failed") {
            HighlightEvent::HighlightStart(h) => stack.push(h.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if let Some(role) = stack
                    .last()
                    .copied()
                    .and_then(SyntaxRole::from_highlight_index)
                {
                    hints.push(StyleHint {
                        range: start..end,
                        role,
                    });
                }
            }
        }
    }
    hints
}

/// Syntax-highlight spans for a single rendered line, as `(column range,
/// role)` pairs in the line's own byte-offset coordinate system.
///
/// Named because the nested form spells out four levels of generics at
/// every use site, and one of those (`resolve_line_patch`) returns it
/// inside a tuple, which clippy's `type_complexity` rejects outright.
pub type LineStyles = Vec<(Range<usize>, SyntaxRole)>;

/// Buckets `hints` (byte offsets relative to `lines.join("\n")`) into one
/// `LineStyles` per entry of `lines` — the coordinate system
/// `App::line_styles` (`protolens/src/tui.rs`) needs to color individual
/// rendered rows. A hint that crosses a line boundary (nothing in
/// `queries/highlights.scm` spans a rendered newline today) is clipped to
/// the line it starts on.
pub fn hints_by_line(lines: &[String], hints: &[StyleHint]) -> Vec<LineStyles> {
    let mut line_starts = Vec::with_capacity(lines.len());
    let mut offset = 0;
    for line in lines {
        line_starts.push(offset);
        offset += line.len() + 1; // +1 for the '\n' joining this line to the next
    }
    let mut buckets = vec![Vec::new(); lines.len()];
    for hint in hints {
        let Some(line_idx) = line_starts
            .partition_point(|&start| start <= hint.range.start)
            .checked_sub(1)
        else {
            continue;
        };
        let line_start = line_starts[line_idx];
        let line_len = lines[line_idx].len();
        let col_start = hint.range.start - line_start;
        let col_end = (hint.range.end - line_start).min(line_len);
        if col_start < col_end {
            buckets[line_idx].push((col_start..col_end, hint.role));
        }
    }
    buckets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles_at(text: &str, needle: &str) -> Vec<SyntaxRole> {
        let hints = colorize(text);
        let start = text.find(needle).expect("needle not found in text");
        let end = start + needle.len();
        hints
            .iter()
            .filter(|h| h.range == (start..end))
            .map(|h| h.role)
            .collect()
    }

    #[test]
    fn nested_message() {
        let text = "outer {\n  inner {\n  }\n}\n";
        assert_eq!(roles_at(text, "outer"), vec![SyntaxRole::Attribute]);
        assert_eq!(roles_at(text, "inner"), vec![SyntaxRole::Attribute]);
    }

    #[test]
    fn repeated_scalar_list_brackets() {
        let text = "vals: [1, 2]\n";
        assert_eq!(
            roles_at(text, "["),
            vec![SyntaxRole::PunctuationBracketList]
        );
        assert_eq!(
            roles_at(text, "]"),
            vec![SyntaxRole::PunctuationBracketList]
        );
    }

    #[test]
    fn repeated_message_field() {
        let text = "msgs { a: 1 }\nmsgs { a: 2 }\n";
        assert_eq!(roles_at(text, "msgs"), vec![SyntaxRole::Attribute]);
    }

    #[test]
    fn extension_field() {
        let text = "[pkg.Ext]: 10\n";
        assert_eq!(roles_at(text, "pkg.Ext"), vec![SyntaxRole::Type]);
        assert!(
            colorize(text)
                .iter()
                .filter(|h| h.role == SyntaxRole::PunctuationBracketExtension)
                .count()
                >= 2
        );
    }

    #[test]
    fn any_field() {
        let text = "[type.googleapis.com/pkg.Type] {\n}\n";
        assert_eq!(
            roles_at(text, "type.googleapis.com"),
            vec![SyntaxRole::StringSpecialUrl]
        );
        assert_eq!(roles_at(text, "pkg.Type"), vec![SyntaxRole::Type]);
    }

    #[test]
    fn string_with_escape() {
        let text = "label: \"a\\nb\"\n";
        assert!(colorize(text)
            .iter()
            .any(|h| h.role == SyntaxRole::StringEscape));
        assert_eq!(roles_at(text, "\\n"), vec![SyntaxRole::StringEscape]);
    }

    #[test]
    fn float_still_number() {
        // `test/highlight/textproto.txt`'s own fixture line (spec 0116
        // §7's Test-plan item 7) — the trailing `f` float suffix is a
        // known upstream tree-sitter-textproto grammar limitation (it
        // never joins the preceding `float_lit`, parsing as a sibling
        // `ERROR` node instead, confirmed against the pinned grammar
        // commit), so only the successfully parsed `1043E-04` portion is
        // asserted here.
        let text = "f: 1043E-04f\n";
        assert_eq!(roles_at(text, "1043E-04"), vec![SyntaxRole::Number]);
    }

    #[test]
    fn hex_int_still_number() {
        let text = "h: 0xfffFF00aeF\n";
        assert_eq!(roles_at(text, "0xfffFF00aeF"), vec![SyntaxRole::Number]);
    }

    #[test]
    fn comment_still_comment() {
        let text = "# hello\nfoo: 1\n";
        assert_eq!(roles_at(text, "# hello"), vec![SyntaxRole::Comment]);
    }

    #[test]
    fn bare_identifier_defaults_to_constant() {
        let text = "status: ACTIVE\n";
        assert_eq!(roles_at(text, "ACTIVE"), vec![SyntaxRole::Constant]);
    }

    #[test]
    fn true_false_are_boolean() {
        let text = "flag: true\nflag2: false\n";
        assert_eq!(roles_at(text, "true"), vec![SyntaxRole::Boolean]);
        assert_eq!(roles_at(text, "false"), vec![SyntaxRole::Boolean]);
    }

    #[test]
    fn inf_stays_number() {
        let text = "reg_scalar: -inf\n";
        assert_eq!(roles_at(text, "-inf"), vec![SyntaxRole::Number]);
    }

    #[test]
    fn bare_decimal_field_name_is_attribute() {
        // Spec 0121: protolens's own rendering convention for an
        // unresolved/unknown field (shown by number instead of name).
        let text = "1 { a: 1 }\n";
        let hints = colorize(text);
        assert!(hints
            .iter()
            .any(|h| h.role == SyntaxRole::Attribute && h.range == (0..1)));
    }

    #[test]
    fn bare_decimal_field_name_does_not_corrupt_sibling_captures() {
        // Spec 0121: before `field_no` was added to the grammar, a bare
        // decimal field name (protolens's own "unresolved field, shown by
        // number" rendering convention) had no `field_name` alternative to
        // match, forcing tree-sitter's error-recovery mode — which then
        // absorbed the next couple of syntactically-valid sibling fields
        // into the same `ERROR` node, losing their captures entirely. This
        // is the exact regression reported against a real document (an
        // `Any`-typed field's `RPC_Request` payload, whose own
        // `request_extensions` MessageSet field was still unpromoted at
        // colorize-time).
        let text = "outer {\n  1 { a: 1 }\n  flag: true\n  name: \"x\"\n}\n";
        assert_eq!(roles_at(text, "flag"), vec![SyntaxRole::Attribute]);
        assert_eq!(roles_at(text, "true"), vec![SyntaxRole::Boolean]);
        assert_eq!(roles_at(text, "name"), vec![SyntaxRole::Attribute]);
        assert_eq!(roles_at(text, "\"x\""), vec![SyntaxRole::StringLiteral]);
    }

    #[test]
    fn decimal_point_float_fully_colored() {
        // Regression test: the grammar's local `field_no` addition (spec
        // 0121) made the LALR automaton merge float_lit's post-"." state
        // with an unrelated field_no state, so the fraction digits lost
        // the lexer's tie-break to field_no's own digit-run token,
        // parsing "48.8566" as float_lit "48." + a sibling ERROR
        // "8566" — coloring only the "48." part as Number. Fixed by
        // giving `frac_digits`/`exp` explicit token precedence in
        // grammar.js.
        let text = "latitude: 48.8566\n";
        assert_eq!(roles_at(text, "48.8566"), vec![SyntaxRole::Number]);

        // Same bug also swallowed an exponent following the fraction
        // digits (e.g. "48.8566e2"), for the same underlying reason.
        let text = "latitude: 48.8566e2\n";
        assert_eq!(roles_at(text, "48.8566e2"), vec![SyntaxRole::Number]);
    }

    #[test]
    fn hints_by_line_buckets_by_row() {
        let lines = vec!["flag: true".to_string(), "n: 1".to_string()];
        let text = lines.join("\n");
        let hints = colorize(&text);
        let buckets = hints_by_line(&lines, &hints);
        assert_eq!(buckets.len(), 2);
        assert!(buckets[0]
            .iter()
            .any(|(r, role)| *role == SyntaxRole::Boolean && &text[r.clone()] == "true"));
        assert!(buckets[1]
            .iter()
            .any(|(r, role)| *role == SyntaxRole::Number && &lines[1][r.clone()] == "1"));
    }
}
