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
    /// The two `#@` annotation severity tiers (spec 0225 §S12). Which
    /// keyword is which tier is decided by `highlights.scm`'s `#any-of?`
    /// lists, mirroring `crate::annotation`'s; what color a tier is is
    /// decided by `theme::tier_color`, which a wire row reaches through
    /// `theme::tier_band`.
    AnnotationNonCanonical,
    AnnotationInvalid,
}

/// Each role paired with the `queries/highlights.scm` capture name that
/// produces it. This array's own indexing *is* the highlight index:
/// `RECOGNIZED_NAMES` is the second column of it, and
/// `from_highlight_index` is a lookup into it, so a role and its name
/// cannot come apart. Written as pairs, rather than as a name list
/// ordered to agree with `SyntaxRole`'s discriminants, because that
/// agreement was unenforceable — a variant inserted mid-enum with its
/// name appended at the end compiled cleanly and miscolored every role
/// after the insertion point.
///
/// Every capture name `highlights.scm` emits is present here *exactly*
/// (not just a dotted-prefix ancestor), so
/// `HighlightConfiguration::configure`'s longest-match resolution never
/// collapses a capture we care about into some unrelated ancestor the
/// way `tree-sitter highlight`'s CLI default theme does (see spec 0116
/// §7's investigation notes).
const ROLES: [(SyntaxRole, &str); 15] = [
    (SyntaxRole::Attribute, "attribute"),
    (SyntaxRole::Type, "type"),
    (SyntaxRole::StringLiteral, "string"),
    (SyntaxRole::StringEscape, "string.escape"),
    (SyntaxRole::StringSpecialUrl, "string.special.url"),
    (SyntaxRole::Comment, "comment"),
    (SyntaxRole::Number, "number"),
    (SyntaxRole::Boolean, "boolean"),
    (SyntaxRole::Constant, "constant"),
    (SyntaxRole::PunctuationDelimiter, "punctuation.delimiter"),
    (SyntaxRole::PunctuationBracket, "punctuation.bracket"),
    (
        SyntaxRole::PunctuationBracketList,
        "punctuation.bracket.list",
    ),
    (
        SyntaxRole::PunctuationBracketExtension,
        "punctuation.bracket.extension",
    ),
    (
        SyntaxRole::AnnotationNonCanonical,
        "annotation.non_canonical",
    ),
    (SyntaxRole::AnnotationInvalid, "annotation.invalid"),
];

/// [`ROLES`]'s name column, in order — `configure`'s `recognized_names`
/// argument, which wants the names alone.
const RECOGNIZED_NAMES: [&str; ROLES.len()] = {
    let mut names = [""; ROLES.len()];
    let mut i = 0;
    while i < ROLES.len() {
        names[i] = ROLES[i].1;
        i += 1;
    }
    names
};

/// [`ROLES`]'s role column, in order — the counterpart of
/// `RECOGNIZED_NAMES` for callers that key a table by role rather than
/// by capture name. Deriving it from `ROLES` rather than writing it out
/// is what keeps such a table the same length as the enum: a variant
/// added to `ROLES` lengthens both, and one added *only* to the enum is
/// caught by `index`.
pub const ALL_ROLES: [SyntaxRole; ROLES.len()] = {
    let mut roles = [SyntaxRole::Attribute; ROLES.len()];
    let mut i = 0;
    while i < ROLES.len() {
        roles[i] = ROLES[i].0;
        i += 1;
    }
    roles
};

impl SyntaxRole {
    fn from_highlight_index(index: usize) -> Option<Self> {
        ROLES.get(index).map(|&(role, _)| role)
    }

