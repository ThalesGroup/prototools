<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# protolens — design

*last verified: 2026-07-31*

## Executive summary

protolens is an interactive terminal tool for exploring an arbitrary
protobuf-encoded blob that may have **no compiled schema at all**, or only a
*partial* one. It decodes the wire format structurally (message boundaries,
field numbers, wire types) independent of any `.proto` definition, renders
it as indented, annotated text, and lets the user progressively attach
schema knowledge — "this range is really a `foo.bar.Baz`" — to individual
subtrees while browsing. Each such attachment is called an **override**.
Applying one re-decodes just that subtree under the chosen type and
re-renders it in place, without disturbing the rest of the document or the
user's navigation state.

The tool's central idea is that "decode" and "know the schema" are
decoupled: a message is always renderable (worst case, as a raw field-list
dump), and schema knowledge — however it was obtained (an explicit CLI
`--type`, a scored auto-inference guess, or a manual override) — only ever
*improves* that rendering, never gates it. This is what makes protolens
useful on real-world blobs where the schema is unknown, partially known, or
evolving (extensions, `Any`, `MessageSet`) — the common case when
reverse-engineering wire captures or debugging schema mismatches.

## Technical overview

protolens is a Rust binary crate (workspace member) built on `ratatui` +
`crossterm`. `main.rs` is a `clap`-based CLI entry point with two modes: an
interactive TUI session (the primary use case) and a non-interactive
`export` subcommand that reuses the same decode/tree/override machinery
headlessly.

At the top level:

- `decode.rs` — turns a blob + optional descriptor pool into a
  `Decoded` value: rendered lines, the node arena and the overlay over
  it. Owns the synthetic-wrapper trick that lets *any* byte range (not
  just a real top-level message) be decoded as if it were a message
  field, which is what makes overrides possible.
- `blob.rs` — the wrapped byte buffer and its wrapper offset.
- `extract.rs` / `export_descriptor.rs` — slice a node's own bytes
  (binary) or re-decodable text back out of the document, or emit a
  synthetic schema for it, independent of the tool's own display state
  (fold, pan, etc.).
