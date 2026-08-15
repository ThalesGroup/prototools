<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0299 — `message` as an override type

Status: implemented
Implemented in: 2026-08-15
App: protolens
Refs: docs/specs/0114-….md (§1.1, the synthetic field-1 wrapper every span
        coordinate is stated against), docs/specs/0135-….md (§G3–G4,
        wrapper targets and primitive keywords), docs/specs/0136-….md
        (§G6, `fqdn_needs_dot_prefix`), docs/specs/0168-….md (`--raw`),
        docs/specs/0216-….md (S1, arena slot 0 *is* the wrapper),
        docs/specs/0238-….md (N6, why the scoring veto is not touched),
        docs/specs/0266-….md (the probe's verdict — deliberately untouched)

## Background

protolens renders an untyped document as a single opaque string whenever
the payload holds one malformed token anywhere, however late.
`grpconf/stage/boblog` — 20 198 bytes, three intact log entries and a
fourth cut short by 1 024 bytes — renders as one 20 KB line and `/1` does
not resolve.  Three gRPConf demo beats stand on that document.

The cause is a seam, not a policy. `Blob` prepends a real tag+length
prefix (`blob.rs:51`) so that every span coordinate is relative to the
wrapped bytes. `render_resolved` then registers a wrapper descriptor for
that field — but only when a root type was resolved
(`decode.rs:1824-1839`). With no root type it passes `None`, so the
document falls through to the ordinary **unknown-LEN** path and is put to
`ProbeSink`: a probe whose question is *"should I descend into this nested
payload?"*. One invalid token disqualifies it (spec 0266), and the whole
file collapses to `bytes`.

`prototext decode --raw` renders the same buffer correctly, because it
renders it *as a document* and never probes.

The fix is not to change `render_resolved` or the probe. What is missing
is a way for the *reader* to overrule the probe: `message` joins the
override vocabulary as a first-class target, alongside the 15 primitive
keywords and `None`. Applied to the root, it makes the untyped document
render exactly as `prototext --raw` does.

The arena has always been wrapper-shaped: spec 0216 S1 makes slot 0 the
wrapper and the top-level occurrences its children. Only the render
disagreed.

## Measured on today's release binary

Stand-in for the fix: `--load-overrides` with a YAML entry `type:
message` at path `/`. The annotation on the wrapper's own header is
`#@ message = 1` (the synthetic type's short name, which is the keyword
the user typed).

| fixture | untyped | after `override / --as message` | `prototext --raw` |
|---|---|---|---|
| `0a 03 abc 0a 10 "short"` | `1: "\n\003abc\n\020short"  #@ string` | `1: "abc"` / `1: "short" #@ TRUNCATED_BYTES; MISSING: 11` | identical |
| `0a 03 abc 04` | `1: "\n\003abc\004"  #@ string` | `1: "abc"` / `0: "" #@ INVALID_GROUP_END; TAG_OOR` | identical |
| `0a 03 abc 00 01` | `1: "\n\003abc\000\001"  #@ string` | `1: "abc"` / `0: 1 #@ varint; TAG_OOR` | identical |
| `grpconf/stage/boblog` | one 20 KB `#@ string` line | 1 180 lines, one anomaly | identical |
| `grpconf/stage/bobshark` | 23-line render | diff-empty | — |

`boblog` → `export / --format binary` with the override: byte-identical to
the input (20 198 → 20 198; it was 20 202 untyped, a spurious tag and
3-byte length).

Non-protobuf probes (PNG, prose): still one opaque line — nothing about
probing changed.

## Goals

- **G1.** `message` becomes a first-class override target: any LEN node
  can be reinterpreted as a schema-free message, and any schema-free
  message can be reinterpreted back to a primitive.
- **G2.** The combination "open untyped, then `:override / --as message`"
  produces text byte-for-byte identical to what `prototext decode --raw`
  emits for the same bytes. That command already promises "as raw wire
  bytes"; this makes it reachable from the TUI.

## Non-goals

- **N1.** Do not change `ProbeSink`'s policy. Spec 0266 is right: for a
  *nested* LEN payload somebody wrote a length prefix and we must guess
  what is inside it, and the case of the token is the verdict. Probing is
  unchanged — only the user's ability to overrule the result changes.
- **N2.** Do not demote `walk.rs:1649`'s `veto_all` ("LEN body extends past
  end of buffer") or add a truncation penalty tier to the scoring walk.
  Three reasons: it would not fix this (the scorer is never consulted for
  the untyped render); "penalize but keep the score" needs the
  per-boundary snapshot deferred as spec 0238 N6; and a nested overrun
  (the prefix lied — real evidence of a mis-parse) is a different fact
  from a depth-0 overrun (the file was cut).
- **N3.** No new CLI flag. A `--message` switch would have to be decided
  before the reader has seen the file, and the capability belongs in the
  override vocabulary where every other reinterpretation lives.

## Specification

- **S1.** `DescriptorContext` gains a `schema_free_message` method returning
  a synthetic descriptor registered on first use through the existing
  `register_synthetic` and cached by the pool's own `get_message_by_name`
  early return. Package: `protolens_internal`; short name: `message`
  (FQDN `protolens_internal.message`); **no fields**. A zero-field message
  is what makes prototext-core render every field it meets as unknown,
  which is exactly `--raw`'s top-level behavior.

  The short name is the keyword itself, deliberately: the synthetic type's
  name reaches the `#@` annotation on the spliced node's header, and
  `message` is the one word there that is both true and already in the
  reader's vocabulary — they just typed it. (Measured: the header reads
  `1 {  #@ message = 1`.)

  `google.protobuf.Empty` is not reused: it is not guaranteed present in
  every pool protolens is handed, and it would put a real, misleading FQDN
  where the reader expects none.

