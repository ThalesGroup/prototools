<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Asset: the descriptor context

*last verified: 2026-07-31*

## Executive summary

`DescriptorContext` is protolens's window into "what schemas does the user
currently have access to." It wraps a `prost_reflect::DescriptorPool` —
today always a single pool, see the [multi-database forward-looking
note](README.md#forward-looking-multi-database-support) — plus an
optional scoring graph used to rank candidate types when the user hasn't
said which one they mean. It is also where protolens registers types that
have no counterpart in any real `.proto` file: synthetic wrapper
descriptors for the override mechanism, and a shared shape for
`MessageSet` items. `prototext_core` is deliberately kept out of all of
this — see the [prototext_core boundary](README.md#the-prototext_core-boundary).

## Technical detail

### Two ways to hold a pool: eager, or on demand

`DescriptorContext` has an eager branch (a fully built
`prost_reflect::DescriptorPool`) and a lazy one (a `LazyPool` over an
`index.rkyv` sidecar, in the `prototext-schema` workspace member shared
with `prototext`). Which branch is taken is an implementation detail —
`pool()` and `pool_mut()` have the same signatures either way, and every
call site is written against them. What differs is *what is in* the pool
at a given moment: the lazy branch resolves a file only when a type from
it is asked for.

This exists because building the pool eagerly is essentially the whole
startup wait — 1.70 s of a 2.96 s batch run on the 25.6 MB googleapis
corpus — and because `drop(pool)` costs a further 0.84 s at process
exit, which no startup-shaped measurement shows. The lazy branch is
taken only when a sidecar is present and accepted; a missing, stale or
unreadable one degrades to the eager path rather than failing, so the
sidecar is an optimization and never a requirement (spec 0197).

### Root-type autoinference is advisory, never blocking

When the user doesn't specify `--type` explicitly, protolens tries to
guess the document root's type using `prototext_graph::score_all` against
the loaded scoring graph. This inference is explicitly *not* an
error-or-success API: an inconclusive result (no graph loaded, no
candidates score at all, every candidate vetoed, or a tie for the top
score) is a normal, expected outcome — `Ok(None)` — not a failure. The
caller's response to "inconclusive" is simply to render the document raw
(`<raw / no type>`) and let the user resolve it manually via the override
system, exactly like any other unresolved node. No part of protolens ever
treats "we couldn't guess the type" as fatal.

### Synthetic descriptors: registered once, keyed by shape

Two kinds of type exist in the pool that were never written by a human in
a `.proto` file:

- **Wrapper descriptors**, one per distinct `(field number, field type,
  target type)` combination actually used during the session. These are
  what let `splice_override` decode an arbitrary byte range under an
  arbitrary target type: the wrapper is a fresh one-field message whose
  sole field has that number and that type. Its field is always *named*
  `_`, and the node's real display name is patched into the rendered
  text afterwards — so the name is deliberately absent from the
  registration key, and from the render cache's key with it. Two nodes
  that share a field number and target type but differ in name (say
  after a manage-pane rename) correctly share one wrapper and one
  cached render.
- **`MessageSetItem`**, a single globally-shared synthetic shape (`type_id`
  + `message`) representing one entry of a `MessageSet`'s repeated group.
  Unlike wrapper descriptors, there is exactly one of these per pool,
  reused for every `MessageSet` occurrence in the document, because the
  shape itself never varies — only what `message`'s bytes turn out to
  decode as (resolved separately, per occurrence, by the override system's
  Any/MessageSet auto-expansion — see
  [document-tree.md](document-tree.md)).

Both registrations are idempotent and cheap to call repeatedly; callers
don't need to track "have I already registered this" themselves.

### Why `decode()` disables prototext_core's own expansion

`prototext_core` has its own built-in ability to auto-expand `Any` and
`MessageSet` fields during decode. protolens turns this off
unconditionally (`expand_any: false, expand_message_set: false`) and
re-implements the equivalent behavior itself, as ordinary — if
automatically seeded — overrides. The reason is architectural, not a
workaround: prototext_core's expansion is a decode-time, one-shot
transform with no way to *undo* or *retype* an individual expansion
afterward, whereas protolens's override system is designed from the
ground up to be revisited, deactivated, or overridden by the user at any
time. Doing Any/MessageSet expansion as overrides means it participates in
exactly the same lifecycle (activation, demotion, persistence to YAML) as
every other user-driven retype, with no special-casing anywhere else in
the tool.