- `override_pane.rs` — the persistent, YAML-serializable model of "what
  the user has told protolens about the schema so far" — despite the
  name, this is data, not UI (the UI lives in `tui/override_select.rs`
  and `tui/manage_pane.rs`; `override` is a reserved word in Rust, hence
  the file's name).
- `render_cache.rs` / (the `CandidateCache` half of `override_pane.rs`) —
  two structurally identical, byte-bounded MRU caches that make repeated
  re-rendering and re-scoring of the same byte range cheap.
- `provenance.rs` — the interned `(type, origin)` pair a node records as
  what it was last rendered as.
- `sweep.rs` — the parallel root-type inference sweep and the stack-size
  constant every scoring thread takes.
- `colorize.rs` / `theme.rs` — syntax highlighting: a tree-sitter grammar
  parses protolens's own rendered text, and a theme layer maps the
  resulting roles to terminal colors with graduated true-color fallback.
- `complete.rs` — shell-completion script generation for `clap`.
- `tui/` — the interactive session itself: `mod.rs` defines the `App`
  struct (all mutable session state) and the top-level event loop;
  sibling files each own one pane's or one concern's behavior — the panes
  and input plumbing (`navigation`, `mouse`, `override_select`,
  `manage_pane`, `override_apply`, `key_dispatch`, `command_line`,
  `render`, `event`, `neovim`), the document's own geometry
  (`structure` for the arena's derived links, `lines` for line positions
  derived from subtree sizes), the background heat-cue subsystem
  (`heat_worker`, `heat_cue`, `tiered`), and `trace` — the only way to
  instrument a TUI that owns the terminal.

Two ideas recur throughout and are worth naming up front, since later
scope files lean on both without re-explaining them:

- **The synthetic wrapper.** protolens never asks `prototext_core` to
  decode "this message starting here." Instead it always wraps a byte
  range in a fresh, single-field tag+length envelope and decodes *that*,
  using a synthetic one-field message descriptor when a type is known.
  This uniform trick is what lets the document root, an override target,
  and a raw fallback all go through exactly one code path.
- **The splice.** Applying (or clearing) an override never touches
  anything but the affected subtree: its rendered lines, its style spans,
  and its arena entries are replaced in place, while every sibling,
  ancestor, and unrelated cursor/fold/jumplist reference stays valid.
  `splice_override` is the one function that knows how to do this; nearly
  everything else in the override system exists to decide *when* to call
  it and with *what* target.

## Scope index

Each scope below has its own file with a short executive summary followed
by technical detail — read the summaries first for orientation, the
technical sections when you need to change something in that area.

Scopes come from three overlapping, non-partitioning groupings:
**assets** (the data protolens operates on), **panes** (the TUI surfaces
a user interacts with), and **pipelines** (end-to-end paths that cut
*across* several assets and panes). A single mechanism (e.g. the override
collection) is usually described once, as an asset, and referenced (not
re-explained) from the pane files that present it.

A pipeline file exists precisely when no single asset or pane owns the
thing being explained. Rendering is the motivating case: it starts in
`decode.rs` (an asset), passes through the tree and the override
collection (two more assets), surfaces in the main pane, and is joined
mid-flight by a background thread — so filing it under any one of those
would have forced the others to re-explain it. Pipeline files are
therefore deliberately reference-heavy and own no data model of their
own.

### Assets

| File | Covers |
|---|---|
| [target-blob.md](target-blob.md) | The wrapped byte buffer, the synthetic-wrapper trick, payload extraction |
| [descriptor-context.md](descriptor-context.md) | The descriptor pool, root-type autoinference, synthetic type registration, the `prototext_core`/`prototext_graph` boundary |
| [arena-and-batch.md](arena-and-batch.md) | The node arena: how it is built from the bytes alone, what level order buys, the superset property, the splice, and where the memory goes (measured) |
| [document-tree.md](document-tree.md) | What the arena means to the rest of protolens: provenance and demotion detection, Any/MessageSet auto-expansion |
| [override-collection.md](override-collection.md) | The override data model, origin/kind system, auto vs. manual entries, YAML persistence, the render-pass architecture |
| [caches.md](caches.md) | The two MRU byte-bounded caches and why they exist |

### Panes

| File | Covers |
|---|---|
| [main-pane.md](main-pane.md) | Navigation, fold/unfold, mouse selection/clipboard, syntax highlighting |
| [override-select-pane.md](override-select-pane.md) | Candidate ranking, sort modes, live preview |
| [manage-pane.md](manage-pane.md) | The override collection's UI: lifecycle, grouping, kind rotation |
| [command-line.md](command-line.md) | The global command/message row (commands/search/rename/messages), Tab-completion |
| [help-and-chrome.md](help-and-chrome.md) | Help overlay, splash, local statuslines, quit/suspend confirmation |

### Pipelines

| File | Covers |
|---|---|
| [rendering.md](rendering.md) | The five rendering stages (eager decode-to-text, whole-document highlight, arena build, per-frame viewport draw, override splice) plus the concurrent heat-cue pipeline; the three coordinate systems and the invariants tying them together; measured costs at scale |

## The `prototext_core` boundary

protolens is a consumer of `prototext_core`, not an extension of it, and
this document deliberately does not describe `prototext_core`'s own
internals (see `docs/prototext/` for those). At the interface, protolens
depends on exactly four things:

1. `decode_and_render_indexed` + `DecodeRenderOpts` — the schema-optional
   decode/render entry point, called with `expand_any`/`expand_message_set`
   always **off** (protolens implements its own Any/MessageSet expansion
   as ordinary overrides — see `document-tree.md` — rather than relying on
   prototext_core's virtual-node expansion). It is fallible: it refuses a
   buffer over `MAX_INDEXED_BUFFER` (511 MiB) rather than overflow the
   `u32` offsets its spans carry.
2. `NodeSpan` — the flat, per-field record `decode_and_render_indexed`
   emits, which `arena-and-batch.md`'s overlay is built from. 32 bytes, pinned
   by a compile-time equality assertion (spec 0212); its byte offsets and
   line numbers are `u32`, and its "absent" fields use the named sentinels
   `NO_FQDN` and `NO_PACKED_RECORD` rather than `Option`.
3. `FqdnTable` — the intern table a span's `type_fqdn` indexes into.
   protolens owns exactly one per document (`Decoded.fqdns`, then
   `App.fqdns`) and passes it back in on every re-render, because an id
   must name the same type in a freshly spliced span as in the arena
   around it. `get` resolves an id to a name; `id_of` interns a name for
   comparison against a span, and answers a *distinct* reserved id
   (`UNINTERNED`, never `NO_FQDN`) for a name the document never
   produced.
4. Low-level wire-format helpers (`WT_LEN`, `WT_START_GROUP`,
   `parse_varint`, …) — used directly by `extract.rs` and the override
   machinery to reason about tag/length framing without re-decoding. Note
   the `WT_*` constants are `u32` while a span's `wire_type` is a `u8`, so
   comparisons against a span cast.

`prototext_graph`'s `score_all`/`ScoringOpts` (a separate crate, the
`hopcroft.rkyv` scoring sidecar) is the sole source of the "inferred"
candidate ranking in `descriptor-context.md` and `override-select-pane.md`;
it is optional at runtime (`DescriptorContext.graph: Option<_>`), and
protolens degrades gracefully to lexicographic-only ranking without it.

## Forward-looking: multi-database support

A teased, not-yet-designed future feature is splitting today's single
descriptor pool into two: a large, read-only pool of well-known-type (WKT)
and common vendor schemas, loaded once and shared, plus a small, dynamic
per-session "scratchpad" pool that the user builds up interactively (e.g.
by pointing protolens at a `.proto` file mid-session, or by promoting an
inferred structure into a real named type). This would very likely require
`prototext_core` interface extensions for genuine multi-database lookup
(today's `DescriptorPool` is singular throughout the stack) and an
extended type-autoinference story — scoring across two pools with
different trust/recency characteristics is not the same problem
`prototext_graph::score_all` solves today. No design work has started on
this; it is recorded here only so the scope files' single-pool
assumptions (`descriptor-context.md` in particular) aren't mistaken for a
permanent constraint.