- **S2.** `wrapper_target_for` gains a rung for the keyword `message`,
  resolving to `(Some(WrapperTarget::Message(S1's descriptor)),
  Type::Message)`. The rung goes **after** the message-FQDN rung and
  before the primitive rung, preserving the ladder's existing rule: a real
  message named `message` still resolves as itself; bare `message` loses
  to an FQDN only in a pool that has that type.

  It cannot be a line in `primitive_type_for_keyword`, whose contract is
  `(None, ty)` — no target. `message` carries one.

  `is_group` is always `false` for this rung. The keyword is offered for
  `WT_LEN` alone (S3), so a group node never reaches here through a path
  that validated the keyword.

- **S3.** `primitive_keywords_for_wire_type` is renamed
  `override_keywords_for_wire_type`; its current name becomes false the
  moment `message` is in it. The renamed function offers `message` for
  `WT_LEN`, feeding `:override`'s wire-compatibility check and its
  tab-completion.

  `WT_START_GROUP` is left alone. Group framing already *is* message
  framing; offering a keyword that changes nothing is noise.

- **S4.** `fqdn_needs_dot_prefix` (`override_display.rs`) adds `message`
  to its collision set, so a real type of that name displays as `.message`
  in the status line. Presentation only — resolution order is unchanged.

  The two halves of the predicate — the override keywords (fifteen
  primitives plus `message`, via `is_override_keyword`) and the `None`
  sentinel — are now both live, so they are stated together in one
  predicate rather than scattered.

## Alternatives considered

### A `--message` CLI flag

Rejected under N3. It also costs a flag on stage in the gRPConf beat that
opens a log with no type at all, which cuts against "it is the app that
does it".

### Forgiving a truncated tail at the root

A principled variant: nested, a short body means the length prefix lied;
at the root nobody wrote a prefix, so a short tail only means the file was
cut. Measured, it would work for `boblog` — its only anomaly at any depth
is the single `TRUNCATED_BYTES`. But it needs a "this is the document
root" bit plumbed into prototext-core, and it still leaves protolens
disagreeing with `prototext --raw` on the stray-`\x04` and field-0 cases.
Two rules where one will do.

### Never probing the root

Same instinct, wider. Rejected for the same reasons, and it breaks the
demo beat that opens `boblog` *untyped on purpose* — that beat needs the
probe's verdict, because it shows the reader what the probe says before
they overrule it.

### A scoring-penalty tier for truncated payloads

Rejected under N2. The scorer is never consulted for the untyped render;
and "penalize but keep the score" is deferred as spec 0238 N6.

## Test plan

1. **Unit** — `a_real_type_named_message_still_resolves_as_itself`:
   `wrapper_target_for("message", false)` on an empty context gives S1's
   zero-field descriptor; with a pool that has a type literally named
   `message`, the pool wins.
2. **Unit** — `the_keyword_is_offered_for_len_and_shadows_a_bare_fqdn`:
   `override_keywords_for_wire_type(WT_LEN)` contains `message`;
   `WT_START_GROUP` is empty; `format_fqdn_label("message")` is
   `".message"`.
3. **Behavioral** — `overriding_the_root_to_message_renders_what_prototext_raw_renders`:
   fixture `0a 03 abc 0a 10 "short"`, untyped first to confirm the probe
   verdict, then `override / --as message`, then assert interior lines
   equal `prototext decode --raw`. Asserting *agreement with prototext*
   rather than a hand-copied expectation is the point.
4. **Behavioral** — `a_well_formed_payload_reads_the_same_either_way`:
   well-formed bytes; the non-wrapper lines are unchanged by the override.

## Measured outcome

All four automated tests pass. Manual verification on `grpconf/stage/`:

- **Round trip**: `boblog` with `override / --as message`, `export /
  --format binary` → 20 198 bytes, byte-identical to the input. Without
  the override: 20 202 bytes, a spurious wrapper tag and 3-byte length.
- **Render expansion**: untyped → 1 line; overridden → 1 180 lines.
- **No-op on well-formed**: `bobshark` diff-empty, 23 lines in both cases.
- **Non-protobuf unaffected**: PNG and plain text still render as one
  opaque `#@ bytes` / `#@ string` line — nothing about probing changed.
