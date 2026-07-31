<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0122 — protolens: patch a node's own `#@` annotation on override, don't resynthesize it

Status: superseded (implemented 2026-07-15, superseded 2026-07-31 by
        spec 0181, which deleted `NodeSpan::natural_annotation` — the
        field G1 added and this spec's whole patching scheme reads
        from. It no longer exists anywhere in `prototext-core`.)
Implemented in: 2026-07-15
Refs: docs/specs/0181-delete-natural-annotation.md (deletes this
        spec's mechanism),
      docs/specs/0110-protolens-sink-based-render-unification.md (`Sink`/
      `TextSink`/`IndexingTextSink`, `NodeSpan`),
      docs/specs/0114-protolens-single-range-type-override.md (§1.1's
      `wrap_blob`/`register_wrapper` root-render trick, generalized by
      0118),
      docs/specs/0118-protolens-recursive-override-rendering.md (§4's
      "unified splice mechanic", `splice_override`, being replaced here),
      docs/specs/0119-protolens-override-fidelity-and-workflow.md (§G4's
      `name` override, `can_override` eligibility gate),
      docs/specs/0120-protolens-any-messageset-as-auto-overrides.md
      (Any/MessageSet auto-override recursion — the trigger for the
      round-trip bug below)
App: protolens, prototext-core

## Background

A round-trip losslessness check (binary → protolens extract root node as
`#@ prototext` text → `prototext encode` back to binary) was found to
silently corrupt every deprecated proto2 *group*-typed field touched by
protolens's override machinery, not just the MessageSet case that first
surfaced it. Root cause, confirmed by direct code reading (not
speculation):

`splice_override` (`protolens/src/tui.rs`) is the single function that
(re)renders a node's whole line range — header, interior, and footer —
whenever an override applies to it, or whenever `render_overrides`
recurses through it at all (spec 0118 §3 recurses into *every*
message/group node on every pass, whether or not an override is
currently active on it — using the node's own natural type as the
"effective" target when none is). It does so by wrapping the node's raw
payload bytes with a synthetic tag+length prefix
(`decode::wrap_blob(field_number, payload)`) and decoding that synthetic
buffer "as if" it were the sole field of a one-field wrapper message
(`decode::register_wrapper`), then splicing the resulting rendered lines
in wholesale. `wrap_blob` hardcodes `WT_LEN` framing — it has no
wire-type parameter and no way to reproduce `WT_START_GROUP`'s literal
`START_GROUP`/`END_GROUP` tag-pair framing. Consequently, every node
`splice_override` touches gets re-decoded as `WT_LEN`-framed regardless
of its true original wire type, and `prototext-core`'s `Sink` — whose
`NestedKind::Message` vs. `NestedKind::Group` dispatch is driven purely
by which wire tag it actually parses (self-describing, not
schema-driven; confirmed in `sink.rs`'s `begin_nested`/`end_nested`) —
always takes the `Message` branch, which never emits the `#@ group`
annotation token. Since `prototext encode`'s own re-encode dispatch is
itself driven by that same annotation token (`encode_text/mod.rs`: `if
ann.wire_type == "group"`), the resulting binary loses the original
group framing entirely — not a coloring cosmetic bug, but a genuine
lossiness bug for every group field an override pass ever touches
(which, per the paragraph above, is unconditionally *every* message/
group node in the document, from the very first paint).

This is a byproduct of `splice_override` regenerating a node's header
line the same, uniform way it regenerates the interior: entirely from
scratch, via a synthetic re-decode. The interior's re-decode is fine —
`extract::message_payload_range` already strips a node's own
tag(+length) generically for `WT_LEN` and `WT_START_GROUP` alike (spec
0120 bugfix), so the *payload bytes* handed to the synthetic re-decode
are identical in shape regardless of original wire type, and the
interior content that comes back is correct either way. It's only the
*node's own line* — the header's `#@ ...` annotation, entirely a
function of the node's own wire framing, not of its interior — that a
`WT_LEN`-only synthetic re-decode can never reproduce correctly for a
group.

Two fix directions were discussed:

- **(A) Patch the field's own line**: derive an override's header
  annotation by patching only the type-declaration portion of the
  annotation this exact node's *own*, one-time, authoritative natural
  decode already correctly rendered (correctly, because that one-time
  decode parses the real wire tag, not a synthetic `WT_LEN` stand-in) —
  leaving everything else in that annotation (crucially, any `group`
  prefix, and any tag/end-tag anomaly modifiers) untouched, byte for
  byte.
- **(B) Resynthesize the header from stored structured facts** (e.g.
  store `tag_ohb`/`tag_oor`/`len_ohb`/etc. as typed fields on `NodeSpan`
  and rebuild the annotation string from them on every splice).

**(A) was chosen.** It is more robust to future evolutions of the `#@`
annotation grammar: (B) requires `protolens` to durably track, and
correctly re-serialize, every individual modifier token
`prototext-core`'s annotation grammar can ever produce — a list that (a)
is not exhaustively mirrored anywhere in `protolens` today (confirmed:
`tag_ohb`/`tag_oor`/`len_ohb` are computed transiently inside
`prototext-core`'s `Sink` and never surface on `NodeSpan` at all) and
(b) could grow over time. (A) never needs `protolens` to understand the
annotation grammar beyond locating one well-defined substring within it
(see Specification) — any *other* token `prototext-core` ever adds to
the grammar is preserved automatically, as opaque text, with zero
`protolens`-side change required.

## Goals

- G1: `prototext_core::serialize::render_text::NodeSpan` gains a new
  field capturing the exact `#@ ...` annotation text this node's own,
  one-time, authoritative (non-wrapped, non-overridden) decode pass
  rendered for it — `None` when annotations are disabled. This field is
  set once, at the moment the node's span is finalized during that one
  authoritative decode, and is never recomputed or overwritten afterward —
  same immutable-fact treatment `wire_type` already gets across
  `splice_override`'s repeated re-splicing of the same node (confirmed:
  `splice_override` already explicitly preserves `old_span.wire_type`
  into every new spliced span, precisely because it's a fact about the
  underlying bytes, not about the currently-displayed override).
- G2: `splice_override` (`protolens/src/tui.rs`) no longer uses its
  synthetic wrapped re-decode's header line wholesale. It still uses
  the existing `wrap_blob`/`register_wrapper`/`decode_and_render_indexed`
  pipeline exactly as today to obtain the node's *interior* content
  (unchanged — the interior was never the bug) and, from that same
  synthetic render, the *new* type-declaration token for the header (see
  Specification for exactly which token). It then builds the node's own
  header line by taking G1's captured natural annotation and replacing
  only that one token, leaving every other token (any leading `group`
  prefix, any tag/end-tag anomaly modifier) untouched, and prepends the
  field name (unchanged mechanism: `field_name_for(idx)`) and appends the
  interior lines and a closing `}` footer line.
- G3: the eligibility gate for which nodes can be overridden at all is
  unchanged: `can_override` (`tui.rs`) already gates on exactly
  `span.is_message || span.wire_type == WT_LEN` — messages, groups
  (`is_message` covers both — spec 0110's existing convention), strings/
  bytes, and packed-repeated records (`WT_LEN`). This spec confirms that
  gate is already correct and requires no change.
- G4: a node's `wire_type` (and, by extension, whether its patched header
  carries a `group` prefix) is preserved unconditionally through any
  number of overrides/retypes applied to it — never derived from, or
  changed to match, the override target's own declared field kind. An
  overridden `WT_LEN` field always patches into a synthetic *message*
  (matching the existing `is_group()`-vs-not split `Sink` already applies
  based on wire framing, not schema); an overridden `WT_START_GROUP`
  field always patches into a synthetic *group*. This is already true of
  the interior rendering (untouched by this spec) and is confirmed to
  already hold for `wire_type` itself across splices — this spec makes
  it hold for the header's *rendered annotation text* too.
- G5: `--no-annotations` mode is unaffected in substance: with
  annotations off, `NodeSpan::natural_annotation` (G1) is always `None`
  (annotations were never rendered to capture), the header line carries
  no `#@ ...` suffix at all (as today), and the group-vs-message
  wire-type distinction still shows up only via the pre-existing,
  unaffected bracket/name shape (which does not differ between the two
  kinds — see Specification) — no behavior to preserve there beyond what
  already exists.
- G6: the reported symptom (a MessageSet `Item` group, and any other
  group field, losing its `#@ group` annotation — and, on
  re-encode, its group wire framing — the moment protolens's override
  machinery touches it) is fixed, confirmed against the real repro
  document from the original bug report.

## Non-goals

- No change to `extract::message_payload_range`'s existing tag/length-
  prefix stripping (spec 0120's group-aware bugfix there is untouched and
  already correct) — this spec only changes how a node's own header LINE
  is *rendered*, not how its payload bytes are *sliced*.
- No change to `RenderCache`'s key shape or eviction strategy (spec 0116
  §8/§10, spec 0118 §5) — the cache key (`payload_range, target,
  field_name`) is unaffected; only the value it caches (now built via
  patching rather than wholesale resynthesis) changes.
- No attempt to track every individual annotation modifier
  (`tag_ohb`/`TAG_OOR`/`len_ohb`/`etag_ohb`/`ETAG_OOR`/`END_MISMATCH`/
  etc.) as a typed `NodeSpan` field — G1's whole point (see Background)
  is to avoid needing to enumerate or duplicate that list at all.
- No change to how `Sink`/`TextSink` render a *fresh, natural* (non-
  overridden) document — `decode_and_render_indexed` and its group
  post-hoc splice (`render_group_field`, `end_nested`'s `TextMark::Group`
  handling) are the *source* of the text this spec captures and reuses,
  not something this spec modifies.
- No change to the `#@` annotation grammar itself
  (`known_field_ann := ["group" ";"] field_decl [";" modifier
  (";" modifier)*]`, `encode_text/encode_annotation.rs`) — this spec
  relies on that grammar's existing, stable shape to locate the
  patchable token; it does not add, remove, or redefine any token kind.
- No change to `field_name_for`/the plain-text field-name-and-brace
  portion of a header line (`"<name> {"`) — already correctly
  reconstructed today (spec 0119 §G4), untouched by this spec.

## Specification

### 1. `NodeSpan::natural_annotation` (prototext-core)

`prototext_core::serialize::render_text::NodeSpan` gains:

```rust
/// The exact `#@ ...` annotation text this node's own header line was
/// rendered with by the one authoritative, non-wrapped decode pass that
/// produced this `NodeSpan` — i.e. everything from (and including) the
/// `#@` marker to end of line, with no trailing newline. `None` when
/// annotations were disabled for this render.
/// Set once, at span-finalization time, and never recomputed —
/// `protolens`'s override/splice machinery (spec 0122) treats this the
/// same way it already treats `wire_type`: an immutable fact about the
/// underlying bytes, independent of whatever override is currently
/// displayed.
pub natural_annotation: Option<String>,
```

Populated by `Sink`'s `begin_nested`/`end_nested` (`sink.rs`), captured
*after* any post-hoc splice a group's own close-time annotation insertion
performs (`end_nested`'s `TextMark::Group` branch already splices
`tag_ohb`/`TAG_OOR`/`etag_ohb`/`ETAG_OOR`/`END_MISMATCH`/field-decl
tokens into the header line at close time — `natural_annotation` must
reflect the *final* text, not the pre-splice partial `"group"`-only
text `begin_nested` alone produces). For `TextMark::Message`, the
annotation is already complete at `begin_nested` time (no post-hoc
splice needed there) and can be captured then.

### 2. `splice_override` header construction (protolens)

`splice_override(idx, target)` (`protolens/src/tui.rs`) changes as
follows. Everything about how `payload_bytes` is computed
(`extract::message_payload_range`), how the interior is rendered
(`wrap_blob` + `register_wrapper` + `decode_and_render_indexed`, cached
via `RenderCache`), and how old/new lines are stitched into
`self.lines`/`self.tree` (the seam-finding, doc-chain, and ancestor
closing-brace-line-shift logic) is **unchanged** — this spec only
changes what the *header line's* text is, before it gets spliced in.

1. Run the existing synthetic-wrapper render (`wrap_blob`+
   `register_wrapper`+`decode_and_render_indexed`) exactly as today,
   producing `new_text`/`new_spans` as before — needed regardless of
   whether annotations are enabled, since it is also where the
   interior lines (step 6) come from.
2. If annotations are disabled (`opts.annotations == false`, mirroring
   `self.annotations`), skip straight to step 7: the header line is
   just `field_name_for(idx)` followed by literal `" {"`, with no
   `#@ ...` suffix at all (G5) — none of steps 3–6 below (deriving or
   patching an annotation) apply, since no annotation was rendered by
   step 1's synthetic pass to extract a token from in the first place.
   Otherwise (annotations enabled), continue with step 3.
