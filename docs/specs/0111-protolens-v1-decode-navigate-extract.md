<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0111 — `protolens` v1: decode / navigate / extract

Status: implemented
Implemented in: 2026-07-10
Refs: `docs/specs/0109-protolens-interactive-schema-inference.md`,
`docs/specs/0110-render-sink-unification.md`,
`docs/specs/0113-protolens-tui-refinements.md`,
`docs/specs/0114-protolens-range-type-override.md`
App: protolens (new)

---

## Background

Spec 0109 laid out `protolens`'s full envisioned scope (interactive TUI,
per-node and per-field-number type override, candidate ranking via
`score_all`, unlimited undo, save-project/export-`.desc`). This spec narrows
to the first concretely buildable slice — **decode, navigate, extract a
node's raw bytes** — deferring everything else in spec 0109's design to
future work.

This slice depends on spec 0110's `IndexingTextSink` (a byte-offset index
pairing each rendered node with its raw byte range in the source protobuf,
its byte range in the rendered text, and its indentation level) and on
`decode_and_render`'s new `initial_level`/`emit_header` parameters. Spec 0110
is a hard prerequisite: this spec cannot start implementation until it lands.

Beyond the v1 slice this spec commits to implementing, it also captures a
more concrete design sketch for what spec 0109 left open — type overriding,
rendering/styling, and type-definition assistance — so v1's architecture
doesn't paint us into a corner (see "Beyond v1" and "Phasing" below). Spec
0109 itself is left untouched for now; it will be revisited once v1 has
shipped and the sketch below has been validated against real use.

---

## Goals

1. New Rust binary crate `protolens`, workspace member, depending on
   `prototext-core` in-process (no subprocess/FFI), per spec 0109's
   high-level design.
2. **Decode**: given a byte blob, determine its root `MessageDescriptor` and
   render it via `IndexingTextSink`, producing both the rendered text and
   its offset index.
   - **Root-type determination, by default**: v1 reuses `prototext-core`
     tooling's own determination logic — the same `prototext-graph::score`
     engine that backs the `prototext` CLI's `list-schemas` command
     (`score_all(pb, graph, opts) -> Vec<EntryScore>`, corpus-wide
     plausibility ranking against a `--descriptor-set`'s compiled
     `hopcroft.rkyv` index). `protolens` links `prototext-graph` in-process
     (already listed in §1's crate dependencies) — no subprocess, no
     forking the `prototext` binary. If scoring yields a confident top
     candidate, that's the root type; if it can't determine one (no
     `--descriptor-set` given, or no candidate scores well), v1 requires
     the user to pass `--type` explicitly and errors out otherwise.
   - **Explicit `--type` always wins**: if the user passes `--type <FQDN>`,
     it's used verbatim, bypassing determination entirely.
3. **Navigate**: `ratatui` + `crossterm` TUI (rationale for this choice vs.
   alternatives: Annex A). Arrow/`hjkl` cursor movement between index
   entries' start lines. Fold/unfold a subtree using its index-derived
   `[start_line, end_line]` span (folding collapses the range to a single
   summary line; per spec 0109, fold state is pure UI state, not undoable).
   Full key-binding proposal, including sibling-skip movement, parent
   navigation, and a navigation-history ("jumplist") design: Annex B.
   Screen layout — panes, gutter, status/command lines, and how the layout
   grows across later phases: Annex C.
4. **Extract**: for the node under the cursor, use its `raw_range` to obtain
   the corresponding byte sub-slice of the source blob, and expose it (write
   to a file, or print to a status/output area) — enough for the user to
   independently feed it to another tool (e.g. `prototext decode --type X`)
   to test a hypothesis about its type. `protolens` itself does not re-decode
   or re-splice anything in v1 (see Non-goals).

## Non-goals

- Per-node type override UI, candidate-ranking *for override purposes*,
  unlimited undo, save-project/export-`.desc` — all remain future work per
  spec 0109's fuller design (its goals 3–7 are unimplemented after this
  spec). (Root-type determination via `score_all` is, however, part of v1 —
  see Goal 2; what's deferred is *overriding* individual nodes' types
  after the initial decode.)
- In-place splice-and-replace of a node's rendering after a manual re-decode
  — even though spec 0110's initial-level parameter and `IndexingTextSink`'s
  index shape are deliberately designed to support this later, v1 stops at
  extracting raw bytes. Splicing a re-rendered replacement into the composite
  view and re-indexing the replaced subtree is deferred to the spec that
  introduces interactive override (spec 0114).
- Any change to `prototext-core`/`render_text` — spec 0110 is a fixed
  prerequisite, not something this spec revises.

