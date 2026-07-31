<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0109 — `protolens`: interactive schema-inference decoder for unknown protobufs

Status: informational — the founding vision, never implemented *as this
        spec*. It was deliberately superseded a slice at a time: spec
        0111 took decode/navigate/extract, 0114 the single-range type
        override, 0117 the override collection and its YAML
        persistence, 0118 the recursive override render, 0120 the
        Any/MessageSet auto-expansion. Read it for why protolens
        exists, not for what it does; every mechanism here is described
        more accurately elsewhere. Kept because nothing else states the
        original problem.
Refs: `docs/specs/0042-schema-score.md`, `docs/specs/0045-reproto-emit-graph.md`,
`docs/specs/0048-multi-entry-score.md`, `docs/specs/0089-any-expansion.md`,
`docs/specs/0100-message-set-expansion.md`, `docs/specs/0108-message-set-scoring-tier0.md`,
`docs/prototext/PROST-ISSUES.md`
App: protolens (new)

---

## Background

`prototext`/`prototext-graph` already answer two separate questions well: "decode
this blob, given I know its schema" (`prototext decode --type X`), and "which root
type, across an entire descriptor-set corpus, best matches this blob's shape"
(`list-schemas`/`score`, backed by the scoring-walk engine of spec 0089/0108). What
is missing is the *middle* case, which is the common one in practice: the overall
shape is correctly guessed (or already known) at the top level, but individual
nested slots — typically fields wrapped in `google.protobuf.Any`, opaque `bytes`,
or fields whose declared type is simply wrong for the corpus at hand — need
manual, iterative, per-node type assignment, informed by the same candidate-
ranking machinery already built for whole-message inference.

`protolens` is an interactive tool that fills this gap: it lets a user navigate a
decoded protobuf tree, override the interpreted type of any node (individually or
in bulk across all occurrences of the same field), see the result re-rendered
immediately, undo freely, and ultimately turn a session's worth of manual
decisions into a reusable `.desc` schema artifact.

This spec is intentionally high-level: it captures use cases, the envisioned
interaction model, and the architectural shape agreed so far, without committing
to exact APIs or file formats. Several open questions are listed explicitly in
lieu of firm decisions.

---

## Goals

1. An interactive terminal UI to navigate a `prototext decode`-style rendering of
   a protobuf blob, given a descriptor-set corpus.
2. Tree navigation (arrows/hjkl) and fold/unfold of subtrees.
3. Type override of a node, in two scopes:
   - **exact position**: only this specific occurrence is re-typed.
   - **field-number pool**: every node sharing the same
     `(container_fqdn, field_number)` — i.e. every occurrence of the same
     declared field, however many times it repeats in the document — is
     re-typed together.
4. Candidate-type suggestions when overriding, ranked by plausibility, reusing
   the existing scoring engine (`score_all`) rather than a new inference engine:
   - exact-position override: rank candidates by score against that one node's
     raw byte slice.
   - field-number-pool override: rank candidates by score aggregated across
     *all* pooled occurrences (drop any candidate that vetoes on any occurrence,
     rank survivors by combined score).
5. Unlimited undo/redo across all edits (fold/unfold state excluded — that is
   pure UI state, not undoable/session data).
6. **Save project**: persist a full session (base descriptor-set reference(s),
   input reference(s), override decisions, undo history) to a file, resumable
   later with full undo history intact.
7. **Export `.desc`**: given a saved project, produce a self-contained
   `FileDescriptorSet` reflecting the current override decisions, including
   synthesized "patched clone" types where a bulk or positional override
   diverges from an existing known type's declared field — see High-Level
   Design.

## Non-goals

- Decompiling a `.desc` to human-readable `.proto` source. `reproto` already
  solves this (given a collection of `.desc` files, it produces `.proto` files
  and handles syntax/editions concerns) — `protolens` only ever produces
  `.desc` output; turning that into `.proto` source is `reproto`'s job, not
  `protolens`'s.
- Ingesting raw, un-normalized editions-syntax descriptor sets directly.
  `protolens` assumes descriptor sets it's given have already been through
  `reproto`'s normalization (same assumption the rest of the suite makes
  today).
- Extending the scoring engine itself (Tier 1 `Any`/`MessageSet` resolution,
  new `ScoringKind`s, etc.) — `protolens` is a consumer of the existing
  scoring graph, not a place to grow it further.
