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
    single_string_contents: $ => token(prec(1, /[^\n'\\]+/)),
    double_string_contents: $ => token(prec(1, /[^\n"\\]+/)),

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
    // Local fix (spec 0196 S5): the suffix used to be a plain `/[Ff]/`,
    // which ties with `identifier` on length in the same state, so it
    // never attached — `1.5f` parsed as `float_lit` plus a sibling
    // ERROR. `token.immediate` matches only with no preceding
    // whitespace, which is exactly what separates a float suffix from
    // the next field's name.
    float: $ => choice(
      seq($.float_lit, optional(token.immediate(/[Ff]/))),
      seq($.dec_int, token.immediate(/[Ff]/)),
    ),

    comment: $ => seq('#', /.*/)
  },

  // Local removal (spec 0196 S6): upstream also declared
  //   precedences: $ => [["message_list", "scalar_list"]],
  // which orders two *named* precedences. Neither name is ever used —
  // `message_list` and `scalar_list` carry numeric `prec(2)`/`prec(1)` —
  // so the block had no effect.
  extras: $ => [
    /\s/,
    $.comment,
  ],
});