3. From `new_text`'s own header line, extract just its type-declaration
   token — under `wrap_blob`'s hardcoded `WT_LEN` framing this token is
   always token 0 of the annotation (`wrap_blob` never produces a
   leading `group` token), so it is: the annotation substring up to (but
   not including) the first `"; "` separator, or the whole annotation if
   there is no separator. This token is either a real field declaration
   (`"MyType = 5"`, `"repeated MyType = 5"`, etc. — when `target`
   resolves against the descriptor pool) or the bare wire-type
   placeholder `"message"` (when `target` is `None`, i.e. reverting to
   raw/unknown).
4. Take `old_span.natural_annotation` (G1) as the base text to patch.
   This is normally `Some`: annotations are known to be enabled (step 2
   already exited early otherwise), and protolens's own pipeline never
   produces a virtual `NodeSpan` (confirmed: `begin_virtual_nested` — the
   only path that ever creates one — is invoked exclusively from
   `any_field.rs`/`message_set_field.rs`, both gated behind the
   `expand_any`/`expand_message_set` render options, which every
   `NodeSpan`-producing call site in protolens — `decode::decode()` and
   `splice_override`'s own synthetic re-render alike — unconditionally
   sets to `false`).

   The one remaining legitimate `None` case is a node whose own
   authoritative decode was a *scalar* field (`WT_LEN` string/bytes, or a
   packed-repeated record): `natural_annotation` is unconditionally
   `None` for such nodes (only message/group nodes ever populate it —
   Test Plan item 1), and G1's immutable-fact treatment means that `None`
   persists across every subsequent override of the node too, even once
   G3's gate has let it be overridden into a message/group type. There is
   no base text to patch in this case — and none is needed: by G4 the
   node's `wire_type` stays `WT_LEN` forever, so its header can never
   require a `group;` prefix, and any modifier tokens a captured natural
   annotation might have carried are unrecoverable by construction (the
   same limitation G1 already accepts for `--no-annotations` mode). So:
   use step 3's synthetic type-declaration token as the entire annotation
   verbatim, skip steps 5–6 (there is no other token to preserve), and
   continue at step 7.