- Editing the underlying wire bytes/payload. This tool infers and records a
  *schema*; it does not mutate the captured data.
- Multi-user or collaborative editing of a single project file.
- Live re-scoring against corpora too large to fit the existing in-process
  scoring graph (`hopcroft.rkyv`) — same scaling assumptions as `list-schemas`
  today.

---

## Use Cases

1. **Exploratory decode.** An analyst has a blob of unknown or partially-known
   schema and a corpus descriptor set. They open it in `protolens`, see the
   tool's best-guess root type rendered (reusing `list-schemas`'s ranking to
   pick a starting point), and drill into any node whose rendering looks wrong
   (garbled bytes, an `Any` that clearly isn't the type it claims, etc.).
2. **Bulk re-typing.** The analyst notices a nested field is consistently
   mis-rendered across several occurrences (e.g. field 7 under `lib.Foo` never
   looks right under its declared type). They select "override by field
   number," see candidates ranked by aggregate plausibility across every
   observed occurrence of `lib.Foo.7`, and apply one retype to all of them at
   once.
3. **Resume later.** The analyst saves the in-progress investigation and
   returns to it later (possibly a different day, or handed off to a
   colleague with access to the same project file and corpus), with full undo
   history intact.
4. **Hand off a schema.** Satisfied with their override decisions, the analyst
   exports a `.desc` capturing the corrected/inferred schema — including any
   necessary "patched clone" types for fields they retyped away from a known
   type's original declaration — for use by teammates or other tooling
   (`reproto`, `protoc`, `buf`), without hand-writing `.proto` source.

---

## User Interface

Terminal UI, modeled loosely on tools like `less`/tree-view file managers:

- **Main pane**: the decoded tree, one node per line, indentation showing
  nesting, fold markers (`▶`/`▼` or similar) on collapsible nodes. Each line
  mirrors `prototext decode`'s own rendering conventions (field name, wire
  type, value, `#@` annotations) so the visual vocabulary is already familiar
  to existing users of the suite.
- **Navigation**: arrow keys / `hjkl` to move the cursor and collapse/expand
  the current subtree; `+`/`-` (or `zo`/`zc`-style bindings) as explicit
  fold/unfold. Folding is pure UI state, not part of undo history.
- **Override picker**: invoked on the current node (e.g. `Enter`/`o`), opens a
  modal/side panel listing ranked candidate types (FQDN + score), with a
  distinct action to widen scope to "apply to all nodes with this field
  number" versus "this occurrence only."
- **Undo/redo**: `u` / `Ctrl-R` (or similar), unlimited depth.
- **Save / export**: command-line-style entry (`:save <path>`,
  `:export <path>.desc`), consistent with the rest of the suite's CLI-first
  philosophy — `protolens` itself stays a terminal tool, no GUI dependency.
- **Status line**: current node's path (`.1.2`), currently assigned type,
  and — while the override picker is open — the aggregate score breakdown for
  the pool being considered.

---

## High-Level Design

- **New Rust binary crate** (`protolens`), workspace member alongside
  `prototext`, depending directly on `prototext-core` and `prototext-graph`
  in-process — no subprocess/FFI boundary for the interactive loop, so
  navigation and re-scoring stay responsive.
- **TUI framework**: `ratatui` + `crossterm` backend (the de facto standard
  for Rust TUIs; no existing dependency in this workspace to reuse or
  conflict with).
- **Rendering**: reuse `prototext_core::render_as_text` — it already accepts
  an arbitrary byte slice plus a `MessageDescriptor`, not just "the whole
  file" — so re-rendering a node after an override is: extract that node's
  raw byte slice (already known from the initial decode pass), call
  `render_as_text` again with the new type, and splice the result into the
  composite view in place of the old subtree.
- **Candidate ranking**: reuse `prototext-graph`'s existing `score_all`/
  `ScoringOpts` engine — no new inference logic. Field-number-pool ranking
  runs `score_all` once per occurrence and aggregates.
- **In-memory state**: a persistent/immutable tree (structural sharing, e.g.
  `Rc`-based nodes) for the decoded+override view. Every edit (a type
  override) produces a new root via path-copying; undo is a stack of prior
  roots. Cost per edit is proportional to the depth of the changed path, not
  the size of the whole tree — the same technique underlies both the in-memory
  undo stack and, conceptually, the descriptor-graph patch-cloning used for
  export (see below): only nodes on the changed path are cloned, everything
  else is shared.