    /// This role's position in [`ALL_ROLES`] — the inverse of
    /// `from_highlight_index`, for indexing a per-role lookup table.
    ///
    /// A scan of sixteen tag comparisons. It replaces, at each call
    /// site, work that is orders of magnitude larger than itself: see
    /// `theme::wire_styles`.
    pub fn index(self) -> usize {
        ALL_ROLES
            .iter()
            .position(|&role| role == self)
            .expect("ROLES lists every SyntaxRole variant")
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
    // The grammar's `annotation` rule (spec 0225 S12) ends on a required
    // newline token — tree-sitter refuses an `extra` whose ending is
    // ambiguous, so the line break has to belong to the rule rather than
    // to the whitespace extra. A rendered window is joined without a
    // trailing newline and its last line may well carry an annotation,
    // so supply one here; `hints_by_line` clips every hint to its own
    // line's length, which drops the added byte again.
    let owned;
    let text = if text.ends_with('\n') {
        text
    } else {
        owned = format!("{text}\n");
        &owned
    };

    let mut highlighter = Highlighter::new();
    // Every hint this function returns is a color, and the text
    // underneath is already correct without one. So a highlighter that
    // refuses this input costs a monochrome viewport, while panicking —
    // on the per-frame render path, with the terminal in raw mode —
    // costs the whole session.
    let Ok(events) = highlighter.highlight(config(), text.as_bytes(), None, |_| None) else {
        return Vec::new();
    };

    let mut hints = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for event in events {
        // A mid-stream failure leaves the hints already collected
        // untouched, and each is still correctly placed against its own
        // span, so they are worth keeping: the document colors as far as
        // the highlighter got and plainly after that.
        let Ok(event) = event else { break };
        match event {
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
        // A field name, not a type: `[pkg.Ext]: 10` names a field, and
        // on the wire it is a tag like any other field's.
        assert_eq!(roles_at(text, "pkg.Ext"), vec![SyntaxRole::Attribute]);
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
    fn float_suffix_joins_the_literal() {
        // `test/highlight/textproto.txt`'s own fixture line (spec 0116
        // §7's Test-plan item 7). The trailing `f`/`F` float suffix used
        // to be an upstream grammar limitation — a plain `/[Ff]/` ties
        // with `identifier` on length in the same state, so it never
        // attached and parsed as a sibling `ERROR` — until spec 0196 S5
        // made it `token.immediate`.
        let text = "f: 1043E-04f\n";
        assert_eq!(roles_at(text, "1043E-04f"), vec![SyntaxRole::Number]);
        let text = "f: 1.5f\n";
        assert_eq!(roles_at(text, "1.5f"), vec![SyntaxRole::Number]);

        // `immediate` is what separates the suffix from the next field's
        // name: with whitespace between, `f` is a field again.
        let text = "x: 1.5\nf: 2\n";
        assert_eq!(roles_at(text, "1.5"), vec![SyntaxRole::Number]);
        assert_eq!(roles_at(text, "f"), vec![SyntaxRole::Attribute]);
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

    /// Every role assigned to any byte of `needle`, deduplicated in
    /// first-seen order — for the escape tests, where the defect being
    /// asserted against is precisely that a span gets *split*, so
    /// `roles_at`'s exact-range match would report an empty vector for
    /// both the fixed and the broken grammar.
    fn roles_across(text: &str, needle: &str) -> Vec<SyntaxRole> {
        let start = text.find(needle).expect("needle not found in text");
        let end = start + needle.len();
        let mut seen: Vec<SyntaxRole> = Vec::new();
        for hint in colorize(text) {
            if hint.range.start < end && start < hint.range.end && !seen.contains(&hint.role) {
                seen.push(hint.role);
            }
        }
        seen
    }

    #[test]
    fn a_backslash_escape_does_not_swallow_the_closing_quote() {
        // Spec 0196 S1. Upstream's escape list spelled `"\\\""` and
        // `'\\"'`, which are the same two characters in JavaScript — so
        // it held `\"` twice and `\\` never. A `\\` lexed as a lone `\`
        // followed by `\"`, which ate the string's own closing quote:
        // the string ran to end of line and the trailing annotation
        // comment colored as string.
        let text = "s: \"\\027\\002\\\\\"  #@ string\n";
        assert_eq!(roles_across(text, "\\\\"), vec![SyntaxRole::StringEscape]);
        // `roles_across`, not `roles_at`: since spec 0225 S12 an
        // annotation is a rule of its own, so `#@ string` is a marker
        // and a word rather than one comment-shaped span. `string` is
        // not in the tier vocabulary and not a declaration, so the
        // blanket `(annotation) @comment` is all that reaches it — and
        // the point being asserted, that the escape did not run the
        // string on past its quote, is that no byte here is a string.
        assert_eq!(roles_across(text, "#@ string"), vec![SyntaxRole::Comment]);
    }

    #[test]
    fn an_escape_is_one_span_including_its_optional_digits() {
        // Spec 0196 S2. The octal and hex escapes were sequences of
        // separate tokens, so after `\0` the lexer could take either a
        // one-character `oct` or the greedy `double_string_contents`;
        // longest-match won and `\004` colored as an escape `\0` plus
        // ordinary contents `04`.
        let text = "location: \"\\n\\004\\004\\000hello\"  #@ bytes\n";
        for escape in ["\\n", "\\004", "\\000"] {
            assert_eq!(
                roles_across(text, escape),
                vec![SyntaxRole::StringEscape],
                "{escape} is not a single escape span",
            );
        }
        assert_eq!(roles_across(text, "hello"), vec![SyntaxRole::StringLiteral]);
        // The marker stays comment; `bytes` is a wire-type name and
        // takes the type color.
        assert_eq!(
            roles_across(text, "#@ bytes"),
            vec![SyntaxRole::Comment, SyntaxRole::Type],
        );

        // Same defect, hex form, and an octal escape whose value is out
        // of range for a byte — still one token (spec 0196 N2: the
        // grammar delimits, it does not range-check).
        for text in ["s: \"\\x41\"\n", "s: \"\\400\"\n"] {
            let escape = &text[4..text.len() - 3];
            assert_eq!(
                roles_across(text, escape),
                vec![SyntaxRole::StringEscape],
                "{escape} is not a single escape span",
            );
        }
    }

    #[test]
    fn every_protobuf_escape_parses() {
        // Spec 0196 S2. `\U0010FFFF` did not parse at all before:
        // upstream's second `\U` arm was `\U010` plus four hex digits —
        // a seven-digit `\U` — so the whole top plane was unreachable.
        let text = "s: \"\\a\\b\\f\\n\\r\\t\\v\\?\\'\\\"\\\\\"\n";
        assert_eq!(
            colorize(text)
                .iter()
                .filter(|h| h.role == SyntaxRole::StringEscape)
                .count(),
            11,
        );
        for text in [
            "s: \"\\u00e9\"\n",
            "s: \"\\U0001F600\"\n",
            "s: \"\\U0010FFFF\"\n",
        ] {
            let escape = &text[4..text.len() - 3];
            assert_eq!(
                roles_across(text, escape),
                vec![SyntaxRole::StringEscape],
                "{escape} does not parse as an escape",
            );
        }
    }

    #[test]
    fn a_hash_inside_a_string_is_an_ordinary_character() {
        // Spec 0201 S1. `comment` is an `extra`, and tree-sitter offers
        // extras in every lex state — including the ones between a
        // string's own tokens. At a `#` the contents token could only
        // reach the next backslash while `comment` reached end of line,
        // so longest match gave the rest of the payload to `comment` and
        // the string ran on past its closing quote into the next line.
        let text = "4: \"\\022#\\n\\rexport_x\"  #@ bytes = 4\nnext_field: 1\n";
        // `find` reaches the `#` inside the string before the real one.
        assert_eq!(roles_across(text, "#"), vec![SyntaxRole::StringLiteral]);
        assert_eq!(
            roles_across(text, "export_x"),
            vec![SyntaxRole::StringLiteral]
        );
        // `bytes = 4` is a field declaration, so spec 0225 S12's query
        // reads the word before the `=` as a type and the number after
        // it as the field number. What this test is about is the bytes
        // that are *not* here: none of the annotation is string-colored.
        assert_eq!(
            roles_across(text, "#@ bytes = 4"),
            vec![SyntaxRole::Comment, SyntaxRole::Type, SyntaxRole::Attribute],
        );
        // The string closed, so the following line is still a field.
        assert_eq!(roles_at(text, "next_field"), vec![SyntaxRole::Attribute]);

        // A `#` in an ordinary run, and one in a single-quoted string.
        for (text, needle) in [
            ("s: \"trail#\"  # real\n", "trail#"),
            ("s: 'has # hash'  # real\n", "has # hash"),
        ] {
            assert_eq!(roles_across(text, needle), vec![SyntaxRole::StringLiteral]);
            assert_eq!(roles_at(text, "# real"), vec![SyntaxRole::Comment]);
        }
    }

    #[test]
    fn a_space_after_an_escape_stays_inside_the_string() {
        // Spec 0201 G2, the same lex state boundary: the space starting
        // a contents run was skipped as a whitespace `extra` rather than
        // taken into the string.
        let text = "b: \"p\\n q\"\n";
        assert_eq!(roles_across(text, " q"), vec![SyntaxRole::StringLiteral]);
    }

    #[test]
    fn adjacent_strings_still_concatenate() {
        // Spec 0201 test 3: raising the string body's token precedence
        // must not disturb the whitespace *between* two strings, which
        // `scalar_value`'s `repeat1($.string)` needs.
        let text = "a: \"x\" \"y\"\n";
        assert_eq!(roles_at(text, "\"x\""), vec![SyntaxRole::StringLiteral]);
        assert_eq!(roles_at(text, "\"y\""), vec![SyntaxRole::StringLiteral]);
    }

    #[test]
    fn an_identifier_after_a_number_keeps_its_first_letter() {
        // Spec 0196 S3. `exp`'s leading `token(prec(2, /[Ee][-+]?/))`
        // outranked `identifier` — tree-sitter compares explicit token
        // precedence before match length — so a one-character `e` beat
        // the whole word wherever `exp` was valid, which after spec
        // 0121's `field_no` state merge is right after a top-level
        // scalar number. `end: 6` became a stray `e` plus a field named
        // `nd`.
        for value in ["1", "1.5", "48.8566", "0755", "0xff", "1e5"] {
            for name in ["end", "Email"] {
                let text = format!("x: {value}\n{name}: 6\n");
                assert_eq!(
                    roles_at(&text, name),
                    vec![SyntaxRole::Attribute],
                    "{name} after {value} lost its first letter",
                );
            }
        }

        // The worst case: a single-letter `e` field name after a float
        // used to make the parser join both lines into one `number`.
        let text = "x: 1.5\ne: 6\n";
        assert_eq!(roles_at(text, "e"), vec![SyntaxRole::Attribute]);
        assert_eq!(roles_at(text, "6"), vec![SyntaxRole::Number]);
    }

    #[test]
    fn an_empty_or_comment_only_document_parses() {
        // Spec 0196 S4. `message` was defined twice in one object
        // literal, so the later `repeat1($.field)` silently won — and
        // `message` being the start rule too, a document with no field
        // in it parsed as ERROR.
        assert!(colorize("").is_empty());
        assert!(colorize("\n").is_empty());
        assert_eq!(
            roles_at("# just a comment\n", "# just a comment"),
            vec![SyntaxRole::Comment],
        );
    }

    /// The two roles the tests above never reached: the `:`/`,`/`;`
    /// delimiters, and the plain (non-list, non-extension) brackets that
    /// open and close a message body.
    ///
    /// They matter to the tests around them as much as to the screen.
    /// `ROLES`'s *position* is the highlight index `configure` hands
    /// back, so what the grammar captures and what this crate believes
    /// it captured are two lists that only a test naming the text and
    /// the role it should produce can compare — and until every role has
    /// one, a disagreement can hide in whichever role does not.
    #[test]
    fn delimiters_and_message_brackets_are_punctuation() {
        let text = "outer {\n  vals: [1, 2];\n  inner <\n  >\n}\n";
        for delimiter in [":", ",", ";"] {
            assert_eq!(
                roles_at(text, delimiter),
                vec![SyntaxRole::PunctuationDelimiter],
                "{delimiter} is not a delimiter",
            );
        }
        for bracket in ["{", "}", "<", ">"] {
            assert_eq!(
                roles_at(text, bracket),
                vec![SyntaxRole::PunctuationBracket],
                "{bracket} is not a plain bracket",
            );
        }
        // The square brackets of the same document are the *list* kind,
        // not this one — the split that spec 0116 §6 exists for.
        assert_eq!(
            roles_at(text, "["),
            vec![SyntaxRole::PunctuationBracketList]
        );
    }

    /// `RECOGNIZED_NAMES` and the grammar's `highlights.scm` are one
    /// list kept in two places, in two directories, edited by different
    /// changes — so they are checked against each other here.
    ///
    /// Drift is silent in both directions. A capture the query emits and
    /// this list omits is resolved by `configure` to a dotted-prefix
    /// ancestor, or to nothing, and the text quietly loses its color; a
    /// name left here after the query stopped emitting it is a role no
    /// document can ever produce, and every test of it would have to be
    /// deleted before anyone noticed.
    #[test]
    fn the_recognized_names_are_exactly_the_grammars_captures() {
        let emitted: std::collections::BTreeSet<&str> = config().names().iter().copied().collect();
        let recognized: std::collections::BTreeSet<&str> =
            RECOGNIZED_NAMES.iter().copied().collect();
        assert_eq!(
            emitted, recognized,
            "highlights.scm and RECOGNIZED_NAMES disagree",
        );
        assert_eq!(
            recognized.len(),
            RECOGNIZED_NAMES.len(),
            "RECOGNIZED_NAMES lists a name twice, which would give two \
             roles one index",
        );
    }

    /// No two entries of `ROLES` name the same role.
    ///
    /// Pairing each role with its capture name makes an *index* mismatch
    /// unrepresentable, but not a duplicated role: write one twice and
    /// the second capture is styled as the first, so two distinct pieces
    /// of syntax become permanently indistinguishable with nothing said
    /// anywhere.
    #[test]
    fn no_two_recognized_names_share_a_role() {
        let mut seen = Vec::new();
        for (index, name) in RECOGNIZED_NAMES.iter().enumerate() {
            let role = SyntaxRole::from_highlight_index(index)
                .unwrap_or_else(|| panic!("{name} has no role"));
            assert!(
                !seen.contains(&role),
                "{role:?} is claimed by two names, so one of them can never \
                 be told apart from the other",
            );
            seen.push(role);
        }
    }

    /// The role a keyword of `tier` must end up wearing.
    ///
    /// The untiered names are exactly the wire types, and they take
    /// `Type`: an unknown or invalid field has nothing but its wire type
    /// to say what it is, so the name fills the slot a declared type
    /// would, and the wire row's wire type borrows it instead of going
    /// gray exactly where the bytes matter most.
    fn role_for(tier: Option<crate::annotation::Tier>) -> SyntaxRole {
        use crate::annotation::Tier;
        match tier {
            None => SyntaxRole::Type,
            Some(Tier::NonCanonical) => SyntaxRole::AnnotationNonCanonical,
            Some(Tier::Invalid) => SyntaxRole::AnnotationInvalid,
        }
    }

    /// Spec 0225 S12, constraint 1. Three `#` contexts that must stay
    /// apart: `#@` opens an annotation, a bare `#` still opens a
    /// comment, and neither happens inside a string (spec 0201).
    #[test]
    fn a_hash_annotation_is_not_a_plain_comment() {
        assert_eq!(
            roles_across("x: 1  #@ TAG_OOR\n", "TAG_OOR"),
            vec![SyntaxRole::AnnotationInvalid],
        );
        assert_eq!(
            roles_across("x: 1  # TAG_OOR\n", "TAG_OOR"),
            vec![SyntaxRole::Comment],
            "a bare # is still an undifferentiated comment",
        );
        assert_eq!(
            roles_across("s: \"#@ TAG_OOR\"\n", "TAG_OOR"),
            vec![SyntaxRole::StringLiteral],
        );
    }

    /// Spec 0225 S12, constraint 2. The grammar's annotation rule is
    /// total, so a body that matches nothing in the format still parses
    /// — which matters because error recovery in this grammar swallows
    /// *following* siblings, and the failure would show up as the next
    /// line losing its colors rather than as this one gaining them.
    #[test]
    fn a_malformed_annotation_does_not_disturb_the_next_line() {
        let text = "x: 1  #@ !!! ??? ~~~ ;;; ==\nnext_field: 2\n";
        assert_eq!(
            roles_across(text, "next_field"),
            vec![SyntaxRole::Attribute]
        );
        assert_eq!(roles_across(text, "2\n"), vec![SyntaxRole::Number]);
        // A bare marker with no body at all is the degenerate case of
        // the same rule.
        let text = "x: 1  #@\nnext_field: 2\n";
        assert_eq!(
            roles_across(text, "next_field"),
            vec![SyntaxRole::Attribute]
        );
    }

    /// Spec 0225 S12. Two modifiers of different severity on one line
    /// keep their own hues — the tier follows the keyword, not the line.
    #[test]
    fn a_modifier_takes_the_hue_of_its_keyword() {
        let text = "x: 1  #@ varint; len_ohb: 2; TAG_OOR\n";
        assert_eq!(
            roles_across(text, "len_ohb"),
            vec![SyntaxRole::AnnotationNonCanonical],
        );
        assert_eq!(
            roles_across(text, "TAG_OOR"),
            vec![SyntaxRole::AnnotationInvalid],
        );
        assert_eq!(
            roles_across(text, "varint"),
            vec![SyntaxRole::Type],
            "a wire-type name is the document's own vocabulary quoted, \
             not an anomaly — it is what the field is",
        );
    }

    /// Spec 0225 S12, constraint 3's accepted cost. A keyword
    /// prototext-core adds later is visibly uncolored until someone
    /// lists it, rather than being classified by a rule that was never
    /// told about it.
    #[test]
    fn an_unlisted_modifier_stays_a_comment() {
        assert_eq!(
            roles_across("x: 1  #@ varint; SOMETHING_NEW\n", "SOMETHING_NEW"),
            vec![SyntaxRole::Comment],
        );
        assert_eq!(crate::annotation::tier_of("SOMETHING_NEW"), None);
    }

    /// Spec 0225 S12. The declaration before the first `;` is the
    /// document's own vocabulary quoted back, so it wears the
    /// document's own captures rather than a tier.
    #[test]
    fn the_declaration_echo_matches_the_documents_own_styles() {
        let text = "x: 1  #@ repeated int32 [packed=true] = 85; pack_size: 3\n";
        assert_eq!(roles_across(text, "int32"), vec![SyntaxRole::Type]);
        assert_eq!(
            roles_across(text, "repeated"),
            vec![SyntaxRole::Type],
            "the label and the type are two halves of one statement \
             about what the field is",
        );
        // The field number is the document's own, and wears the color
        // its name wears at the head of the line — not the value color.
        assert_eq!(roles_across(text, "85"), vec![SyntaxRole::Attribute]);
        // Spec 0267: `[packed=true]` is the declaration's third word,
        // between the type and the `=`, so it is one statement in one
        // color with the two beside it.
        assert_eq!(roles_across(text, "[packed=true]"), vec![SyntaxRole::Type]);
        // And `pack_size` is a modifier like any other unclassified
        // one: it counts a record's elements, which accuses nothing.
        assert_eq!(roles_across(text, "pack_size"), vec![SyntaxRole::Comment]);

        // An extension name is a field name: `[acme.blade]: 3` names a
        // field, and on the wire it is a tag like any other field's.
        assert_eq!(
            roles_across("[com.example.ext]: 1\n", "com.example.ext"),
            vec![SyntaxRole::Attribute],
        );
        assert_eq!(roles_across("n: 85\n", "85"), vec![SyntaxRole::Number]);

        // A fully-qualified name is one word, not a run split on dots.
        assert_eq!(
            roles_across(
                "x: 1  #@ google.protobuf.FileDescriptorProto = 2\n",
                "google.protobuf.FileDescriptorProto",
            ),
            vec![SyntaxRole::Type],
        );
    }

    /// Spec 0225 S12. An enum field's declaration is two facts, not
    /// one: `Type(5)` is a type name and the number it resolved to, and
    /// the number wears the value color the symbolic name wears on the
    /// document half of the same line.
    #[test]
    fn an_enum_declaration_splits_its_name_from_its_value() {
        let text = "type: TYPE_INT32  #@ Type(5) = 5\n";
        assert_eq!(roles_across(text, "Type"), vec![SyntaxRole::Type]);
        assert_eq!(roles_across(text, "(5)"), vec![SyntaxRole::Number]);
        // The symbolic name in the document and the number it stands
        // for in the annotation: one value, one color.
        assert_eq!(roles_across(text, "TYPE_INT32"), vec![SyntaxRole::Constant]);
        // A packed enum renders its whole run in the parentheses, and
        // it is still one number-colored token.
        assert_eq!(
            roles_across("x: 1  #@ Color([1, 2]) = 7\n", "([1, 2])"),
            vec![SyntaxRole::Number],
        );
    }

    /// Spec 0225 S12. `ENUM_UNKNOWN` is ALL CAPS and non-canonical —
    /// the counterexample in the direction `pack_size` does not cover,
    /// and half the reason the vocabulary is keyed on keywords rather
    /// than on capitalization.
    #[test]
    fn enum_unknown_is_yellow_not_red() {
        assert_eq!(
            roles_across("x: 1  #@ varint; ENUM_UNKNOWN\n", "ENUM_UNKNOWN"),
            vec![SyntaxRole::AnnotationNonCanonical],
        );
    }

    /// Spec 0225 S12's drift test. `highlights.scm`'s `#any-of?` lists
    /// are the one copy of `annotation::tier_of`'s table that lives
    /// outside Rust, because a query file cannot call it. Every keyword
    /// is colorized here so the copy cannot fall behind quietly.
    #[test]
    fn every_keyword_is_colored_by_its_tier() {
        for (keyword, tier) in crate::annotation::vocabulary() {
            let text = format!("x: 1  #@ varint; {keyword}\n");
            assert_eq!(
                roles_across(&text, keyword),
                vec![role_for(tier)],
                "{keyword} ({tier:?}) is not colored by its tier — \
                 highlights.scm and annotation.rs have drifted",
            );
        }
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