See "Beyond v1" below for the design sketch these non-goals defer to —
captured now to avoid architectural dead ends, not committed for v1
implementation.

---

## Specification

### §1 — `protolens` crate structure (v1)

New workspace member, binary crate:

```
protolens/
  Cargo.toml           — depends on prototext-core, prototext-graph, ratatui, crossterm
  src/
    main.rs             — CLI entry: blob path, --descriptor-set, --type <FQDN> (§3)
    decode.rs            — load blob + descriptor-set; if --type absent, call
                            prototext-graph::score_all to determine the root
                            type; call decode_and_render with an
                            IndexingTextSink, then build the navigation tree
                            (below) from its Vec<NodeSpan>; hand back
                            (text, Vec<TreeNode>)
    tui.rs                — ratatui app: render text pane, cursor/fold state, key handling
    extract.rs             — "extract" action: NodeSpan.raw_range -> byte slice -> file/stdout
```

No project-file format, no undo stack, no override picker in v1 — those land
with the spec that introduces interactive override (per Non-goals).

### Tree construction (ingestion)

`IndexingTextSink` hands back `Vec<NodeSpan>` in emission order (pre-order,
document order), each entry's `level` recording its nesting depth — but no
parent/child/sibling links (spec 0110's `NodeSpan` doesn't carry them, and
shouldn't: this is `protolens`-specific navigation structure, not a
render-time concern). Resolves Open Issue 3 below.

`decode.rs` builds a local arena immediately after decoding:

```rust
struct TreeNode {
    span: NodeSpan,
    parent: Option<usize>,
    first_child: Option<usize>,
    last_child: Option<usize>,
    next_sibling: Option<usize>,
    prev_sibling: Option<usize>,
}
// Vec<TreeNode>, index-parallel to the emitted Vec<NodeSpan> — index i IS
// the i-th emitted node, so document-order move (j/k) stays free array-index
// arithmetic; only parent/sibling/child need explicit links.
```

Construction is a single `O(n)` pass over the flat, level-annotated vec with
a small stack of "currently open ancestors": pop until the stack top's level
is one less than the incoming node's — that's the parent; splice the node
into the parent's child list (`first_child`/`last_child` +
`prev_sibling`/`next_sibling`); push; continue. Standard "rebuild a tree
from flattened pre-order + depth" construction (the same idea as an
`indextree`-style arena) — no bespoke research needed.

Why sibling *links* rather than a `children: Vec<usize>` per node: `O(1)`
sibling-skip (`J`/`K`, Annex B) and child-jump (`l`) without rescanning or a
per-parent heap allocation — one flat backing array. `h` (parent move) is
`O(1)` too.