- **Field-number pool membership** is recomputed dynamically each time the
  override picker is opened for a field-number-level override — not cached —
  since an ancestor's effective type may itself have been overridden earlier
  in the same session, changing which nodes currently share a given
  `(container_fqdn, field_number)`.
- **Project file**: a new serializable format capturing session state — base
  descriptor-set reference(s), input reference(s), override decisions (keyed
  by container FQDN + field number, or by exact node path for
  position-only overrides), and the full undo history. Format (human-readable
  vs. compact binary) is an open question — see below.
- **Export path**: `protolens` does *not* construct the `.desc` itself.
  Constructing it requires cloning-and-patching existing `DescriptorProto`
  entries from the base corpus while preserving every option/extension/
  feature the clone doesn't itself touch — a round-trip-fidelity requirement
  that `prost`/`prost-reflect` cannot currently guarantee (see
  `docs/prototext/PROST-ISSUES.md`, particularly the documented `FieldOptions`
  round-trip bug). Instead, `protolens` exports the *current* override state
  (ignoring undo history — only the final decisions matter for schema
  construction) to a new `reproto` subcommand, which performs the actual
  clone/patch/rename/serialize using the canonical `google.protobuf` Python
  library already trusted elsewhere in this codebase for descriptor-level
  work.
  - **Patch-cloning**: overriding `.1.2`'s type (a field nested inside a
    known type `lib.Foo` used for the root's field 1) means cloning `lib.Foo`
    with only field 2's `type`/`type_name` changed, and repointing the root's
    field-1 edge at the clone — nothing else in the descriptor graph needs to
    change. A chain of overrides at increasing depth produces a chain of
    clones, each pointing at the next.
  - **Cycle avoidance by construction**: all clones generated in one export
    session live in a single new synthetic file, which only ever *imports*
    pre-existing corpus files (for types it still shares unmodified) and is
    never imported back by them. Mutual recursion between two clones from the
    same session is intra-file (always legal); a file-level import cycle
    becomes structurally impossible unless the original corpus was already
    cyclic.

---

## Open Issues and Challenges

1. **Aggregation formula** for field-number-pool candidate ranking (sum vs.
   mean vs. min of per-occurrence scores, after dropping any candidate that
   vetoes on at least one occurrence) — needs tuning against real corpora,
   not just a priori choice.
2. **Naming convention** for synthesized artifacts: both synthetic field
   names (already agreed: tool proposes a default, user can rename before
   committing) and synthetic *patched-clone message* names (default scheme
   not yet decided, e.g. `<fqdn>_patched<N>`).
3. **Project file format**: human-readable/diffable (YAML/JSON, consistent
   with `reproto`'s existing YAML scoring-graph convention) vs. compact
   binary (`rkyv`, consistent with `hopcroft.rkyv`). Undo history could grow
   large over a long session; unlimited undo may need a compaction or
   truncation strategy despite the stated goal.
4. **Conflict handling**: when a field-number-pool override and an
   already-applied position-level override disagree for the same node, the
   export step must surface this as an explicit warning, never silently pick
   a winner. UI-side, `protolens` should probably also warn at override time
   if a new pool-level override would silently override an existing,
   different position-level decision.
5. **Circular-dependency mitigation validation**: the "single synthetic file"
   strategy needs checking against corpora with deep, pre-existing import
   graphs; also needs a decision on how the synthetic file's own
   package/name avoids colliding with existing corpus namespaces.
6. **Performance** of live re-scoring for large field-number pools (many
   occurrences) on every override-picker invocation, for large captured
   blobs — may need per-occurrence score caching across the session.
7. **Composite-rendering UI complexity**: line numbering, wrapping, and
   `#@`-style annotation conventions need to keep working sensibly when a
   view is a patchwork of independently re-decoded subtrees rather than one
   flat `prototext decode` output.
8. **Descriptor-set discovery at export time**: the project file must carry
   enough information (path, hash, or embedded copy) for the `reproto`
   export step to reliably locate the *exact* original descriptor set used
   during the session, even if the corpus on disk has since changed.