5. Otherwise (`old_span.natural_annotation` is `Some`), within that base
   text, locate the same type-declaration/wire-type-placeholder token
   slot:
   - If the base text's first token is exactly `"group"`, the slot is
     token 1 (immediately following), when present. `TYPE_MISMATCH`
     (the only token that can occupy that position while genuinely
     *not* being a type-declaration slot — emitted for a group field
     whose resolved schema says non-group) counts as "slot occupied, to
     be replaced," not as an anomaly modifier to preserve: it describes
     a mismatch against the *previous* interpretation, which is no
     longer meaningful once a fresh, explicit `target` is applied here.
     When no token 1 exists at all (an entirely unknown group field:
     base text is just `"group"` alone, or `"group; <modifier
     tokens...>"` with no decl/mismatch token), the new token is
     *inserted* immediately after `"group"` (or after `"group; "`,
     before any modifier tokens) rather than replacing anything.
   - Otherwise (no leading `"group"`), the slot is token 0 — always
     present for a message-shaped node (`Sink`'s `Message` branch always
     writes either a field declaration or the bare `"message"`
     placeholder — confirmed, no third case).
6. Every other token in the base text (any leading `"group"`, any
   `tag_ohb`/`TAG_OOR`/`etag_ohb`/`ETAG_OOR`/`END_MISMATCH`/`len_ohb`
   modifier) is copied through verbatim, unexamined — this is the crux
   of "patch, don't resynthesize": `protolens` never needs to know what
   any of these tokens mean or how many of them there are, and any
   future token `prototext-core`'s grammar adds is preserved
   automatically.
