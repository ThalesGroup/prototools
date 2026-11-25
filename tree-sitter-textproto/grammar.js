// Vendored, locally-modified copy of upstream tree-sitter-textproto's
// grammar.js (pinned commit 568471b80fd8793d37ed01865d8c2208a9fefd1b:
// https://github.com/PorterAtGoogle/tree-sitter-textproto). See
// docs/specs/0121-tree-sitter-textproto-field-no-vendoring.md for why this
// is now vendored+modified rather than fetched-and-generated verbatim: a
// new `field_no` rule (cloned from `dec_int`'s own definition, not reused
// directly — a shared rule risks an LR table conflict between `field_no`'s
// structural position, inside `field_name`, and `dec_int`'s own, inside
// `number`) is added as a `field_name` alternative, so a bare decimal
// field number (protolens's own rendering convention for an
// unknown/unresolved field) parses as a valid `field_name` instead of
// triggering tree-sitter's error-recovery mode.
//
// docs/specs/0196-the-textproto-grammars-lexer-loses-tokens.md then
// fixed seven further defects, six of them upstream's — see the "Local
// fix (spec 0196 ...)" comments at `document`, `string_escape`, `exp`,
// `float`, and above `extras`. Tracking upstream would reintroduce
// them, which is a second reason this copy is vendored rather than
// fetched.
//
// docs/specs/0201-a-hash-inside-a-string-is-not-a-comment.md then fixed
// an eighth, also upstream's: a `#` inside a quoted string started a
// `comment`, because comments are `extras` and extras are offered in
// every lex state — including the states between a string's own tokens.
// See the "Local fix (spec 0201 S1)" comment at
// `single_string_contents`.
//
// docs/specs/0225-the-wire-bytes-are-shown-under-each-line.md then added
// `annotation`, a second composite extra beside `comment`, so that
// protolens's own `#@` annotations can be colored by severity instead of
// being one undifferentiated comment span. See the "Local addition (spec
// 0225 S12)" comment above `annotation`.

// https://protobuf.dev/reference/protobuf/textformat-spec/

