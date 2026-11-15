<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0121 — tree-sitter-textproto: vendor `grammar.js`, add `field_no`

Status: implemented
Implemented in: 2026-07-14
Refs: docs/specs/0065-textproto-structural-scanner.md (original "fetch,
      don't commit" `grammar.js` sourcing strategy, now superseded here),
      docs/specs/0116-tree-sitter-textproto-highlight-captures.md
      (`colorize.rs`/`RenderCache`, Non-goals superseded here),
      docs/specs/0120-protolens-any-messageset-as-auto-overrides.md
      (Any/MessageSet auto-override recursion, the trigger for the bug
      below)
App: tree-sitter-textproto, protolens

## Background

Interactive testing against a real document (`Request`'s
`request_extensions` MessageSet field, nested inside an auto-expanded
`Any`, spec 0120) surfaced a syntax-highlighting bug: two rendered
sibling lines — `payload_display_when_private: true #@ bool = 26` and
`package_name: "proxy" #@ string = 31` — lost all field-name/
value coloring, while their trailing `#@` comments (lexed as `extras`,
independent of parser state) stayed correctly colored.

**Root cause**, confirmed by direct reproduction against the real
document (a synthetic standalone repro did not reproduce it — the bug
needs the real auto-expansion cascade): `colorize()`
(`protolens/src/colorize.rs`) runs once per node, at the moment
`splice_override` (`protolens/src/tui.rs`) splices that node's
freshly-rendered text into the document. When an `Any`'s `value` field
auto-expands to `Request` (spec 0120), `Request`'s own
`request_extensions` MessageSet field is, at that exact instant, still
raw/unpromoted — rendered as a bare decimal field name (`1 { ... }`).
`tree-sitter-textproto`'s grammar has no `field_name` alternative for a
bare integer (only `identifier`/`extension_name`/`any_name` — see
`grammar.js`'s `field_name` rule, quoted below), so this token forces
the parser into its cost-based error-recovery mode. That recovery
absorbs the next couple of syntactically-valid sibling fields
(`payload_display_when_private`, `package_name`) into the same `ERROR`
node before resyncing — nothing for `highlights.scm`'s captures to
match there, hence no color (not *wrong* color).

This corrupted `colorize()` result gets cached for the whole node's
range (`RenderCache`, spec 0116 §8/§10). Later, when the MessageSet
`Item` children get their own independent auto-promotion splices, only
*their own* line ranges are refreshed — the corrupted sibling lines
are never revisited, since nothing tells the cache their previously-
sealed content depended on now-stale sibling state.

**Rejected fix directions** (design discussion preceding this spec):

- **Defer/reseal a node's cached render until its own
  auto-expansion recursion is complete.** Rejected: this would make a
  cached `(range, type)` render's correctness depend on *when* it was
  computed relative to other nodes' recursion — violating an important
  existing property (confirmed, not new to this spec) that
  `RenderCache`'s `(payload_range, type, field_name)` key is meant to
  be a pure function of its inputs alone, independent of any override
  state or splice-ordering elsewhere in the tree. `RenderCache`'s own
  doc comment already states this: "no invalidation beyond ordinary
  MRU eviction needed, since a cached entry's key is tied to immutable
  input." A defer/reseal fix would break that invariant, not just work
  around a bug.
- **Bound tree-sitter's error-recovery blast radius some other way.**
  Investigated: tree-sitter's error recovery is a heuristic, cost-based
  resynchronization algorithm with no exposed API to constrain how far
  it can absorb neighboring, individually well-formed tokens. No fix
  available at this layer.

**Chosen fix**: extend the grammar so a bare decimal field number never
triggers error-recovery mode in the first place — `field_name` gains a
new alternative accepting the exact decimal-integer syntax protolens's
own rendering convention already uses for an unresolved/unknown field
(see `prototext-core`'s field-by-number rendering, referenced by this
repo's own `#@ ... =` trailing-comment convention above). This is safe
specifically *because* `tree-sitter-textproto` here only ever parses
protolens's own rendered output (`colorize.rs`'s `colorize()`, spec
0116 §7) — never arbitrary user-authored textproto — so there is no
risk of the grammar becoming silently more permissive than intended
for some other, real-world input.

## Goals

- G1: `reproto/tree-sitter-textproto/grammar.js` is vendored — committed
  in-repo at `reproto/tree-sitter-textproto/grammar.js` — instead of
  fetched from upstream at Nix build time, reversing spec 0065's
  original "fetch, don't commit" decision for this one file (superseded
  here; see spec 0116's Non-goals, now annotated). This follows the
  precedent already set for `highlights.scm`/`binding.c` (spec 0116
  Background): a small, now-locally-owned file, not a large
  machine-generated one (`src/parser.c` remains fetched-and-generated,
  unchanged).
- G2: a new `field_no` rule is added to the vendored grammar — cloned
  from `dec_int`'s own definition (`"0" | /[1-9][0-9]*/`), not a shared
  reference to `dec_int` itself, since `field_name` and `number` occupy
  different structural positions in the grammar and reusing one rule in
  both risks an LR table conflict `tree-sitter generate` would need to
  resolve via an explicit `conflicts: $ => [...]` declaration. Cloning
  avoids that question entirely.
- G3: `field_name`'s `choice(...)` gains `$.field_no` as a new
  alternative, alongside the existing `extension_name`/`any_name`/
  `identifier`. No `highlights.scm` change is needed: its existing
  `(field_name) @attribute` pattern matches the `field_name` node type
  itself, not `identifier` specifically, so a `field_no`-based
  `field_name` inherits `@attribute` coloring automatically.
- G4: `default.nix`'s tree-sitter-textproto Nix pipeline
  (`treeSitterTextprotoGenerated`, `treeSitterTextprotoHighlightTest`)
  reads `grammar.js` from the new committed file instead of the
  `treeSitterTextprotoSrc` `fetchzip`, which is removed entirely (its
  sole purpose — sourcing `grammar.js` — is now served by the vendored
  copy).
- G5: `reproto/tree-sitter-textproto/grammar.js` is REUSE-annotated per
  its upstream ISC licence (`Copyright 2024 Porter Matteo Haet`), via a
  `REUSE.toml` `[[annotations]]` block (not `reuse annotate` directly —
  per this repo's convention for third-party-sourced files, even when
  locally modified).
- G6: the original reported symptom (`payload_display_when_private`/
  `package_name` losing coloring after `Request` auto-expands
  inside an `Any`) is confirmed fixed against the real repro document.

## Non-goals

- No change to `queries/highlights.scm` — G3 confirms the existing
  `(field_name) @attribute` pattern already covers the new alternative
  with no query edit.
- No change to `RenderCache`'s key shape, eviction strategy, or the
  splice/recursion order in `render_overrides`/`splice_override`
  (`protolens/src/tui.rs`) — this spec fixes the render *function*
  (the grammar) so it never produces corruptible intermediate output,
  rather than changing when/how renders are cached or sequenced (see
  Background's "Rejected fix directions").
- No change to `src/parser.c` sourcing — it remains machine-generated
  at Nix build time from the (now-vendored) `grammar.js`, never
  committed, unchanged from spec 0065/0116.
- No attempt to make the grammar accept arbitrary invalid textproto
  more broadly — `field_no` is scoped narrowly to exactly protolens's
  own bare-decimal-field-name rendering convention, not a general
  error-tolerance change.

## Specification

### `grammar.js` diff (relative to upstream pinned commit
`568471b80fd8793d37ed01865d8c2208a9fefd1b`)

```js
field_name: $ => choice(
  $.extension_name,
  $.any_name,
  $.identifier,
  $.field_no,       // new
),

// ... (unchanged rules elided) ...

field_no: $ => choice(   // new rule, cloned from dec_int below
  "0",
  /[1-9][0-9]*/,
),
```

`dec_int` itself (used by `number`) is untouched, byte-for-byte
identical to upstream.

### Nix wiring

`default.nix`: `treeSitterTextprotoSrc` (the pinned `fetchzip`) is
removed. `treeSitterTextprotoGenerated`'s `buildPhase` drops its
`cp ${treeSitterTextprotoSrc}/grammar.js .` step — `grammar.js` is
already present via its `src = ./reproto/tree-sitter-textproto`.
`treeSitterTextprotoHighlightTest` replaces the same `cp` line with
`cp ${./reproto/tree-sitter-textproto/grammar.js} grammar.js`.

### REUSE

`REUSE.toml` gains an `[[annotations]]` block for
`reproto/tree-sitter-textproto/grammar.js`:
`SPDX-FileCopyrightText = "Porter Matteo Haet"`,
`SPDX-License-Identifier = "ISC"` — same pattern already used for
`nixpkgs/**` and `workspace-hack/**`.

## Test plan

1. `tree-sitter generate` against the vendored grammar succeeds with no
   LR conflict warning (confirms G2's clone-not-share choice avoided a
   conflict).
2. `treeSitterTextprotoHighlightTest` (`tree-sitter test`) still passes
   against the existing committed
   `test/highlight/textproto.txt` fixture — no regression on any
   existing capture.
3. `colorize.rs`'s existing unit tests still pass unchanged.
4. A new `colorize.rs` unit test: a snippet containing a bare-decimal
   `field_name` (e.g. `"1 { a: 1 }\n"`) parses with `(field_name)
   @attribute` firing on the `1` token, and — the actual regression
   case — a syntactically-valid sibling field *after* such a snippet
   keeps its own coloring rather than falling into a corrupted `ERROR`
   region.
5. Rebuild protolens; re-run the original repro (`--descriptor-set
   /tmp/pdb.desc /tmp/anu`, navigating to the `Request`-typed `Any`
   under the `request_extensions` MessageSet) and confirm
   `payload_display_when_private`/`package_name` render with correct
   coloring (G6).
6. `reuse lint` passes.
