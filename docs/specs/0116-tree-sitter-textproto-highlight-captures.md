<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0116 — `tree-sitter-textproto`: richer highlight captures in `queries/highlights.scm`

**Status:** implemented
**Implemented in:** 2026-07-13
**Refs:** `docs/specs/0065-textproto-structural-scanner.md` (upstream pin,
licence, repository layout); `docs/specs/0110-render-sink-unification.md`
(`decode_and_render_indexed`/`TextSink`); `docs/specs/0111-protolens-v1-decode-navigate-extract.md`
(`NodeSpan`); `docs/specs/0114-protolens-range-type-override.md` §6
(`CandidateCache`, the byte-bounded MRU cache this spec's render cache
mirrors)
**App:** tree-sitter-textproto, protolens

---

## Background

Spec 0065 established `oss-prototools`'s dependency on
[`PorterAtGoogle/tree-sitter-textproto`](https://github.com/PorterAtGoogle/tree-sitter-textproto)
(ISC licence) for structural scanning of textproto source, fetched at Nix
build time from a pinned commit. Per spec 0065's "Source acquisition
strategy", `grammar.js`, `src/parser.c`, and every other
machine-generated grammar-authoring file remain **deliberately not
committed** to this repo — they stay fetched-and-generated at Nix build
time (`treeSitterTextprotoSrc`'s `fetchzip` + `tree-sitter generate`).

`queries/highlights.scm` is this spec's one exception: unlike
`parser.c`, it's a small, hand-authored file, not machine-generated —
so, following the exact precedent already set by
`reproto/tree-sitter-textproto/binding.c` (a small file this repo
writes and commits itself, rather than trusting upstream's generated
`bindings/` output), `oss-prototools` commits and owns its *own*
`queries/highlights.scm` directly in `reproto/tree-sitter-textproto/`
— no separate external clone or fork needed to develop or use it.
Upstream's copy remains useful only as this spec's starting point and
ground truth (quoted below); once this spec's file is committed here,
`oss-prototools`'s Nix build stops reading `queries/` out of the
`treeSitterTextprotoSrc` fetch entirely (§7's "Nix wiring") — it reads
our own committed copy instead.

Upstream's `grammar.js` (fetched, not committed, not touched by this
spec) currently pairs with a minimal `queries/highlights.scm` — five
*capture names* — `@string`, `@attribute`, `@comment`, `@number`,
`@punctuation.bracket` — spread over seven patterns (confirmed by
fetching the upstream repo tree at the pinned commit,
`568471b80fd8793d37ed01865d8c2208a9fefd1b`, out of the Nix store — see
`docs/specs/0065-textproto-structural-scanner.md`'s `fetchzip`
derivation; `oss-prototools`'s own build only ever copies `grammar.js`
out of that tree today, but the store output still holds the full
upstream source, including `queries/` and `example-file.textproto`,
ground-truthing this spec's node/field names below):

```scheme
(string) @string
(field_name) @attribute
(comment) @comment
(number) @number
; For stuff like "inf" and "-inf".
(scalar_value (identifier)) @number
(scalar_value (signed_identifier)) @number
(open_squiggly) @punctuation.bracket
(close_squiggly) @punctuation.bracket
(open_square) @punctuation.bracket
(close_square) @punctuation.bracket
(open_arrow) @punctuation.bracket
(close_arrow) @punctuation.bracket
```

This is enough for coarse syntax coloring but loses information the
grammar already exposes structurally — e.g. an `extension_name`/
`any_name` field reference (which names *another* message type) is
captured identically to an ordinary `field_name`, and every bracket/
brace in the file — message body, repeated-value list, extension
reference — shares one capture. It also has a latent inaccuracy Goal 4
addresses: the `(scalar_value (identifier|signed_identifier))
@number` pair is **unconditional** — despite the comment, it captures
*every* bare identifier scalar (`true`, `false`, any enum-value name,
not just `inf`/`-inf`) as `@number`.

## Goals

Extend `queries/highlights.scm` with more granular captures, purely by
adding new query patterns (or narrowing existing ones with more specific
node paths) — no grammar changes:

1. **`extension_name` / `any_name` get a distinct capture** (`@type`),
   separate from ordinary `field_name` — they name another message type,
   not a field on the current message.
2. **Within `any_name`, `domain` and `type_name` are captured
   separately**: `domain` (e.g. `type.googleapis.com`) as
   `@string.special.url`, `type_name` (e.g. `pkg.Type`) as `@type`
   (consistent with Goal 1).
3. **`string_escape` nodes get `@string.escape`**, distinct from the
   surrounding `@string` on the enclosing string literal.
4. **`scalar_value (identifier)` / `scalar_value (signed_identifier)`**
   (bare, unquoted identifiers as a scalar field's value — enum-value
   names, or a bool literal, depending on the field's schema, which the
   grammar cannot see) get a documented default capture:
   - `"true"` / `"false"` specifically → `@boolean`.
   - `"inf"` / `"-inf"` specifically → stays `@number` (today's existing
     behavior for these two values, narrowed — see §4).
   - Every other identifier value → `@constant` (the chosen default for
     "probably an enum value name" — see §4 for the reasoning to record
     in a comment).
   - Confirmed (§4): today's `(scalar_value (identifier|
     signed_identifier)) @number` pair is **not** actually an
     `inf`/`-inf`-only special case — it's unconditional, capturing
     every bare identifier scalar as `@number`. This goal *narrows*
     that existing pattern to `inf`/`-inf` text only, then adds the new
     `@boolean`/`@constant` patterns for everything else — a real
     behavior change for non-`inf`/`-inf` identifiers (they currently
     render `@number`), which is the explicit point of this goal.
5. **`:`, `,`, `;` get `@punctuation.delimiter`** (currently uncaptured).
6. **`@punctuation.bracket` is split by context**, each new capture
   falling back to the existing generic name:
   - Message body braces (`open_squiggly`/`close_squiggly`,
     `open_arrow`/`close_arrow`) keep the plain `@punctuation.bracket`
     (unchanged — this is the "default" bracket kind, per Constraints).
   - List brackets on `message_list`/`scalar_list` →
     `@punctuation.bracket.list`.
   - Brackets around `extension_name`/`any_name` →
     `@punctuation.bracket.extension`.

7. **Ship a `test/highlight/` test file**, committed in-repo alongside
   the new `queries/highlights.scm`
   (`reproto/tree-sitter-textproto/test/highlight/textproto.txt`),
   exercising every new capture (see Test plan). Wired into
   `default.nix` as a hermetic Nix check (§7) that assembles the
   fetched `grammar.js` with our own committed `highlights.scm`/test
   file and runs `tree-sitter generate && tree-sitter test` — no
   external clone or manual step needed to run it.
8. **Deliver a capture-name summary**: once implemented, list every
   capture name introduced/refined and the exact node it targets, so
   colors can be assigned downstream (this is a communication
   deliverable of the implementation, not a file to commit).
9. **protolens becomes a Rust consumer** of the compiled grammar and
   `queries/highlights.scm`: it parses its own `TextSink`-rendered
   output (already textproto syntax) and runs the highlight query over
   it to classify text spans for its `ratatui`-based TUI. protolens is
   the *only* consumer this spec designs for — no generic editor/theme
   integration (§7).
10. **Cache `(range, type) → styled render`**: `apply_override`
    (`protolens/src/tui.rs`) is currently the only call site (besides
    initial document load) of `decode_and_render_indexed`, invoked
    fresh every time the user commits (`Enter`) a candidate type for a
    range — including re-committing a type already seen for that same
    range. Cache the render+colorize result, keyed by `(payload byte
    range, type)`, with the same byte-bounded MRU structure and
    eviction strategy as `CandidateCache` (spec 0114 §6,
    `protolens/src/override_pane.rs`), so ping-ponging between
    previously-seen types for a range skips both passes on a hit (§8).
11. **The colorizer emits *style roles*, not colors.** The tree-sitter
    query pass classifies each text span by a semantic `SyntaxRole`
    (one per capture name from Goals 1–6/Background) — it never
    produces a `ratatui::style::Color` or `Style` itself. Mapping a
    `SyntaxRole` to an actual color is a separate, later step (a
    `Theme`), so a cached render (Goal 10) survives a theme switch
    without re-parsing or re-querying (§9).

## Non-goals

- **No changes to `grammar.js`, `src/parser.c`, `src/grammar.json`, or
  `bindings/`** — this is a query-only change. If the grammar itself
  turns out to be missing a node needed for one of the Goals (unlikely,
  per the Background's confirmation these nodes already exist), that is
  out of scope for this spec and would need its own.
  **Superseded (spec 0121, 2026-07-14)**: `grammar.js` is now vendored
  and locally modified — see `docs/specs/0121-tree-sitter-textproto-field-no-vendoring.md`.
- **No change to how `grammar.js`/`src/parser.c` are sourced** — still
  fetched-and-generated at Nix build time, never committed (Background).
  Only `queries/highlights.scm` gains committed, in-repo ownership; no
  external grammar clone or fork is introduced or required by this spec.
  **Superseded (spec 0121, 2026-07-14)**: `grammar.js` is now committed
  in-repo too (`reproto/tree-sitter-textproto/grammar.js`), following the
  same "own it locally" precedent this spec already set for
  `highlights.scm`/`binding.c` — only `src/parser.c` (machine-generated)
  remains fetched-and-generated, unchanged.
- **No renaming or removal of the existing 5 captures.** New captures
  are added alongside them, or existing patterns are narrowed to more
  specific node paths that additionally emit a new capture — consumers
  relying on `@string`/`@attribute`/`@comment`/`@number`/
  `@punctuation.bracket` continuing to fire on the same nodes they do
  today must see no regression.
- **No schema-aware disambiguation** of Goal 4's enum-value-vs-bool
  question — the grammar (and therefore this query file) has no access
  to a `.proto` schema, so `@constant` for non-`true`/`false`
  identifiers is a fixed, schema-blind default, not a real
  classification.
- **No re-probing color depth after startup, and no querying for
  capabilities other than RGB support.** §9's RGB-vs-ANSI-16 signal is
  resolved once, cached for the process's lifetime, and checked in the
  same layered order Vim uses (patch 9.1.1060, vim/vim#16490):
  `COLORTERM` (`truecolor`/`24bit`), then a live XTGETTCAP query to the
  terminal for the `RGB` capability, then a passive terminfo database
  lookup (`RGB`/`Tc` boolean capabilities, or a `max_colors` value of
  `0x1000000`) for terminals that don't answer the live query. No other
  termcap/terminfo capabilities are probed. (Distinct from §9's
  background-*luminance* probe for `system` theme resolution, which is
  a separate, unrelated check — that one decides *which* palette pair,
  dark or light, to use, via an OSC 11 query; the RGB-vs-ANSI-16 signal
  decides color depth within whichever pair is chosen.)
- **No user-configurable or pluggable theme system** — exactly two
  fixed, built-in palettes (§9); no config file, no per-`SyntaxRole`
  user overrides, no third-party theme format support.
- **No live preview while browsing the override pane's candidate list
  before committing.** Confirmed (§8): `decode_and_render_indexed` has
  exactly one call site today outside the initial document load —
  `apply_override`, triggered only by `Enter`. This spec's cache (Goal
  10) targets repeated *commits* of a previously-seen type for the
  same range, not a hypothetical render-as-you-browse feature; adding
  the latter is out of scope here.

---

## Specification

### §1/§2 — `extension_name`, `any_name`, and `any_name`'s children

Confirmed against `grammar.js`: `any_name`'s `domain`/`type_name` are
plain positional children, not named grammar fields (`any_name: $ =>
seq($.open_square, $.domain, "/", $.type_name, $.close_square)`, no
`field()` wrapper) — the query matches by child node type, not a
field name.

```scheme
(extension_name) @type

(any_name
  (domain) @string.special.url
  (type_name) @type)
```

**Superseded by spec 0225 S12**: `(extension_name) @type` is gone, and an
extension name falls through to the blanket `(field_name) @attribute`.
`[acme.blade]: 3` names a field, and on the wire it is a tag like any
other field's, so it belongs in the field-name color. The `any_name`
pattern stands: an Any key does hold a type name.

The outer `any_name @type` capture (Goal 1's general case) and the
inner `type_name @type` capture (Goal 2) target the same capture name
by design — `any_name`'s `type_name` child *is* the "other message
type" reference, consistent with `extension_name`'s.

**Precedence note**: `field_name: $ => choice($.extension_name,
$.any_name, $.identifier)` has no other tokens — when its child is an
`extension_name`/`any_name`, `field_name`'s byte range is identical to
its child's. Since today's `(field_name) @attribute` pattern still
fires there too, the new `@type` patterns above must be declared
*after* it in `highlights.scm` so tree-sitter's last-match-wins
precedence lets `@type` stand for that range (consistent with Goal 1's
intent — an `extension_name`/`any_name` is explicitly meant to look
different from a plain `field_name`).

### §3 — `string_escape`

```scheme
(string_escape) @string.escape
```

Added as a sibling pattern, not a modification of whatever pattern
currently captures the enclosing string as `@string` — both captures
fire (the enclosing string node keeps `@string`; unrecognized captures
from unsupported themes fall back to the surrounding `@string`
region rendering unescaped, which is the strictly-worse-but-harmless
default anyway).

### §4 — bare identifier scalar values

Confirmed against `grammar.js`: `scalar_value: $ => choice(repeat1
($.string), $.identifier, $.signed_identifier, $.number)` and
`signed_identifier: $ => seq("-", $.identifier)` — so `"-inf"` is a
`signed_identifier` wrapping an `identifier` whose own text is
`"inf"`; `"true"`/`"false"` can only ever appear as a bare
`identifier` (never `signed_identifier`, since that grammar rule
always prepends `-`). Today's existing `(scalar_value (identifier))
@number` / `(scalar_value (signed_identifier)) @number` pair (quoted
in full in Background) is unconditional, not restricted to `inf`/
`-inf` text — this section **narrows** it rather than adding a new
pattern after an already-selective one:

```scheme
; Blanket default, declared first: schema-blind default. Without a
; .proto schema, the grammar cannot tell an enum value name
; (KNOWN_ENUM_VALUE) from a bool/inf value handled below, or (for that
; matter) from a genuinely invalid/unknown scalar. @constant is chosen
; as the least-wrong default: enum-value-name identifiers are by far
; the more common case for a *non*-true/false/inf bare identifier in
; practice, and @constant is the closest standard capture semantically
; ("a named, unchanging value"). Declared first so the more specific
; patterns below can override it (tree-sitter's last-match-wins
; precedence is declaration order, not predicate specificity).
(scalar_value (identifier) @constant)
(scalar_value (signed_identifier) @constant)

((scalar_value (identifier) @boolean)
 (#any-of? @boolean "true" "false"))

; Narrowed from today's unconditional pattern (Background) — @number
; now applies only to the inf/-inf identifier values it was actually
; meant for. Declared last (most specific) so it wins over both
; patterns above for these two exact values.
((scalar_value (identifier) @number)
 (#eq? @number "inf"))
((scalar_value (signed_identifier) @number)
 (#eq? @number "-inf"))
```

**Precedence note**: all three pattern groups above can match the
same node (the blanket `@constant` patterns have no predicate, so they
always match); tree-sitter resolves this via last-match-wins by
*declaration order in the file*, not predicate specificity — so the
more specific `@boolean` and `@number`(`inf`/`-inf`) patterns must be
declared *after* the blanket `@constant` ones, not before.

### §5 — delimiter punctuation

```scheme
[":" "," ";"] @punctuation.delimiter
```

### §6 — split `@punctuation.bracket` by context

Confirmed against `grammar.js`: braces/angle-brackets use dedicated
named token rules — `open_squiggly`/`close_squiggly` (`{`/`}`),
`open_arrow`/`close_arrow` (`<`/`>`) — left untouched, unchanged from
today's `highlights.scm` (Background). Square brackets are a single
pair of named token rules, `open_square`/`close_square` (`[`/`]`),
reused as-is across four different contexts (`message_list`,
`scalar_list`, `extension_name`, `any_name`) — there's no separate
token per context, so disambiguation must go through the *parent*
node, matching a child `(open_square)`/`(close_square)` scoped inside
each parent pattern:

```scheme
; Message body braces/angle-brackets: unchanged, stays the plain,
; generic capture (today's existing pattern, kept as-is per
; Constraints — not rewritten).
(open_squiggly) @punctuation.bracket
(close_squiggly) @punctuation.bracket
(open_arrow) @punctuation.bracket
(close_arrow) @punctuation.bracket

(message_list (open_square) @punctuation.bracket.list)
(message_list (close_square) @punctuation.bracket.list)
(scalar_list (open_square) @punctuation.bracket.list)
(scalar_list (close_square) @punctuation.bracket.list)

(extension_name (open_square) @punctuation.bracket.extension)
(extension_name (close_square) @punctuation.bracket.extension)
(any_name (open_square) @punctuation.bracket.extension)
(any_name (close_square) @punctuation.bracket.extension)
```

These context-scoped patterns must be declared *after* today's
existing blanket `(open_square) @punctuation.bracket` / `(close_square)
@punctuation.bracket` pattern (Background) so the more specific
`.list`/`.extension` captures win by last-match-wins declaration
order, per the same precedence rule noted in §1/§2 and §4 — the
blanket pattern is otherwise still needed as-is (Non-goals: existing 5
captures preserved) to cover any future `open_square`/`close_square`
usage this spec doesn't enumerate.

**Fallback semantics caveat**: dotted-capture-name fallback (e.g. a
consumer that only knows `@punctuation.bracket` still highlighting
`@punctuation.bracket.list` nodes, by stripping the last dot-segment) is
a documented Neovim `nvim-treesitter` behavior, not a tree-sitter-query
language guarantee — other consumers (Helix, Zed, `tree-sitter
highlight`'s own default theme, etc.) may or may not implement the same
fallback. This is inherent to the ecosystem, not something this spec's
query file can control; noted here so the fallback isn't assumed
universal when validating downstream.

### §7 — protolens: Rust grammar binding + colorizer

**Confirmed defect in upstream's Rust bindings**: the fetched pinned-
commit source's `bindings/rust/lib.rs` is the *unfilled*
`tree-sitter-cli` scaffold template — `extern "C" { fn
tree_sitter_YOUR_LANGUAGE_NAME() -> Language; }`, never renamed to the
grammar's real exported symbol. The real symbol is `tree_sitter_textproto`
— confirmed two ways: `grammar.js`'s `name: 'textproto'` (tree-sitter's
`tree_sitter_<name>` naming convention), and this repo's own working
Python `binding.c` (`reproto/tree-sitter-textproto/binding.c`), which
already correctly declares `extern const TSLanguage
*tree_sitter_textproto(void);`. `bindings/rust/lib.rs`'s
`HIGHLIGHTS_QUERY`/`NODE_TYPES` consts are also commented out except
`NODE_TYPES`. protolens must not depend on `tree-sitter-textproto`'s
published Rust crate as-is (see Open Issues for the fix-upstream vs.
vendor-our-own-shim choice).

**Design**: protolens links the compiled grammar directly — but, per
Nix-friendliness (auto-generated C code must never be generated at
`cargo build` time, only linked), the *entire* C compile happens
inside Nix, exactly like this repo's existing Python `.so`
derivation. protolens's `build.rs` never invokes a compiler or
`tree-sitter generate` itself — it only emits linker flags pointing at
a Nix-built static library. `colorize.rs` then wraps the linked symbol
in a small, hand-written, correctly-named `extern` declaration (the
same precedent already set by this repo's own `binding.c`, not
trusted to upstream's `bindings/`):

```rust
unsafe extern "C" { fn tree_sitter_textproto() -> tree_sitter::Language; }

fn language() -> tree_sitter::Language {
    unsafe { tree_sitter_textproto() }
}
```

**Nix wiring** (resolves Open Issue 8): three derivations, chained so
`tree-sitter generate`'s auto-generated output (`src/parser.c`/
`src/parser.h`) is produced exactly once and shared by both the
existing Python binding and the new Rust static lib — reusing the
existing pinned `treeSitterTextprotoSrc` fetch, no new hash:

```nix
# Codegen only, hermetic (pkgs.tree-sitter CLI + already-pinned
# grammar.js — no network). Intermediate derivation — same "expose a
# compiled/generated artifact, not raw source" shape as reproto's own
# treeSitterTextprotoSrc -> treeSitterTextproto pattern — so both
# downstream consumers share one `tree-sitter generate` run instead
# of each re-running it. queries/highlights.scm now comes from our
# own committed file (Background), not from treeSitterTextprotoSrc.
treeSitterTextprotoGenerated = pkgs.stdenv.mkDerivation {
  name = "tree-sitter-textproto-generated";
  dontUnpack = true;
  nativeBuildInputs = [ pkgs.tree-sitter ];
  buildPhase = ''
    cp ${treeSitterTextprotoSrc}/grammar.js .
    tree-sitter generate
  '';
  installPhase = ''
    mkdir -p $out/src $out/queries
    cp src/parser.c src/parser.h $out/src/
    cp ${./reproto/tree-sitter-textproto/highlights.scm} $out/queries/highlights.scm
  '';
};

# Existing Python .so derivation, refactored to consume the shared
# generated source above instead of running `tree-sitter generate`
# itself — buildPhase now compiles binding.c against
# ${treeSitterTextprotoGenerated}/src/parser.c directly; installed
# textproto*.so bytes unchanged.
treeSitterTextproto = pkgs.stdenv.mkDerivation { /* ... */ };

# New: a Rust-linkable static library, compiled once in Nix — exactly
# like the Python .so is. protolens's build.rs never compiles C or
# runs tree-sitter itself; it only links this pre-built artifact.
treeSitterTextprotoRustLib = pkgs.stdenv.mkDerivation {
  name = "tree-sitter-textproto-rust-lib";
  dontUnpack = true;
  buildPhase = ''
    $CC -c -fPIC -I ${treeSitterTextprotoGenerated}/src \
      ${treeSitterTextprotoGenerated}/src/parser.c -o parser.o
    ar rcs libtree_sitter_textproto.a parser.o
  '';
  installPhase = ''
    mkdir -p $out/lib $out/queries
    cp libtree_sitter_textproto.a $out/lib/
    cp ${treeSitterTextprotoGenerated}/queries/highlights.scm $out/queries/
  '';
};

# Hermetic regression check for queries/highlights.scm (Goal 7, Test
# plan) — assembles the fetched upstream tree with our own committed
# highlights.scm + test/highlight/textproto.txt overlaid, then runs
# `tree-sitter generate && tree-sitter test`. Building this derivation
# *is* the test — a capture regression fails the Nix build. No
# external clone or manual step needed.
treeSitterTextprotoHighlightTest = pkgs.runCommand
  "tree-sitter-textproto-highlight-test"
  { nativeBuildInputs = [ pkgs.tree-sitter ]; }
  ''
    cp -r ${treeSitterTextprotoSrc} work
    chmod -R +w work
    cd work
    rm -rf queries test
    mkdir -p queries test/highlight
    cp ${./reproto/tree-sitter-textproto/highlights.scm} queries/highlights.scm
    cp ${./reproto/tree-sitter-textproto/test/highlight/textproto.txt} \
      test/highlight/textproto.txt
    tree-sitter generate
    tree-sitter test
    touch $out
  '';
```

Exposed at the top level as `tree-sitter-textproto-highlight-test` and
added to the `ci`/`ci-no-clippy` `linkFarmFromDrvs` lists, so a
`queries/highlights.scm` regression fails CI like any other check.

`nix/rust.nix`'s `commonArgs` gains two new `env` entries (same
mechanism as today's `PYO3_PYTHON`/`RUSTFLAGS`):

```nix
env.TREE_SITTER_TEXTPROTO_LIB_DIR     = "${treeSitterTextprotoRustLib}/lib";
env.TREE_SITTER_TEXTPROTO_QUERIES_DIR = "${treeSitterTextprotoRustLib}/queries";
```

`protolens/build.rs` becomes a thin, Nix-supplied-artifact-linking
shim — no `cc` crate dependency, no C compilation, no `tree-sitter
generate` invocation:

```rust
fn main() {
    let lib_dir = std::env::var("TREE_SITTER_TEXTPROTO_LIB_DIR")
        .expect("set by nix-shell/nix-build — see nix/rust.nix");
    let queries_dir = std::env::var("TREE_SITTER_TEXTPROTO_QUERIES_DIR")
        .expect("set by nix-shell/nix-build — see nix/rust.nix");
    println!("cargo:rustc-link-search=native={lib_dir}");
    println!("cargo:rustc-link-lib=static=tree_sitter_textproto");
    // Forwarded to compile-time `env!()` so `colorize.rs` can
    // `include_str!(concat!(env!("TREE_SITTER_TEXTPROTO_QUERIES_DIR"),
    // "/highlights.scm"))`.
    println!("cargo:rustc-env=TREE_SITTER_TEXTPROTO_QUERIES_DIR={queries_dir}");
}
```

`nix/shells.nix`'s `_hook_rust` also exports both vars (same
precedent as `RUSTFLAGS="${pyo3Rustflags}"`), so a manual `cargo build
--release -p protolens` inside the project's `nix-shell` links
correctly without a full `nix-build`.

**`tree-sitter`/`tree-sitter-highlight` crate versions** (resolves Open
Issue 6): since protolens never depends on upstream's published
`tree-sitter-textproto` crate or its `Cargo.toml` (vendored shim
above), its stale `tree-sitter = "~0.20.10"` pin doesn't bind us.
`protolens/Cargo.toml` picks whatever current `tree-sitter` crate
release is ABI-compatible with `pkgs.tree-sitter`'s CLI-emitted
`LANGUAGE_VERSION` (a one-time compatibility check at implementation
time, not a design decision this spec needs to fix a number for), and
pins `tree-sitter-highlight` to the same release line as `tree-sitter`
(the two crates are versioned together upstream).

**Colorizer**: parses protolens's own already-rendered textproto text
(`decode_and_render_indexed`'s `TextSink` output) with this grammar
and turns `queries/highlights.scm`'s (§1–§6) captures into
`StyleHint`s — no `ratatui::style::Color` or `Style` is produced here
(Goal 11; colors are §9's job).

Uses the `tree-sitter-highlight` crate's `Highlighter`/
`HighlightConfiguration`/`HighlightEvent` API — not raw
`tree_sitter::Query`/`QueryCursor` run by hand. This is not a stylistic
choice: overlapping/identical-range captures (e.g. `extension_name`'s
`@type` fully overlapping the outer `field_name`'s `@attribute`, or
`.list`/`.extension` bracket sub-captures overlapping the blanket
`@punctuation.bracket`) do not resolve via simple "last-declared-
pattern replaces the range" semantics — `tree-sitter-highlight` models
overlapping captures as a nested stack of highlight-start/highlight-end
events, where a later-declared pattern becomes the *innermost* (winning)
layer nested inside earlier-declared patterns, which remain present as
outer layers rather than being discarded (confirmed empirically against
`tree-sitter test`'s own assertion checker, which matches only the
innermost/top-of-stack capture at a given position — the same
precedence model this spec's `highlights.scm` precedence notes in
§1–§6 rely on). Hand-rolling this stack logic over raw `QueryMatch`es
would just reimplement `tree-sitter-highlight`; using the crate directly
is both simpler and guaranteed to match `tree-sitter test`'s behavior,
which is what `treeSitterTextprotoHighlightTest` (§7) checks. The
colorizer walks the `HighlightEvent` stream, tracks the current
highlight-name stack, and emits one `StyleHint` per `Source` span using
the top-of-stack `SyntaxRole` (or none, if the stack is empty):

```rust
/// One semantic role a rendered text span can have — one variant per
/// capture name from Background/Goals 1–6. `Copy`, tag-sized — kept
/// cheap so `StyleHint`s are inexpensive to cache (§8).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SyntaxRole {
    Attribute,          // @attribute        (field_name)
    Type,                // @type             (extension_name/any_name/type_name)
    StringLiteral,       // @string
    StringEscape,        // @string.escape
    StringSpecialUrl,    // @string.special.url (any_name's domain)
    Comment,              // @comment
    Number,               // @number
    Boolean,              // @boolean
    Constant,             // @constant
    PunctuationDelimiter, // @punctuation.delimiter
    PunctuationBracket,          // @punctuation.bracket
    PunctuationBracketList,      // @punctuation.bracket.list
    PunctuationBracketExtension, // @punctuation.bracket.extension
}

/// A capture's span within the *rendered text*, tagged with its role
/// — deliberately not a color; §9 resolves that separately, per theme.
struct StyleHint {
    range: Range<usize>,
    role: SyntaxRole,
}
```

### §8 — Render cache: `(range, type) → (text, spans, style hints)`

**Motivation**: `apply_override` (`protolens/src/tui.rs`) is the only
call site of `decode_and_render_indexed` besides the initial document
load, invoked on every `Enter` commit of a candidate type for the
override target — including re-committing a type already seen for
that same range (ping-ponging between candidates, Goal 10). Each
commit currently re-runs the full render (`TextSink`) *and* the new
colorize pass (§7) from scratch, even when both have already been
computed for that exact `(range, type)` pair earlier in the session.

**Design**: a byte-bounded MRU cache, `RenderCache`, structurally
identical to `CandidateCache` (spec 0114 §6) — a `Vec` of entries
ordered LRU-at-front/MRU-at-back, evicted by a byte budget, `get`
promoting to MRU on a hit:

```rust
/// Key: the same `payload_range` `apply_override` already computes
/// via `extract::message_payload_range`, plus the type it was
/// rendered under (`None` = raw/schema-less override) — the exact two
/// inputs that determine `decode_and_render_indexed`'s output today.
type RenderKey = (Range<usize>, Option<String>);

/// Value: everything `apply_override` derives from a fresh
/// `decode_and_render_indexed` call plus the colorize pass (§7) — a
/// cache hit skips *both* passes; only the splice/index-adjustment
/// logic already downstream of the render call in `apply_override`
/// still runs, unchanged by this spec.
type RenderValue = (Vec<String>, Vec<NodeSpan>, Vec<StyleHint>);

pub struct RenderCache {
    entries: Vec<(RenderKey, RenderValue)>,
    total_bytes: usize,
    max_bytes: usize,
}
```

Sizing/eviction mirrors `CandidateCache`'s `candidates_bytes` heuristic
— approximate footprint = rendered lines' string bytes +
`new_spans.len() * size_of::<NodeSpan>()` + `style_hints.len() *
size_of::<StyleHint>()`, deliberately approximate (same "not
correctness-sensitive" caveat as `CandidateCache`'s doc comment). A new
`RENDER_CACHE_MAX_BYTES` constant, sized independently of
`CANDIDATE_CACHE_MAX_BYTES` (`1 << 20` today) since entries are
heavier — exact value an implementation-time tuning choice (Open
Issues).

**Invalidation**: none needed beyond ordinary MRU eviction — a cached
entry's key is tied to immutable input (`self.blob`'s bytes never
change once a document is loaded, and the type is part of the key), so
an entry stays valid for the loaded document's lifetime.

### §9 — Theme: mapping `SyntaxRole` to `Style`

**protolens currently has no theme concept at all** — confirmed: zero
uses of `ratatui::style::Color` anywhere in `protolens/src`; the TUI's
only existing `Style` usage is `Style::default().add_modifier
(Modifier::REVERSED)` / `Modifier::BOLD)` for cursor/selection
highlighting (no foreground/background color anywhere). This spec
introduces protolens's first notion of theme.

Two fixed, built-in palette *pairs* — `ThemeKind::Dark`/
`ThemeKind::Light` — each map every `SyntaxRole` to a
`ratatui::style::Style`, in one of two color depths:

- **RGB** (`Color::Rgb`), used when the terminal advertises 24-bit
  color support, checked in the same layered order Vim uses (patch
  9.1.1060, vim/vim#16490): `COLORTERM=truecolor` or `COLORTERM=24bit`
  (the same signal `bat`, `delta`, and most other Rust terminal tools
  key off) first; then, if unset, a live XTGETTCAP query to the
  terminal for the `RGB` termcap capability; then, if the terminal
  doesn't answer that query, a static terminfo capability probe
  (`RGB`/`Tc` boolean capabilities, or `max_colors == 0x1000000`). The
  live query catches terminals that support true color but whose
  terminfo entry doesn't advertise it. See Non-goals for what this
  probe does *not* do. Colors are borrowed directly from VSCode's
  built-in Dark+/Light+ themes (see "RGB palette" below).
- **ANSI-16** (portable `ratatui::style::Color` variants only —
  `Black`/`Red`/`Green`/`Yellow`/`Blue`/`Magenta`/`Cyan`/`Gray`/
  `DarkGray`/`LightRed`/`LightGreen`/`LightYellow`/`LightBlue`/
  `LightMagenta`/`LightCyan`/`White`), used as a fallback whenever
  neither signal above confirms truecolor support. This is the
  original ANSI-16 palette from this spec's first implementation,
  unchanged.

A third `ThemeKind` value, `System`, exists only at the CLI-selection
layer (see "Selection mechanism" below) — it is resolved to `Dark` or
`Light` once at startup, before any rendering happens; `style_for`
itself only ever takes the resolved `Dark`/`Light` variant, never
`System`. The RGB-vs-ANSI-16 choice is orthogonal to `Dark`/`Light`
and is *not* part of `ThemeKind` — it's re-checked inside `style_for`
itself: the `COLORTERM` lookup is a cheap env read with no caching;
the XTGETTCAP query and the terminfo probe are each cached, evaluated
once and reused for the process's lifetime (`TERM` doesn't change at
runtime, and a terminal's live-query answer can't either). The
XTGETTCAP query is primed explicitly at startup (`main.rs`, alongside
`resolve_system`) so it runs before the TUI takes over the terminal
with its own input-polling loop, and is skipped entirely (returning
`false` with no terminal I/O) when stdin/stdout aren't real terminals
(e.g. under `cargo test`).

```rust
fn style_for(role: SyntaxRole, theme: ThemeKind) -> ratatui::style::Style;
```

**RGB palette** (primary, used under `COLORTERM=truecolor`/`24bit`, the
XTGETTCAP live-query fallback, or the terminfo fallback):
borrowed from VSCode's `dark_plus.json`/`light_plus.json` (and their
`dark_vs.json`/`light_vs.json` base), cited per-role by TextMate scope
name below. Two roles have no upstream textproto-grammar analog in
VSCode (`PunctuationBracketList`/`PunctuationBracketExtension` are
this spec's own Goal 6 invention, distinguishing bracket *context* —
list vs. extension/Any — which no external tool's textproto grammar
does); their colors are repurposed from unrelated-but-visually-fitting
VSCode scopes, noted as such.

Three rows of the table below are **superseded by spec 0225 S12**, which
put the annotation echo on the same line as the document text and made
the borrowed VSCode colors collide there. `Type` is now orange (`#FE8019`
dark / `#AF3A03` light); `StringSpecialUrl` keeps the blue-green under
its own palette name; `Constant` takes `Number`'s color, a symbolic enum
value and the number it stands for being one fact seen twice. In the
ANSI-16 table below only `Constant` moved, to `Number`'s color for the
same reason; `Type` stays cyan/blue there, because sixteen colors hold no
orange and both warm candidates are already a severity tier.

| `SyntaxRole` | Dark RGB (scope) | Light RGB (scope) | Modifier |
|---|---|---|---|
| `Attribute` | `#9CDCFE` (`entity.other.attribute-name`) | `#E50000` (`entity.other.attribute-name`) | — |
| `Type` | `#4EC9B0` (`entity.name.type`/`support.type`) | `#267F99` (`entity.name.type`/`support.type`) | — |
| `StringLiteral` | `#CE9178` (`string`) | `#A31515` (`string`) | — |
| `StringEscape` | `#D7BA7D` (`constant.character.escape`) | `#EE0000` (`constant.character.escape`) | — |
| `StringSpecialUrl` | `#4EC9B0` (= `Type`) | `#267F99` (= `Type`) | Underlined |
| `Comment` | `#6A9955` (`comment`) | `#008000` (`comment`) | — |
| `Number` | `#B5CEA8` (`constant.numeric`) | `#098658` (`constant.numeric`) | — |
| `Boolean` | `#569CD6` (`constant.language`) | `#0000FF` (`constant.language`) | — |
| `Constant` | `#4EC9B0` (`entity.name.type.enum`, per the `pbkit.vscode-pbkit` textproto grammar) | `#267F99` (same) | — |
| `PunctuationDelimiter` | terminal default | terminal default | — |
| `PunctuationBracket` | terminal default | terminal default | — |
| `PunctuationBracketList` | `#DCDCAA` (`support.constant`, repurposed) | `#0451A5` (`support.constant`, repurposed) | — |
| `PunctuationBracketExtension` | `#D16969` (`string.regexp`, repurposed) | `#811F3F` (`string.regexp`, repurposed) | — |

Unlike the original ANSI-16-only design, `Attribute` (ordinary field
names) *does* get an explicit RGB color here (VSCode colors attribute/
field names too) — the "leave it terminal-default to stay unobtrusive"
rationale only applies to the two punctuation roles now, which VSCode
itself also leaves uncolored (no `punctuation`-scope rule in
Dark+/Light+, confirmed against the fetched theme JSON). `Type` and
`Constant` deliberately share one color (both map to VSCode
`entity.name.type`-prefixed scopes) — matches the `pbkit` extension's
own grammar, which scopes bare enum-value-shaped identifiers the same
as message/type names.

**ANSI-16 palette** (fallback, used otherwise) — unchanged from this
spec's original implementation:

| `SyntaxRole` | Dark: Color / Modifier | Light: Color / Modifier |
|---|---|---|
| `Attribute` | (terminal default) | (terminal default) |
| `Type` | Cyan / Bold | Blue / Bold |
| `StringLiteral` | Green | Green |
| `StringEscape` | LightGreen / Bold | Green / Bold |
| `StringSpecialUrl` | Cyan / Underlined | Blue / Underlined |
| `Comment` | DarkGray / Italic | DarkGray / Italic |
| `Number` | Blue | Cyan |
| `Boolean` | Magenta / Bold | Magenta / Bold |
| `Constant` | Magenta | Magenta |
| `PunctuationDelimiter` | DarkGray | DarkGray |
| `PunctuationBracket` | Gray | Black |
| `PunctuationBracketList` | Yellow | Yellow |
| `PunctuationBracketExtension` | LightRed | Red |

Rationale for the ANSI-16 dark/light split: the dark palette leans on
`Light*` variants for contrast against a dark background; the light
palette avoids them (`Light*` tends to wash out against a white/light
background) in favor of the base 8 colors.

**Selection mechanism**: a `--theme dark|light|system` CLI flag,
defaulting to `system` — mirroring the shape of protolens's existing
`--type` flag. Three fixed choices behind one flag, not a config file
or per-role override, so this doesn't reopen Non-goals' "no pluggable
theme system."

`system` queries the terminal's actual background once at startup,
via the `terminal-light` crate (small, already handles both detection
paths below plus the read timeout — not hand-rolled here):

1. `COLORFGBG` env var, if set (some terminals — rxvt, some xterm
   configs — export `fg;bg` ANSI color indices; no terminal I/O
   needed).
2. Otherwise, an OSC 11 query (`\x1b]11;?\x07`, read back with a
   bounded timeout) asking the terminal for its actual background RGB,
   thresholded on luminance. Handles tmux/screen's passthrough
   wrapping of the escape sequence.
3. If neither yields an answer (non-interactive/piped output,
   unsupported terminal, timeout) — falls back to `Dark`.

Resolution happens exactly once, before any rendering — no live
re-detection while running (matching this spec's existing "no live
preview" precedent, Non-goals).

---

## Test plan

New file, committed in-repo:
`reproto/tree-sitter-textproto/test/highlight/textproto.txt`. Confirmed
against the fetched upstream source: there is no existing `test/`
directory at all upstream (no `test/highlight/`, no `test/corpus/`,
`package.json`'s own `"test"` script is an unimplemented stub) — no
precedent to match, so this follows `tree-sitter test`'s own default
convention directly (`test/highlight/*.txt`, auto-discovered by
directory presence alone, no wiring needed beyond §7's
`treeSitterTextprotoHighlightTest` Nix check). Uses the standard
`tree-sitter test` highlight-assertion comment syntax (`<- capture` for
"same line, capture applies to the token this comment trails",
`^ capture` for "the token directly above this column").

Coverage required, one example of each:

1. A nested message (`outer { inner { … } }`).
2. A repeated scalar field (`vals: 1 vals: 2` or `vals: [1, 2]` —
   whichever form exercises `scalar_list`'s brackets per Goal 6, plus
   at least one bare `field { … }`-repeated form for comparison).
3. A repeated message field (`msgs { … } msgs { … }`).
4. An extension field: `[pkg.Ext] { … }` — exercising Goals 1 and the
   `@punctuation.bracket.extension` half of Goal 6.
5. An Any field: `[type.googleapis.com/pkg.Type] { … }` — exercising
   Goals 1, 2, and the `@punctuation.bracket.extension` half of Goal 6.
6. A string with at least one escape sequence (e.g. `"a\nb"`) —
   exercising Goal 3 (`@string.escape`) alongside the surrounding
   `@string`.
7. A float (exercising the existing `@number` capture is undisturbed).
8. A hex or octal int (same — confirms Goal 5/6 changes don't regress
   numeric-literal captures, which share lexical proximity with some of
   the newly-captured punctuation).
9. A comment (confirms the existing `@comment` capture is undisturbed).

Also include at least one bare (non-`true`/`false`/`inf`) enum-value-
shaped identifier scalar (Goal 4's `@constant` case), one `true`/
`false` scalar (Goal 4's `@boolean` case), and one `inf`/`-inf` scalar,
confirming it still renders `@number` after the narrowing (precedence
note in §4).

**Verification**:

- `tree-sitter test` — no special setup needed (Open Issue 3
  resolved): the CLI auto-discovers `test/highlight/*` by directory
  convention alone once `grammar.js` is regenerated, so adding
  `test/highlight/textproto.txt` is sufficient. Run automatically via
  `nix-build -A tree-sitter-textproto-highlight-test` (§7) — part of
  `ci`.
- Additionally: `tree-sitter highlight example-file.textproto` (the
  upstream fetch's existing example file, not committed —
  Background/Constraints), manually inspecting the terminal output
  against the new capture names for each construct in Goals 1–6,
  confirming no capture is missing or attached to the wrong
  node/range. `example-file.textproto`
  already exercises nested messages, a repeated scalar field
  (`nums`/`nums2`), a repeated message field (`objs`), an extension
  field (`[com.foo.ext.scalar]`/`[com.foo.ext.message]`), an Any field
  (`[type.googleapis.com/com.foo.any]`), a float (`1043E-04f`), a hex
  int (`0xfffFF00aeF`), an `-inf` scalar (`reg_scalar: -inf`), and
  comments — good manual-sanity-check coverage for most of Goals 1–6
  without needing to extend it (it has no string-escape or bare-
  boolean/enum-identifier example, so the new `test/highlight/` file
  remains necessary for full coverage per the list above).
- Confirm the existing 5 capture names still fire on exactly the same
  nodes as before this change, *except* where a Goal explicitly
  narrows one (Goal 1's `field_name`→`extension_name`/`any_name`
  override, Goal 4's `@number`→`@boolean`/`@constant` narrowing for
  non-`inf`/`-inf` identifiers, Goal 6's `@punctuation.bracket`→
  `.list`/`.extension` split) — spot-check against
  `example-file.textproto` before/after.

**protolens-side verification (§7–§9)**, separate from the in-repo
`tree-sitter test`/`tree-sitter highlight` check above:

- `cargo test -p protolens`: new unit tests for `RenderCache` mirroring
  `CandidateCache`'s existing three (`*_hit_promotes_to_most_recently_
  used`, `*_evicts_least_recently_used_past_byte_budget`, `*_keeps_
  oversized_entry_alone`) plus a test confirming `apply_override`
  skips `decode_and_render_indexed` on a `RenderCache` hit (a re-commit
  of a previously-seen `(range, type)` pair).
- A colorizer test asserting each Test-plan construct (1–9 above, plus
  the bare-identifier/boolean/`inf` cases) produces the expected
  `SyntaxRole` on the expected span, run against protolens's *own*
  rendered output (not the upstream fetch's `example-file.textproto`
  directly — protolens re-parses its own `TextSink` output, §7).
- Manual visual check of both `ThemeKind::Dark`/`Light` against a real
  terminal once implemented (Open Issue 10 — not verifiable in this
  sandbox).
- `system` resolution: unit-testable for the `COLORFGBG`-present path
  (set the env var, assert the expected `Dark`/`Light` resolution);
  the OSC 11 query path and its timeout fallback are not verifiable
  without a real interactive terminal (same sandbox limitation as
  above).

---

## Open Issues

1. ~~Exact grammar node/field names unconfirmed against `grammar.js`.~~
   **Resolved** — the upstream repo tree at the pinned commit
   (`568471b80fd8793d37ed01865d8c2208a9fefd1b`) was fetched out of the
   Nix store (the `treeSitterTextprotoSrc` `fetchzip` derivation from
   `docs/specs/0065-textproto-structural-scanner.md`/`default.nix` was
   already realized locally) and read directly. All node/field names
   used in §1–§6 above are ground-truthed against that `grammar.js`,
   not guessed. One important correction it surfaced: the
   `(scalar_value (identifier|signed_identifier)) @number` pattern
   assumed to be an `inf`/`-inf`-only special case is actually
   unconditional in today's query (Background) — §4 now narrows it
   rather than treating it as pre-existing selective behavior to work
   around.
2. ~~`test/highlight/` file extension/location convention.~~
   **Resolved** — no `test/` directory exists anywhere in the fetched
   upstream source; there's no precedent to match, so this spec uses
   `tree-sitter test`'s own default convention directly (Test plan).
3. ~~`tree-sitter test` availability.~~ **Resolved** — confirmed no
   existing `test/highlight/`/`test/corpus/` wiring either way in the
   fetched source; `tree-sitter test` auto-discovers `test/highlight/*`
   by directory convention alone (no explicit setup step needed) once
   `grammar.js` is present and `tree-sitter generate` has run.
4. **Upstreaming** — whether this repo's own committed
   `queries/highlights.scm` (Background) should also be proposed as a
   PR to upstream `PorterAtGoogle/tree-sitter-textproto` once
   validated, purely for other consumers' benefit. Not decided; purely
   optional — has no bearing on this spec's usability inside
   `oss-prototools`, since we no longer depend on upstream's copy at
   all.
5. ~~Upstream `bindings/rust/lib.rs` is unfilled/broken — fix it when
   landing the fork, or have protolens vendor its own minimal FFI
   shim?~~ **Resolved** — protolens vendors its own shim (§7):
   `colorize.rs` declares `extern "C" { fn tree_sitter_textproto() ->
   Language; }` directly, linking a Nix-built static library; it never
   depends on `bindings/rust` at all. Consistent with this project's
   existing convention (`binding.c`) of not trusting upstream's
   `bindings/` output. (Whether to *also* fix `bindings/rust/lib.rs`
   upstream for other consumers' benefit is now folded into Open Issue
   4 — orthogonal to protolens's own build.)
6. ~~`tree-sitter` crate version — upstream's `Cargo.toml` pins
   `tree-sitter = "~0.20.10"`.~~ **Resolved** — moot: protolens never
   depends on upstream's `Cargo.toml` (Open Issue 5's resolution), so
   that pin doesn't apply. `protolens/Cargo.toml` picks the current
   `tree-sitter` crate release, subject to a one-time ABI-compatibility
   check against `pkgs.tree-sitter`'s CLI-emitted `LANGUAGE_VERSION` at
   implementation time (§7).
7. ~~`RENDER_CACHE_MAX_BYTES` tuning — no data yet on typical
   rendered-subtree size.~~ **Resolved (starting value)** — default to
   the same `1 << 20` as `CANDIDATE_CACHE_MAX_BYTES`, tuned later if
   profiling shows it's wrong for `RenderCache`'s heavier entries; not
   a blocking decision either way.
8. ~~Nix wiring for `queries/highlights.scm` + Rust-linkable parser.~~
   **Resolved** — four derivations (§7's "Nix wiring"):
   `treeSitterTextprotoGenerated` (codegen only, shared — its
   `queries/highlights.scm` now comes from our own committed
   `reproto/tree-sitter-textproto/highlights.scm`, not from
   `treeSitterTextprotoSrc`), the existing `treeSitterTextproto`
   refactored to consume it, a new `treeSitterTextprotoRustLib` (static
   lib + `queries/highlights.scm` copy) exposed to Crane via
   `nix/rust.nix`'s `commonArgs.env` and to manual builds via
   `nix/shells.nix`'s `_hook_rust`, and a new
   `treeSitterTextprotoHighlightTest` (`pkgs.runCommand` check running
   `tree-sitter generate && tree-sitter test` against our committed
   `highlights.scm`/test file) added to the `ci`/`ci-no-clippy`
   `linkFarmFromDrvs` lists and exposed at the top level as
   `tree-sitter-textproto-highlight-test`.
9. ~~Theme selection mechanism — how the user picks
   `ThemeKind::Dark`/`Light`.~~ **Resolved** — a `--theme
   dark|light|system` CLI flag (mirroring protolens's existing
   `--type` flag shape), defaulting to `system`. `system` resolves to
   `Dark`/`Light` once at startup via the `terminal-light` crate
   (`COLORFGBG` env var, then an OSC 11 background-color query with a
   bounded timeout, then `Dark` if inconclusive — §9). Three fixed
   choices behind one flag doesn't contradict Non-goals' "no
   pluggable/config-file theme system."
10. **Palette table values** (§9) are a starting suggestion, not
    validated against an actual terminal in this sandbox (no
    interactive terminal available here) — confirm visually once
    implemented, adjust as needed. Genuinely not resolvable without an
    interactive terminal; the only remaining open item besides #4.

---

## Files changed (anticipated)

| File | Change |
|---|---|
| `reproto/tree-sitter-textproto/highlights.scm` (new, committed) | Capture patterns per §1–§6 above, hand-authored in-repo (Background) — starting point is upstream's minimal 7-pattern/5-capture file, but this file is now this repo's own, not a patch against `treeSitterTextprotoSrc` |
| `reproto/tree-sitter-textproto/test/highlight/textproto.txt` (new, committed) | New highlight-assertion test file per Test plan |
| `default.nix` | New `treeSitterTextprotoGenerated` (codegen-only, shared; assembles the fetched `grammar.js` with our committed `highlights.scm`), `treeSitterTextprotoRustLib` (static lib + `queries/highlights.scm`), and `treeSitterTextprotoHighlightTest` (`tree-sitter generate && tree-sitter test` check) derivations; existing `treeSitterTextproto` refactored to consume `treeSitterTextprotoGenerated` instead of re-running `tree-sitter generate`; `treeSitterTextprotoHighlightTest` added to `ci`/`ci-no-clippy` and the top-level output attrset — all reuse the existing `treeSitterTextprotoSrc` pin, no new hash (§7) |
| `nix/rust.nix` | `commonArgs.env` gains `TREE_SITTER_TEXTPROTO_LIB_DIR`/`TREE_SITTER_TEXTPROTO_QUERIES_DIR` (§7) |
| `nix/shells.nix` | `_hook_rust` exports both vars for manual `cargo build -p protolens` (§7) |
| `protolens/Cargo.toml` | New `tree-sitter`, `tree-sitter-highlight`, and `terminal-light` dependencies (no `cc` crate needed — C compilation happens in Nix, not `build.rs`) |
| `protolens/build.rs` (new) | Emits `cargo:rustc-link-search`/`cargo:rustc-link-lib` from the Nix-supplied env vars; forwards the queries dir via `cargo:rustc-env` — no compilation, no `tree-sitter generate` (§7) |
| `protolens/src/colorize.rs` (new) | `SyntaxRole`, `StyleHint`, `extern "C" fn tree_sitter_textproto()` shim, query execution (§7) |
| `protolens/src/render_cache.rs` (new, or folded into `override_pane.rs`) | `RenderCache` (§8) |
| `protolens/src/theme.rs` (new) | `ThemeKind`, `style_for` (§9) |
| `protolens/src/tui.rs` | `apply_override` consults/populates `RenderCache` instead of calling `decode_and_render_indexed` unconditionally; main-pane rendering applies `StyleHint`s via the active theme; new `--theme dark|light|system` CLI flag, `system` resolved once at startup (§9) |