module.exports = grammar({
  name: 'textproto',

  rules: {
    // Local fix (spec 0196 S4): upstream defines `message` twice — once
    // here as `repeat($.field)` and again below as `repeat1($.field)`.
    // It is one JavaScript object literal, so the second silently won
    // and this one was dead code; `message` being the start rule as
    // well, an empty (or comment-only) document parsed as ERROR. The
    // two uses want different rules and now get them: a document may be
    // empty, a message *body* may not — `message` has to stay
    // `repeat1`, because `message_value` already wraps it in
    // `optional()` and a nullable rule under `optional` is ambiguous.
    document: $ => repeat($.field),

    field: $ => choice($.message_field, $.scalar_field),

    message_field: $ => seq(
      $.field_name,
      optional(":"),
      choice($.message_value, $.message_list),
      optional(choice(";", ",")),
    ),

    scalar_field: $ => seq(
      $.field_name,
      ":",
      choice(
    $.scalar_value,
    $.scalar_list,
      ),
      optional(choice(";", ",")),
    ),

    message: $ => repeat1($.field),

    message_value: $ => choice(
      seq(
    $.open_squiggly,
    optional($.message),
    $.close_squiggly,
      ),
      seq(
    $.open_arrow,
    optional($.message),
    $.close_arrow,
      )
    ),

    message_list: $ => prec(2, seq(
      $.open_square,
      optional(
    seq(
      $.message_value,
      repeat(
        seq(
          ",",
          $.message_value,
        ),
      ),
    ),
      ),
      $.close_square,
    )),

    open_squiggly: $ => '{',
    close_squiggly: $ => '}',
    open_square: $ => '[',
    close_square: $ => ']',
    open_arrow: $ => '<',
    close_arrow: $ => '>',

    // Local modification (see file header): `$.field_no` is a new
    // alternative, not present upstream — protolens renders a field it
    // cannot resolve to a schema name as its bare decimal field number
    // (e.g. `1 { ... }`), which upstream's grammar rejects (only
    // `identifier`/`extension_name`/`any_name` are valid `field_name`s),
    // triggering tree-sitter's error-recovery mode and corrupting
    // highlight captures on neighboring siblings.
    field_name: $ => choice(
      $.extension_name,
      $.any_name,
      $.identifier,
      $.field_no,
    ),

    extension_name: $ => seq(
      $.open_square,
      $.type_name,
      $.close_square,
    ),

    any_name: $ => seq(
      $.open_square,
      $.domain,
      "/",
      $.type_name,
      $.close_square,
    ),

    type_name: $ => seq(
      $.identifier,
      repeat(choice(".", $.identifier)),
    ),
    domain: $ => seq(
      $.identifier,
      repeat(choice(".", $.identifier)),
    ),

    identifier: $ => /[A-Za-z_][A-Za-z0-9_]*/,
    signed_identifier: $ => seq(
      "-",
      $.identifier,
    ),

    // Local addition (see file header): cloned from `dec_int`'s own
    // definition below, deliberately not shared with it — `field_name`
    // and `number` occupy different structural positions in the grammar,
    // and reusing the same rule in both risked an LR table conflict.
    field_no: $ => choice(
      "0",
      /[1-9][0-9]*/,
    ),

    scalar_value: $ => choice(
      repeat1($.string),
      $.identifier,
      $.signed_identifier,
      $.number,
    ),

    scalar_list: $ => prec(1, seq(
      $.open_square,
      optional(
    seq(
      $.scalar_value,
      repeat(seq(",", $.scalar_value)),
    ),
      ),
      $.close_square,
    )),

    string: $ => choice(
      $.single_string,
      $.double_string,
    ),

    single_string: $ => seq(
      "'",
      repeat(choice(
    $.string_escape,
    $.single_string_contents,
      )),
      "'",
    ),

    double_string: $ => seq(
      '"',
      repeat(choice(
    $.string_escape,
    $.double_string_contents,
      )),
      '"',
    ),

    // Local fix (spec 0201 S1): the `prec(1, …)` is not upstream's. A
    // string is not one token — `double_string` is `seq('"', repeat(…),
    // '"')` — so there is a lex state between its inner tokens, and
    // tree-sitter offers every `extra` as a candidate in every lex
    // state. At a `#` inside a string, the contents token could only
    // reach the next backslash while `comment` reached end of line;
    // longest match handed the rest of the payload to `comment`, and the
    // string then ran on past its own closing quote. Explicit token
    // precedence is compared *before* match length, so precedence 1
    // settles that contest — and only that one, since these tokens are
    // valid in no state outside a string body. It also keeps a leading
    // space from being skipped as a whitespace extra: `"p\n q"`'s space
    // now belongs to the string.
    //
    // Raised from 1 to 2 by spec 0225 S12: `annotation_marker` is a
    // second extra that can open inside a string body, and at precedence
    // 1 it would have *tied* with these — `#@` right after an escape
    // matches two characters and so does the contents run that follows
    // it, and a tie is settled by neither rule. Staying strictly above
    // every extra is what spec 0201's fix actually needs.
    single_string_contents: $ => token(prec(2, /[^\n'\\]+/)),
    double_string_contents: $ => token(prec(2, /[^\n"\\]+/)),

    // Local fix (spec 0196 S1/S2, and the `prec(1, …)` of spec 0201 S1 —
    // see `double_string_contents` above). Three defects in upstream's own
    // version of this rule:
    //
    //   1. Its character-escape list spelled `"\\\""` and `'\\"'`, which
    //      are the *same* two characters in JavaScript — so it contained
    //      `\"` twice and `\\` not at all. A `\\` in a payload lexed as a
    //      lone `\` followed by `\"`, eating the string's own closing
    //      quote and swallowing the rest of the line.
    //   2. It was a *sequence* of nodes, so after `\0` the lexer could
    //      take either `oct` (one character) or `double_string_contents`
    //      (`/[^\n"\\]+/`, greedy). Longest-match won and the optional
    //      trailing octal/hex digits were never taken: `\004` parsed as
    //      an escape `\0` plus ordinary contents `04`. `\uXXXX` escaped
    //      this because all four of its digits are required.
    //   3. `\U010` + 4 hex is a seven-digit `\U`; protobuf's takes
    //      eight, and the intent is plainly `\U0010` + 4 (the sibling
    //      `\U000` + 5 covers U+00000-U+FFFFF, this one covers
    //      U+100000-U+10FFFF). As written, `\U0010FFFF` did not parse.
    //
    // Wrapping the whole rule in `token()` fixes 2 at the source — the
    // lexer commits to an entire escape or to none of it — and is why
    // upstream's `oct`/`hex` rules are gone: `token()` requires terminal
    // contents, so they are inlined as the character classes they were.
    // Nothing outside this rule referenced them (`highlights.scm`
    // captures `(string_escape)` whole).
    string_escape: $ => token(prec(1, choice(
      seq("\\", /[abfnrtv?'"\\]/),
      seq("\\", /[0-7]/, optional(/[0-7]/), optional(/[0-7]/)),
      seq("\\x", /[0-9A-Fa-f]/, optional(/[0-9A-Fa-f]/)),
      seq("\\u", /[0-9A-Fa-f]{4}/),
      seq("\\U000", /[0-9A-Fa-f]{5}/),
      seq("\\U0010", /[0-9A-Fa-f]{4}/),
    ))),

    number: $ => choice(
      $.dec_int,
      $.oct_int,
      $.hex_int,
      seq(optional('-'), $.float),
      seq("-", $.dec_int),    // signed decimal int
      seq('-', $.oct_int),    // signed octal int
      seq('-', $.hex_int),    // signed hexidecimal int
    ),

    dec_int: $ => choice(
      "0",
      /[1-9][0-9]*/,
    ),
    oct_int: $ => /0[0-7]+/,
    hex_int: $ => /0[Xx][0-9A-Fa-f]+/,
    float_lit: $ => choice(
      seq(
    $.dec_int,
    $.exp
      ),
      seq(
    ".",
    $.frac_digits,
    optional($.exp)
      ),
      seq(
    $.dec_int,
    ".",
    optional($.frac_digits),
    optional($.exp)
      ),
    ),
    // Local fix: the `field_no` addition above (spec 0121) makes the
    // LALR automaton merge float_lit's post-"." state with an unrelated
    // field_no state, so plain (unprioritized) digit-run/exponent-lead
    // tokens here lose the lexer's ambiguity tie-break to field_no's
    // `/[1-9][0-9]*/` (or, for `exp`, to a plain `identifier`) — e.g.
    // "48.8566" parses as float_lit "48." + ERROR "8566", silently
    // dropping the fraction digits' `number` highlight past the
    // decimal point. `token(prec(N, ...))` gives these two tokens
    // lexer-level priority so they win that tie-break; named (not
    // anonymous inline /\d+/) so the fraction digits get their own
    // node distinct from `dec_int`.
    frac_digits: $ => token(prec(2, /\d+/)),
    // Local fix (spec 0196 S3): `exp` used to be `seq(token(prec(2,
    // /[Ee][-+]?/)), /\d+/)`. tree-sitter's lexer compares explicit
    // token precedence *before* match length, so that leading one-
    // character token beat `identifier` wherever `exp` was valid — which,
    // after the `field_no` state merge above, is immediately after a
    // top-level scalar number, i.e. exactly where field names live.
    // `end: 6` after a scalar field lexed as a stray `e` plus a field
    // named `nd`. Requiring the digits in the *same* token keeps spec
    // 0121's precedence (still needed for `48.8566e2`) while making the
    // token unable to match a bare identifier's first letter.
    exp: $ => token(prec(2, /[Ee][-+]?[0-9]+/)),
    // Local fix (spec 0196 S5, revised by spec 0225 S12): the suffix
    // used to be a plain `/[Ff]/`, which ties with `identifier` on
    // length in the same state, so it never attached — `1.5f` parsed as
    // `float_lit` plus a sibling ERROR. 0196 fixed that with
    // `token.immediate(/[Ff]/)`, on the reasoning that the absence of
    // whitespace is what separates a float suffix from the next field's
    // name.
    //
    // That reasoning stopped holding once `annotation` (below) became an
    // extra whose terminator *consumes* the newline. `token.immediate`
    // means "no extras were skipped before this token", not "no
    // whitespace precedes it", and after an extra has eaten the line
    // break the next line's first byte is adjacent to that extra's end.
    // So `x: 5  #@ varint` on one line and `f: 6` on the next lexed the
    // `f` as this suffix and left a MISSING identifier behind it.
    //
    // Folding the suffix into a single token removes the immediacy
    // instead of trying to condition it: a suffixed float is now lexed
    // whole, and no `token.immediate` remains in the grammar outside the
    // annotation rules. Precedence 3 keeps it above `frac_digits`/`exp`
    // (2) and above `dec_int`, so the longer suffixed match is taken
    // wherever a number is valid; where no `[Ff]` follows, the token
    // simply does not match and the unsuffixed arms below apply.
    float: $ => choice(
      $.float_lit,
      $.float_suffixed,
    ),
    float_suffixed: $ => token(prec(3, seq(
      choice(
        /[0-9]+/,
        seq(/[0-9]+/, '.', optional(/[0-9]+/)),
        seq('.', /[0-9]+/),
      ),
      optional(/[Ee][-+]?[0-9]+/),
      /[Ff]/,
    ))),

    comment: $ => seq('#', /.*/),

    // Local addition (spec 0225 S12): protolens renders its own `#@`
    // annotations into the text it colors, and they carry the severity
    // of what the decoder found. As one `comment` span they were
    // uncolorable, so they get a rule of their own here and
    // `highlights.scm` maps its tokens onto tiers.
    //
    // Four properties this rule is built around:
    //
    //   1. `#@` must beat `comment`'s `'#'`. tree-sitter compares
    //      explicit token precedence before match length (spec 0196 S3,
    //      spec 0201 S1), so precedence 1 against `'#'`'s implicit 0
    //      settles it — the longer match is not what decides.
    //   2. It must be total. Every byte after the marker falls into one
    //      of the five item tokens, the last of which
    //      (`annotation_junk`) is the catch-all, so no annotation can
    //      fail to parse. A rule that could fail would drop the parser
    //      into error recovery, which in this grammar swallows
    //      *following* siblings and would lose the highlighting of the
    //      lines after it.
    //   3. It ends at the newline, and says so. tree-sitter requires an
    //      `extra` to have an unambiguous ending — a rule whose last
    //      element is a `repeat` is rejected outright — so the newline
    //      is a token of the rule rather than the whitespace extra it
    //      would otherwise be. `annotation_end` is the only place a line
    //      break can be consumed inside an annotation, which is what
    //      stops one from swallowing the field on the next line.
    //   4. Its item tokens are ordinary tokens, matching no leading
    //      whitespace of their own. The obvious alternative — make each
    //      one `token.immediate(/[ \t]*…/)` so it absorbs its own
    //      spacing — leaks out of the annotation: tree-sitter merges the
    //      lexer's DFA states, and a token that may begin with a run of
    //      spaces makes the *whole* lex state stop treating a space as
    //      skippable. `outer {` then lexed `{` as a two-character
    //      `open_squiggly` starting at the space. Leaving the spacing to
    //      the `/\s/` extra keeps every other token's span where it was.
    //
    // `colorize` guarantees the trailing newline this needs, since a
    // rendered window is joined without one and its last line may well
    // carry an annotation.
    //
    // The rule stays keyword-blind on purpose: it says which tokens an
    // annotation is made of, never which words matter. `highlights.scm`
    // holds the vocabulary, mirroring `protolens/src/annotation.rs`, and
    // is the cheap file to edit — a query change needs no
    // `tree-sitter generate`.
    annotation: $ => seq(
      $.annotation_marker,
      optional($.annotation_item),
      repeat(seq($.annotation_semi, optional($.annotation_item))),
      $.annotation_end,
    ),

    // One `;`-separated item: a flat run of typed tokens. Flat rather
    // than `key [":" value]` because an item is not always a pair — the
    // leading declaration item is `[label] type [attribute] "=" number`
    // — and a shape that has to choose between the two needs lookahead
    // the LR table cannot give it while staying total.
    annotation_item: $ => repeat1(choice(
      $.annotation_word,
      $.annotation_number,
      $.annotation_eq,
      $.annotation_attribute,
      $.annotation_enum_value,
      $.annotation_junk,
    )),

    annotation_marker: $ => token(prec(1, '#@')),

    // Dots are part of a word so a fully-qualified type name is one
    // token.
    annotation_word: $ => token(prec(2, /[A-Za-z_][A-Za-z0-9_.]*/)),
    // An enum field's rendered wire value, `Color(99)` or the packed
    // `Color([1, 2])`. Its own token rather than a suffix of
    // `annotation_word`, because the name is a type and the value in
    // the parentheses is a number, and `highlights.scm` colors those
    // two differently. Splitting costs nothing structurally: the pair
    // is still adjacent, and nothing but a type name can precede it.
    //
    // Above `annotation_junk` (whose class contains `(` and `)`) so the
    // whole group is taken rather than its first character. An
    // unterminated `(` matches neither and falls to junk, which is what
    // keeps rule 2 true of a malformed annotation.
    annotation_enum_value: $ => token(prec(3, /\([-0-9,\[\] ]*\)/)),
    annotation_number: $ => token(prec(2,
      /-?(0[xX][0-9A-Fa-f]+|[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?)/)),
    annotation_eq: $ => token(prec(2, /=/)),
    annotation_semi: $ => token(prec(2, /;/)),
    // `[packed=true]` is the only bracketed group the format defines,
    // but matching any of them keeps rule 2 above true of a malformed
    // one too.
    annotation_attribute: $ => token(prec(3, /\[[^\]\n]*\]/)),
    // The catch-all of rule 2. Its class excludes every character a
    // word, a number, an `=` or a `;` can start with, so it cannot take
    // one of those from under them. It does overlap the remaining two:
    // `[` is settled by `annotation_attribute`'s higher precedence, and
    // a leading `-` by `annotation_number` matching further — which is
    // also why an unterminated `[` or a bare `-` still lands here.
    annotation_junk: $ => token(prec(2, /[^A-Za-z0-9_;=\n \t]+/)),
    // The one token allowed to reach a line break.
    //
    // Ranked above `annotation_marker`, not merely above the item
    // tokens. Extras are offered inside an extra too, so at the newline
    // the lexer weighs ending this annotation against opening a fresh
    // one on the next line, and would otherwise skip the newline to do
    // it and take the following lines with it. Precedence is compared
    // before match length, so this is what settles it — and it settles
    // the contest with the `/\s/` extra the same way, which is why the
    // token can match the bare newline and leave any spaces before it to
    // be skipped, rather than spelling a `[ \t]*` prefix and running
    // into rule 4.
    annotation_end: $ => token(prec(3, /\r?\n/)),
  },

  // Local removal (spec 0196 S6): upstream also declared
  //   precedences: $ => [["message_list", "scalar_list"]],
  // which orders two *named* precedences. Neither name is ever used —
  // `message_list` and `scalar_list` carry numeric `prec(2)`/`prec(1)` —
  // so the block had no effect.
  extras: $ => [
    /\s/,
    $.annotation,
    $.comment,
  ],
});