7. The patched annotation, `field_name_for(idx)` (unchanged), and a
   literal `"{"` are combined into the new header line; the interior
   lines from step 1's render are used unchanged (already correct,
   independent of wire type — confirmed, this was never the bug); a
   literal `"}"` is the new footer line (both `NestedKind::Message` and
   `NestedKind::Group` already render an identical `"}"` closing line —
   confirmed in `sink.rs` — so no group/message distinction is needed
   here).
8. The existing root-header "`0 `"/"`<field_number> `" stripping special
   case (today's `splice_override`, needed because the *wrapper's own*
   header line was previously used wholesale) is removed — it no longer
   applies, since the header is no longer built from the wrapper's own
   render at all.

### 3. `RenderCache` value shape

Unaffected in kind (still `(Vec<String>, Vec<NodeSpan>, StyleHints)`) —
only *which* lines end up in the cached `Vec<String>` changes (patched
header instead of wholesale wrapper header). Cache key is unchanged.

## Test plan

1. `NodeSpan::natural_annotation` unit tests (prototext-core): a plain
   message field, a group field (with and without a resolvable schema,
   with and without tag/end-tag anomaly modifiers), and a scalar field
   (`None`, since only message/group nodes carry this) each capture the
   expected exact annotation text.
2. `splice_override` unit tests (protolens, extending the existing
   `splice_override_omits_the_synthetic_root_field_name_from_the_header_
   line` and `message_set_group_items_auto_expand_through_render_
   overrides` tests):
   - Overriding a group field to a resolvable type produces a header
     with `#@ group; <NewType> = <N>` — `group;` present.
   - Overriding a `WT_LEN` field to a resolvable type produces a header
     with `#@ <NewType> = <N>` — no `group;`.
   - A group field carrying a `tag_ohb`/`TAG_OOR`/`etag_ohb`/
     `ETAG_OOR`/`END_MISMATCH` modifier keeps that modifier verbatim
     after being overridden to a different type.
   - Reverting an override (`target: None`) on a group field restores
     `#@ group` (with no decl token, or `TYPE_MISMATCH` if the node's
     own natural schema — not the override — genuinely mismatches).
3. Round-trip regression test (new, using spec 0123's batch mode once
   available, or a direct `App`-level test beforehand): decode a
   fixture containing a MessageSet, apply the automatic Any/MessageSet
   overrides (spec 0120), extract the root as `#@ prototext` text,
   `prototext encode` it back to binary, and assert byte-for-byte
   equality with the original — the original reported bug's exact
   scenario.
4. `reuse lint` passes (no new files besides this spec).