Deliberately `protolens`-local, not part of `NodeSpan`/spec 0110: future
navigation needs (e.g. a cached subtree byte-size, a "next foldable
ancestor" shortcut) are just new `TreeNode` fields, no upstream change
required.

### §2 — v1 TUI interaction

v1's screen is a single scrollable pane plus a 1-line header, status line,
and command/message line (no split panes, no modal overlays — those are
later phases; full layout, sizing, and gutter design: Annex C).

- Cursor movement, fold/unfold, parent/sibling navigation, and navigation
  history: exact key bindings proposed in Annex B; semantics summarized
  here:
  - **Document-order move** (next/previous `NodeSpan` by `raw_range` start):
    default up/down.
  - **Sibling-skip move** (next/previous sibling, skipping the current
    node's children): distinct binding from document-order move.
  - **Parent move**: jump to the cursor's containing `NodeSpan`.
  - **Fold/unfold**: collapses/expands a subtree using its
    `text_range`-derived `[start_line, end_line]` — pure UI state, not part
    of the index/undo model (matches spec 0109 §"Navigation").
  - **Navigation history**: a jumplist (not a data-undo stack) recording
    "big" cursor jumps (parent/sibling-skip, not every single-line move) so
    the user can pop back to where they came from — see Annex B for the
    design rationale.
- An extract key/command (exact binding TBD during implementation, e.g. `x`
  or `:extract <path>`): writes the node under the cursor to a
  user-specified file (or a status area for short spans) in one of two
  formats — see Open Issue 2 (now resolved): plain binary, or `#@ prototext`
  text.

### §3 — CLI flags and `--help` text

v1's flags mirror `prototext`'s own, since `protolens` reuses the same
determination/decode machinery:

- `--descriptor-set <path>`: schema corpus for root-type determination and
  for resolving any type the user references. Matches `prototext`'s own
  flag of the same name — singular, not repeatable (verified against
  `prototext`'s `Cli::descriptor: Option<PathBuf>`); requires the corpus's
  sibling `hopcroft.rkyv` for determination to run (same requirement as
  `prototext list-schemas`). Required in v1: unlike `prototext`, there is
  no embedded-WKT-descriptor fallback, so its absence is a hard error even
  if `--type` is given explicitly.
- `--type <FQDN>`: explicit root type. Optional — if omitted, v1 attempts
  determination via `--descriptor-set` (Goal 2); if that fails (no
  `--descriptor-set`, or no confident candidate), `protolens` exits with an
  error asking for `--type` explicitly.
- `<blob>`: positional path to the binary protobuf to decode.

Sketch of the `--help` text v1 should ship, for illustration (exact wording
TBD at implementation time):

```
protolens — decode, navigate, and extract raw bytes from a binary protobuf

USAGE:
    protolens --descriptor-set <path> [--type <FQDN>] <blob>

OPTIONS:
    --descriptor-set <path>   FileDescriptorSet for root-type determination
                               and type resolution (required)
    --type <FQDN>             Root message type; if omitted, determined
                               automatically from --descriptor-set
    -h, --help                Print help
```

**Ideas for subsequent versions** (not part of v1's `--help`, noted here so
the CLI surface doesn't need a breaking redesign later — see Phasing):

- A project-file flag (`--project <path>`) once save/export lands (Phase 7).
- An override flag/subcommand to seed non-default type overrides from the
  command line, without going through the TUI picker (Phase 3+).
- A `--no-expand-any`-style flag mirroring `prototext`'s own, once Any/
  MessageSet default-overriding candidate scoring needs the same escape
  hatch `score_all`'s `ScoringOpts` already exposes.

### §4 — Testing approach

Nothing here needs an interactive terminal or a scripted end-to-end
harness — `ratatui`'s own recommended pattern (separate app state from
drawing) is enough:

- **Pure-logic unit tests**: `TreeNode` arena construction (§1 — given a
  synthetic `Vec<NodeSpan>`, assert correct `parent`/`first_child`/
  `next_sibling` links), root-type determination, `extract.rs`'s byte-slice
  and dedent transform, jumplist push/pop. Plain `#[test]`, deterministic,
  no `ratatui`/`crossterm` involved.
- **`App` state-machine tests**: `tui.rs` exposes an `App` with a
  `handle_key(&mut self, key: KeyEvent)` method that owns all cursor/fold/
  scroll/jumplist state, kept separate from the actual `frame.render_widget`
  calls. Tests construct an `App` over a fixture tree, feed a scripted
  `KeyEvent` sequence, and assert on the resulting state — no terminal
  involved.
- **Interaction + rendering tests, combined**: `ratatui::backend::TestBackend`
  (built into `ratatui`, mature — an in-memory `Buffer` in place of a real
  terminal). Run a scripted `KeyEvent` sequence through `App::handle_key`,
  then `terminal.draw(|f| app.render(f))` against a `TestBackend`, then
  assert on the buffer's cell contents (or via a snapshot crate like
  `insta`). This combination — hand-rolled `App` + built-in `TestBackend` —
  *is* the standard way to exercise a full interaction sequence end-to-end;
  no PTY or real terminal process is needed. (Considered and rejected for
  this: `ratatui-testlib`, a PTY-based `send_key()`/`wait_for()` harness —
  too immature to depend on: v0.1.0, single maintainer, "implementation
  starting" per its own docs, 63 downloads/month.)
- **Fixture-based integration tests**: reuse `prototext-core/fixtures/
  descriptor.pb` (already in-repo, already backing the `codec` bench) as
  real input — decode it, assert tree/`NodeSpan` counts, walk known
  navigation sequences, extract known ranges and diff the resulting bytes.
- **Deliberately not tested**: real `crossterm` terminal I/O (raw-mode
  entry, actual key events from a TTY) — kept to a thin
  `crossterm::event::read()` loop translating into the same `KeyEvent`/
  action type used above, small enough to be obviously correct by
  inspection rather than needing its own test harness.
- **Wiring**: no separate CI step needed — the workspace's `buildRustPackage`
  derivation (`nixpkgs/pkgs/by-name/pr/prototools/package.nix`) already runs
  `cargo test` for the whole workspace, so tests under `protolens/` are
  picked up automatically.
- **Beyond v1**: a saved-project file (Phase 7) would additionally enable
  fully headless, golden-file end-to-end tests exercising the real
  override/render pipeline, not just cursor bookkeeping — see "Project file
  and batch mode" under "Beyond v1" below.

---

## Beyond v1 — design notes for later phases

Not part of v1's Goals/Specification above — captured here so later phases
build on a coherent architecture instead of retrofitting one. Nothing in
this section is committed for implementation by this spec.

### Type overriding (data model)

- **Overriding record**: `(range, type_fqdn, descriptor_set)`. Overriding
  only ever targets a LENDEL at wire level — the only wire type with an
  ambiguous, overridable interpretation. `range` is the raw byte range in
  the source blob (matches `NodeSpan::raw_range`, spec 0110 §3);
  `descriptor_set` identifies where `type_fqdn` is resolved from (so an
  override can pull a type from a different loaded corpus than the one used
  for the initial guess).
- **Collection, indexed two ways**: by `range` (pre-ordered — used during
  the render pass: "does this LENDEL have an overriding?"), and by
  `type_fqdn` (pre-ordered — a UI convenience: "show every range this type
  currently applies to").
- **At most one active overriding per range**: several overridings can exist
  for the same range (alternative hypotheses the user tried), but only one
  is `active` at a time. Rendering only ever consults the active one, if
  any.
- **Default overridings**, seeded when a blob is opened:
  - One at the root (the whole-blob range), for whatever type v1's
    determination step landed on (`score_all` result, or the explicit
    `--type` override — see Goal 2).
  - One per `Any`/`MessageSet` wrapper node, wherever `type_url`-based
    auto-expansion (spec 0089/0100) already picks a type today. **No
    special-casing needed**: this is the same mechanism as the root
    default — the type comes from the payload (`type_url`) instead of a
    user choice, but it's stored and overridable identically to any other
    range. Overriding one of these ranges just means forcing an explicit
    `type_fqdn` for that payload instead of trusting `type_url`.
- **Rendering as recursive substitution**: starting from the root, for
  every LENDEL in `IndexingTextSink`'s top-level structure, look up an
  active overriding exactly matching that range and substitute its type if
  present, then recurse into the substituted (or default) rendering's own
  LENDEL children. This converges to the full displayable tree.
- **Undo, deferred**: the active/inactive toggle is itself a lightweight
  approximation of undo — reverting one range's decision is "reactivate
  whichever overriding was active before." Not equivalent to spec 0109 Goal
  5's full multi-step, cross-range undo/redo (persistent-tree /
  path-copying), but may be sufficient for several phases before that's
  needed — see Phasing.
- **Open**: incremental re-interpretation (re-render only the subtree(s)
  actually affected by an active-overriding change, not the whole document)
  needs its own detailed design once this lands — flagged, not solved here.

### Project file and batch mode

- **Saved project as a plain config file** (Phase 7's "save-project"): a
  YAML file listing the overriding collection (§"Type overriding" above —
  both active and inactive entries, so re-opening a project restores the
  exact same alternative-hypothesis history, not just the current choice)
  plus a handful of session globals — the blob path, the `--descriptor-set`
  path(s), and, once it exists, a `.proto`-source-files path (Phase 5/6).
  Human-readable and diffable by design (a real advantage over a binary
  `.desc` export for this purpose) — a code reviewer can see exactly which
  ranges were reinterpreted and how, in a plain text diff.
- **Batch/headless mode**: given a project file, `protolens` doesn't
  strictly need the TUI at all to reproduce a decode — `--project
  <path> extract [--type-override-only] <output-format>`-style invocation
  applies the saved overriding collection to the saved blob and writes the
  result (binary or `#@ prototext`), with no terminal/`ratatui` involved.
  This only works cleanly if the override-application/rendering pipeline
  (Beyond v1's recursive-substitution model) is already factored out from
  `tui.rs` as its own library-level entry point, independent of any
  interactive state — a discipline worth keeping from the start (mirrors
  §4's `App`-vs-rendering split) rather than retrofitting later.
- **Why this matters for testing**: batch mode turns into `protolens`'s
  strongest end-to-end test shape — fully deterministic input (blob +
  descriptor-set + explicit saved overrides) to deterministic output
  (extracted bytes/text), comparable against a golden file, with zero
  interactive/terminal surface. Exercises the real override/render
  pipeline, not just cursor bookkeeping (contrast with §4's `App`+
  `TestBackend` tests, which cover interaction but not override
  application). Not usable until Phase 7 lands the project-file format
  itself — noted here for architecture, not committed for v1.

### Rendering and styling

- **Copy with relative indentation**: the on-screen tree is indented per
  nesting depth. Copying a selected subtree to compose a standalone
  `#@ prototext` snippet in an external editor shouldn't carry that
  subtree's absolute indentation along — it's irrelevant noise relative to
  a hand-composed file. On copy (not on render), compute the minimum
  leading-whitespace width across the selected lines and strip exactly that
  many columns from every line — the least-indented selected line lands at
  column 0, deeper lines keep their relative indentation to each other
  (same transform as Python's `textwrap.dedent()`). The on-screen view
  itself is untouched; only the clipboard content is dedented.
- **Syntax coloring, short term (textual)**: checked against spec 0110's
  actual `NodeSpan` shape — `text_range` is a **line-number range**, not a
  sub-line/column/token span. `IndexingTextSink` does not currently carry
  the per-token granularity a structural colorer would need, so coloring
  driven directly from the index isn't available in v1's index shape as-is
  (corrects the earlier draft's assumption here). Short-term plan instead:
  run a generic, off-the-shelf syntax-highlighting library over each line
  of `TextSink`'s rendered text (protobuf-text-like grammar: `#`-comments,
  `field_name: value` / `field_name { ... }` — close enough to an existing
  grammar, e.g. YAML- or JSON-ish highlighters, that a generic library
  should handle it acceptably without a bespoke lexer) and feed the colored
  result into `ratatui` instead of the plain text. This is a textual,
  re-lexing approach — no `NodeSpan` changes required. Like tree construction
  above, colorization runs once at ingestion — producing a styled line
  buffer alongside the plain one — not re-run per frame; the main pane then
  windows into whichever buffer (plain or colored) is active (windowed-
  rendering note, Annex C).
- **Syntax coloring, longer term (structural, deferred)**: if the
  textual approach proves insufficient (e.g. it can't reliably distinguish
  overridden nodes or malformed values from well-typed ones), extend
  `IndexingTextSink`/`NodeSpan` with sub-line (column) spans per token —
  it already knows exactly where each token sits when it writes it, this
  just isn't surfaced in the index today. Deferred: not needed unless the
  short-term approach turns out to be inadequate. Extends Open Issue #3
  below (`NodeSpan` shape).
- **Opportunity, once structural coloring exists — malformity/invalid-
  serialization styling**: give a distinct style to nodes/fields the render
  pass already flags as malformed or type-mismatched (`ProbeSink`'s
  existing malformity tracking, `TYPE_MISMATCH` annotations, spec
  0097/0110) — surfacing "this doesn't parse as its current type" visually,
  not just in the annotation text. Reuse the same styling channel for "an
  active overriding impacts this node." Depends on the structural coloring
  path above landing first (or on a textual heuristic keyed off the
  existing `TYPE_MISMATCH` annotation text, if that proves good enough
  sooner).

### Type definition assistance

- **Separate pane, current node's type**: show the `.proto` (or compiled
  textproto) definition of the currently-selected/candidate FQDN, to help
  the user judge candidates or an already-chosen type.
- **Locating a declaration inside a decompiled `.proto` file, and
  navigating imports**: `SourceCodeInfo` on the `FileDescriptorProto` would
  give exact declaration spans, including for imported types, "for free."
  Today's pipeline gets *stripped* `FileDescriptorProto`s and decompiles
  them via `reproto` — recompiling with `protoc --include_source_info` to
  obtain `SourceCodeInfo` would be a heavier, separate step (a tolerable
  one-time cost, but with a nonzero, if unlikely, risk of the recompiled
  descriptor diverging from the originally captured bytes). **Preferred
  alternative**: have `reproto` synthesize `SourceCodeInfo.Location` entries
  itself while decompiling, since it already knows exactly where each
  type/field it emits lands in the `.proto` text it's writing — no `protoc`
  recompilation involved at all. This is a `reproto`-side feature, not a
  `protolens`-side one; needs its own design pass in `reproto`'s spec
  lineage, not decided here.
- **Deep dives**: for anything beyond the summary pane, shell out to
  `$EDITOR` at the located `file:line`, suspending the TUI and resuming on
  return — the standard pattern (`git commit`, `fzf`, etc.). No
  embedded-vim-as-library approach: rejected, no mature Rust crate for this.
- **Future, separate project**: a dedicated `.proto` navigator
  (reverse-engineering/auditing-oriented — cross-references,
  go-to-definition, import graphs) is an interesting idea on its own,
  decoupled from `protolens`. If built as a library, `protolens` could
  integrate it later for this pane instead of a bespoke minimal viewer.
  Noted as a related future project, not part of this spec's scope.

---

## Phasing

v1 (this spec's actual Goals/Specification, above) is the only phase
committed to implementation now. The rest is a sequencing sketch — to avoid
dead ends, not a schedule.

| Phase | Scope | Depends on |
|---|---|---|
| **v1** (this spec) | Decode/navigate/extract raw bytes (Goals 1–4) | spec 0110 |
| **Phase 2** | Overriding data model (range + FQDN + descriptor-set, active/inactive); default overridings at root + `Any`/`MessageSet` ranges; full-tree re-render on change. No picker UI, no candidate ranking, no undo beyond active/inactive toggling. | v1's `IndexingTextSink`/`NodeSpan` index |
| **Phase 3** | Override picker UI (modal overlay, Annex C) + `score_all` candidate ranking (spec 0109 Goals 3–4). Exact-position only; field-number-pool bulk override still deferred. | Phase 2's overriding model |
| **Phase 4** | Rendering polish: textual syntax coloring (generic library over rendered lines), copy-with-dedent. | v1's `TextSink` output |
| **Phase 4b** (deferred, conditional) | Structural syntax coloring (incl. malformity/invalid-serialization highlighting), if the textual approach (Phase 4) proves insufficient. | Phase 2 (styling hook) + `NodeSpan` column-span extension |
| **Phase 5** | Type definition assistance pane (split-pane layout, Annex C): `reproto`-side `SourceCodeInfo` synthesis, in-pane viewer, import navigation, `$EDITOR` fallback. | Phase 3 (most useful alongside the candidate picker) + a `reproto`-side spec |
| **Phase 6** (future, likely separate project) | Standalone `.proto` navigator library, later integrated into protolens's Phase 5 pane. | — |
| **Phase 7** (revisit spec 0109) | Field-number-pool bulk override, true multi-step undo/redo, save-project (YAML project file, "Project file and batch mode"), export `.desc`, batch/headless extract mode. | Phases 2–3; spec 0109 itself revisited once these land |

Incremental re-interpretation (re-render only affected subtrees) is a
cross-cutting concern starting at Phase 2 — its own open design task, not
tied to any one phase's ship date.

---

## Open Issues and Challenges

1. **v1's root-type input** — resolved: `--descriptor-set <path>`
   (repeatable) and `--type <FQDN>` (optional override), matching
   `prototext`'s own flags. No startup schema-picker UI in v1. See §3 for
   the full flag/`--help` treatment, including a place for subsequent-
   version flag ideas.
2. **Extract action's output format** — resolved: two formats, both
   supported — (1) plain binary (raw `raw_range` byte slice), and (2)
   `#@ prototext` text (the node's rendering, sliced from `TextSink`'s
   output via `text_range`). For the `#@ prototext` format, a `text_range`
   slice carries its on-screen absolute indentation; extraction must apply
   the same dedent transform as the copy-paste feature ("Copy with relative
   indentation" above) so the extracted snippet is self-contained and
   directly usable in an external editor, not just the interactive-copy
   path.
3. **`NodeSpan` shape** — resolved: `NodeSpan` itself (spec 0110) stays as
   emitted (`raw_range`, `text_range`, `level`, `type_fqdn`), no upstream
   change. `protolens` builds its own local arena (`TreeNode`: parent +
   doubly-linked siblings + first/last child) on ingestion — see "Tree
   construction (ingestion)" in §1. If Phase 4b's structural coloring is
   pursued later, sub-line column spans per token would be an additional
   requirement on `NodeSpan` itself — separate from this arena, revisit
   then.
4. **Incremental re-interpretation**: re-rendering only the subtree(s)
   affected by an active-overriding change, not the whole document (Phase
   2, "Beyond v1") — no detailed design yet.
5. **`reproto`-side `SourceCodeInfo` synthesis** (Phase 5, "Beyond v1"):
   tracked in its own spec, `docs/specs/0112-reproto-source-code-info-synthesis.md`
   — feasibility of having `reproto` synthesize `SourceCodeInfo.Location`
   entries during decompilation, instead of recompiling with `protoc
   --include_source_info`.
6. **`.proto` navigator** (Phase 6, "Beyond v1"): tracked here only as a
   pointer to a related future project, not owned by this spec or by
   `protolens`'s own roadmap. May belong better under spec 0109's
   higher-level `protolens` vision than here, once 0109 is revisited
   (Phase 7) — left here for now per this spec's "leave 0109 untouched"
   stance (Background).

---

## Annex A — `ratatui` + `crossterm`: rationale

**`crossterm`** — cross-platform terminal backend (raw mode, alternate
screen, key/mouse events, cursor control). Considered against:
- `termion`: Unix-only (no Windows support) — a hard blocker if `protolens`
  should ever run on Windows; `crossterm` has no such restriction.
- Writing directly against a platform's terminal APIs: no cross-platform
  abstraction, far more code for no benefit over an existing, widely-used
  crate.

**`ratatui`** (the maintained fork of the now-archived `tui-rs`) — immediate-
mode widget/layout library built on top of a terminal backend (`crossterm`
here). Considered against:
- `tui-rs`: unmaintained since 2023 (archived); `ratatui` is its actively
  maintained continuation and the de facto standard for new Rust TUI work —
  no reason to pick the archived original.
- `cursive`: a more opinionated, retained-mode widget framework (its own
  event loop, view tree, focus management). Good fit for form-like UIs;
  less natural for `protolens`'s core UI, which is fundamentally "one large
  scrollable/foldable text pane with a cursor" — closer to a text-editor
  widget than a form. `ratatui`'s immediate-mode model (redraw each frame
  from application state) maps more directly onto "index + cursor position
  + fold state" than `cursive`'s view-tree model would.
- `termwiz` (from the `wezterm` project): capable but more niche/less
  documented for general application use outside `wezterm` itself; smaller
  ecosystem and fewer examples to build from.

Why this pair fits `protolens` specifically: the core UI is a single
scrollable text pane (the rendered `#@ prototext`) plus a cursor and,
later, a secondary pane (type-definition assistance, Phase 5) — exactly
`ratatui`'s sweet spot (`Paragraph`/custom widget with a `Block`, `Layout`
splitting for the secondary pane, `Frame`-driven redraw keyed off
`NodeSpan`-derived state). No retained widget tree or complex focus model
is needed for v1's scope.

## Annex B — key bindings and navigation-history proposal

### Proposed key bindings (v1)

| Key | Action |
|---|---|
| `j` / `Down` | Document-order move: next `NodeSpan` (by `raw_range` start) |
| `k` / `Up` | Document-order move: previous `NodeSpan` |
| `J` / `Shift-Down` | Sibling-skip move: next sibling (skips the current node's children) |
| `K` / `Shift-Up` | Sibling-skip move: previous sibling |
| `h` / `Left` | Parent move: jump to the cursor's containing `NodeSpan` (if the cursor is on a folded node, this doubles as "fold" — `nvim-tree`-style, see below) |
| `l` / `Right` | Child move: jump to the cursor node's first child (mirrors `h`) |
| `z` / `Space` | Toggle fold/unfold on the node under the cursor |
| `Ctrl-O` | Navigation history: jump back to the previous recorded location |
| `Ctrl-I` | Navigation history: jump forward (redo the last `Ctrl-O`) |
| `x` | Extract: write the node under the cursor to a file (binary or `#@ prototext`, per Open Issue 2) |
| `q` | Quit |

Rationale for `h`/`l` doubling as fold/unfold on a folded node: this is the
same convention `nvim-tree` and similar file-tree UIs use — pressing "go to
parent" on an already-collapsed node is a natural synonym for "collapse
further," and avoids needing a second dedicated fold key for that specific
case (the dedicated `z`/`Space` binding remains available for folding an
*expanded* node without first navigating to it).

### Navigation history: not data-undo, a jumplist

This is **not** an extension of spec 0109's data-model undo/redo — folding
and cursor position are pure UI state (per Goal 3/§2), so there's nothing
in the override/data model to undo here. The relevant concept is a
**jumplist**, the same mechanism `vim` uses for `Ctrl-O`/`Ctrl-I`: an
append-only history of "significant" cursor positions, with a movable
pointer into it.

- **What gets recorded**: only "big" jumps — parent move, sibling-skip
  move, an explicit "go to" (e.g. a future search/filter feature) — not
  every single `j`/`k` step. This matches `vim`'s own heuristic and avoids
  the jumplist filling up with noise from routine scrolling.
- **Why this matters for the workflow described**: the motivating case —
  "I went to the parent to check something, now I want to get back to the
  exact child I started from" — is exactly what a jumplist solves: `h`
  (parent move) records the child's position before jumping, so `Ctrl-O`
  returns to it precisely, without needing to manually re-navigate back
  down through siblings.
- **Scope for v1**: a small in-memory `Vec<NodeSpan index>` with a cursor
  pointer is sufficient — no persistence across sessions needed, no
  interaction with fold state beyond "the jumplist target might currently
  be inside a folded subtree" (in which case jumping to it should also
  unfold its ancestors, so the target is actually visible).

## Annex C — screen layout and pane design

### v1 layout (single pane, committed)

```
┌────────────────────────────────────────────────────────────────────┐
│ protolens — descriptor.pb — google.protobuf.FileDescriptorSet       │ ← header (1 line)
├────────────────────────────────────────────────────────────────────┤
│  1 #@ google.protobuf.FileDescriptorSet                             │
│  2 file {                                                           │
│▸ 3   name: "google/protobuf/descriptor.proto"                       │ ← gutter + cursor line
│  4   package: "google.protobuf"                                     │
│▾ 5   message_type {                                                 │ ← fold indicator (expanded)
│  6     name: "FileDescriptorProto"                                  │
│  7     field { ... }                                                │
│  ⋮                                                                   │
├────────────────────────────────────────────────────────────────────┤
│ L5/3062  bytes[212..8841]  google.protobuf.DescriptorProto           │ ← status line (1 line)
├────────────────────────────────────────────────────────────────────┤
│ :                                                                     │ ← command/message line (1 line)
└────────────────────────────────────────────────────────────────────┘
```

Four regions, laid out with `ratatui::Layout` as a single vertical split:

- **Header** (`Constraint::Length(1)`): blob path, root type — static for
  the session.
- **Main pane** (`Constraint::Min(0)`, takes all remaining rows): the
  `TextSink`-rendered `#@ prototext`, scrollable. A **2-column gutter**,
  prepended by the TUI itself (not part of `TextSink`'s output, so it
  never leaks into extracted/copied text) shows, per line: `▸`/`▾` fold
  state on a foldable node's start line, blank otherwise; the cursor row
  is highlighted separately (reverse video, or a leading marker — exact
  styling TBD at implementation time). Keeping fold/cursor chrome in a
  dedicated gutter, distinct from the render's own indentation, is what
  keeps a selected/extracted range self-contained (ties into "Copy with
  relative indentation" above and Open Issue 2's extraction format).
- **Status line** (`Length(1)`): cursor position (`Lx/total`), the cursor
  node's `raw_range`, its `type_fqdn` — "what am I looking at."
- **Command/message line** (`Length(1)`): vim-style — echoes partial
  multi-key sequences (e.g. after a leading `g`), hosts `:extract <path>`-
  style ex-commands (Annex B), and doubles as a transient status/error
  area (extract confirmation, decode errors).

**Windowed, just-in-time rendering**: the main pane must never hand its full
line buffer (plain text, or later, colorized — "Syntax coloring, short
term" above) to `ratatui` each frame. Ingestion produces the complete
buffer once; every redraw instead slices it to exactly the visible range
(`scroll_offset..scroll_offset + pane_height`, derived from the main pane's
`Rect` height) before constructing that frame's widget. This keeps each
frame's work proportional to the terminal's visible rows, not to the
document's total size — the payoff of `ratatui`'s immediate-mode model
(Annex A) is only realized if the app does this slicing itself; feeding the
whole buffer to a `Paragraph` every frame would forgo it.

Minimum viable terminal: roughly 60×15. Below that, drop the header
(folded into the status line) rather than refuse to run.

A `?` help overlay (renders Annex B's key-binding table as a centered
modal) is cheap enough to ship in v1 itself, even though the panes below
it are Phase 3/5 features.

### Later phases — modal overlay and split pane

**Phase 3 (override picker)** — sketch superseded by spec 0114, which
implements this phase as an ephemeral right-hand split pane instead of the
centered modal originally sketched here. Left below for historical
context.

A centered floating/modal overlay, not a permanent pane, so v1's
single-pane layout is otherwise untouched:

```
        ┌─────────────────────────────────────────────┐
        │ Override type — range [212..8841]             │
        ├─────────────────────────────────────────────┤
        │ > google.protobuf.DescriptorProto     0.98    │
        │   google.protobuf.FieldDescriptorProto 0.41   │
        │   google.protobuf.EnumDescriptorProto  0.12   │
        ├─────────────────────────────────────────────┤
        │ Enter: apply   Esc: cancel                     │
        └─────────────────────────────────────────────┘
```

Sized to content (`Percentage(60)` width, capped, centered via `Layout`
margins), main pane dimmed/inactive behind it — an `fzf`/`telescope`-style
picker, entries ranked by `score_all` (spec 0109 Goals 3–4).

**Phase 5 (type definition assistance pane)** — horizontal split, main
pane shrinks:

```
┌──────────────────────────────────┬───────────────────────────────┐
│ Main tree pane (Percentage(65))    │ Type definition (Percentage(35))│
│▾ message_type {                    │ message DescriptorProto {       │
│▸   name: "DescriptorProto"         │   optional string name = 1;     │
│    field { ... }                   │   repeated FieldDescriptorProto │
│                                     │     field = 2;                  │
│                                     │   ...                           │
├─────────────────────────────────────┴───────────────────────────────┤
│ status line                                                           │
├───────────────────────────────────────────────────────────────────-──┤
│ command/message line                                                  │
└───────────────────────────────────────────────────────────────────-──┘
```

Toggled on/off with a key (exact binding TBD). Below a width threshold
(e.g. 100 columns) the split is forced off rather than squeezing both
panes unreadably — this pane is a "when you have room" feature, not core
navigation, matching Phase 5's own framing ("Type definition assistance"
above) as a helper pane, not a required one.

---

## Files changed (anticipated)

| File | Change |
|---|---|
| `protolens/` (new crate) | `Cargo.toml`, `src/main.rs`, `src/decode.rs`, `src/tui.rs`, `src/extract.rs` |
| `Cargo.toml` (workspace root) | Add `protolens` member |
