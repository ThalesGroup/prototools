<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0315 — an anchor needs a name of its own

Status: implemented
Implemented in: 2026-08-17
App: protolens
Refs: docs/specs/0299-the-file-is-not-a-field.md (the `message` keyword
        and the schema-free zero-field synthetic this generalizes),
      docs/specs/0309-the-pane-says-what-enter-will-do.md (why
        `fqdn:field` is refused under a `message`-overridden parent),
      docs/specs/0237-the-origin-is-the-argument.md (`:override`'s
        grammar, its completers and their two primitives),
      docs/specs/0117-protolens-override-collection.md (the
        `:save`/`:restore` YAML this adds one key to),
      docs/specs/0305-a-loading-list-is-not-an-empty-list.md
        (`lexico_candidates`, shared with the cold-cache placeholder)

## Background

Spec 0299 gave the reader `--as message`: reinterpret a node's bytes as
a message with no schema, so every field inside renders as an unknown
with its number and wire type. It is what makes a truncated or untyped
blob readable at all — `grpconf/stage/boblog` goes from one 20 KB line
to 1 180 lines with it.

But a node opened that way is a dead end for *further* naming. Spec 0309
refuses `fqdn:field` origins under a `message`-overridden parent, in two
places: `override_origin_if_it_fits` (`override_apply.rs:1713`) and
`origin_for_kind`'s `FqdnField` arm (`override_apply.rs:1851`). The
reason is not that a schema-free parent is an unfit anchor — it is that
every one of them has the *same* name. `protolens_internal.message:3`
would claim field 3 of every node anyone ever overrode to `message`.

So the reader who has just made a blob legible can only name its fields
one node at a time, with `path` or `path:field` origins, and the second
occurrence of the same structure elsewhere in the document has to be
done again from scratch. `fqdn:field` — the origin kind spec 0308 made
the *default* precisely because a reader who retypes a field means the
field — is unreachable exactly where it would earn the most.

The missing capability is a name. Not a schema: a name.

## Goals

- **G1.** The reader can declare a message type that the descriptor set
  does not contain, and override a node to it.
- **G2.** Such a type anchors `fqdn:field` overrides with no special
  case — the existing origin machinery treats it as the ordinary FQDN
  it is.
- **G3.** Declaring is idempotent, so a scripted step (spec 0271) that
  declares a type replays without error.
- **G4.** A misspelled `--as` remains an error. Today
  `--as googl.rpc.Status` is refused; that diagnostic is load-bearing
  and must not be weakened into a silent success.
- **G5.** Declared types are visible where they are useful: `--as`
  completion and the selection pane's lexicographic list.
- **G6.** A saved override collection that uses declared types restores
  into a fresh session.

## Non-goals

- **N1. Declaring fields.** A declared type is always the zero-field
  message spec 0299 already builds. The reader's reconstructed schema is
  the set of `fqdn:field` override entries hanging off the anchor, not a
  descriptor. One representation, already persisted, nothing to keep in
  sync — and it is what makes N2 safe. Synthesizing real fields would
  require a schema editor and a second source of truth.
- **N2. An `--idempotent` / `--allow-redo` flag.** Considered and
  rejected; see Alternatives.
- **N3. Declared enums, or declared anything that is not a message.** An
  enum cannot anchor a field-scoped origin, which is the entire purpose.
  This is also why the flag is `--as-new` and not `--as-new-message`:
  there is nothing else it could create.
- **N4. A listing command (`:types`).** Completion (S7) and the
  selection pane (S8) are the two surfaces the feature needs to be
  usable. A standalone listing may turn out to be wanted; it is cheap to
  add later and expensive to guess at now.
- **N5. Recomputing `all_type_fqdns` so declared types appear in it.**
  That list is built once per session, before any override exists
  (`override_pane.rs:36`), and that timing is the only reason
  `protolens_internal.*` wrappers do not already pollute the
  lexicographic candidate list. Recomputing it would leak every
  synthetic wrapper. S7 and S8 prepend the declared set explicitly
  instead.
- **N6. Creation from the selection pane.** `Enter` on a candidate
  applies a type that exists. Declaring is an act the reader performs
  deliberately on the command line, and keeping it there is what keeps
  G4 true.

## Specification

### The flag

- **S1.** `:override` gains `--as-new <FQDN>`, mutually exclusive with
  `--as` (a parse error naming both). `OverrideArgs` gains a `bool`
  beside the existing `r#type`; it does **not** gain a second type
  field. `--as-new foo` and `--as foo` store the identical entry —
  `r#type: Some("foo")` — because declaring is a property of the
  *invocation*, not of the entry.

  That is what makes every replay path correct at once: re-render
  passes, undo/redo, `:restore` and scripted steps all re-apply entries,
  and none of them re-declares anything.

- **S2.** `--as-new <FQDN>` registers a zero-field message named
  `<FQDN>`, then proceeds exactly as `--as <FQDN>` would. Registration
  happens before validation and before `activate`, so the type exists by
  the time `render_overrides` resolves it.

- **S3.** Declaration is idempotent: `--as-new` on an FQDN that is
  already a declared type succeeds and sets the message
  `<FQDN> already declared — reusing it`. It is not an error because the
  declaration has no content — N1 guarantees the created type is always
  the identical zero-field message, so a second declaration cannot
  conflict with the first. **If N1 is ever relaxed, this reasoning
  fails and the idempotence must be revisited.**

- **S4.** `--as-new` refuses, with a message, four kinds of name:

  1. one the pool already resolves as a real type —
     `<FQDN> already exists — use --as`. This is G4's other direction:
     the reader cannot silently shadow a published type;
  2. an override keyword (`is_override_keyword`, i.e. the 15 primitives
     plus `message`). `wrapper_target_for` asks the pool before it
     checks its keyword rungs (`decode.rs:1344`, deliberately), so a
     declared type named `bool` would silently change what `--as bool`
     means for the rest of the session;
  3. anything in the `protolens_internal` package, which is reserved for
     the wrapper descriptors and for spec 0299's synthetic;
  4. a name that is not a dot-separated sequence of protobuf
     identifiers. Without this the failure surfaces as
     `add_file_descriptor_proto`'s own error, which does not say what is
     wrong with the name.

- **S5.** The package is whatever the reader's FQDN says it is —
  `mine.Foo` declares package `mine`, `Envelope` declares no package.
  The name is not confined to a reserved prefix, because the reader's
  intent is usually "this is probably `bobapp.v1.Envelope` and I am
  reconstructing it", and a forced `local.` prefix would destroy exactly
  the meaning being expressed. Provenance is tracked by S6 instead.

### Registration and the registry

- **S6.** `DescriptorContext` gains a creation-ordered registry of
  declared FQDNs and a method that registers one. `register_synthetic`
  currently hardcodes `package: "protolens_internal"`; it takes the
  package as a parameter, derived by splitting the FQDN at its last dot.
  The package, the full name, the short name and the file name travel
  together as one `SyntheticName` value: none of the four can be derived
  from the others — a full name is *not* the package joined to the short
  name, since protobuf puts a nested message's parents in between — and
  four adjacent `&str` parameters is exactly the hazard that invites
  passing them in the wrong order.

  The file name is `protolens_new/<FQDN>.proto` — unique per declared
  type (two declarations in one package must not share a file) and, at
  the same time, the marker that makes provenance recoverable from the
  pool alone if the registry is ever unavailable.

### Where declared types are visible

- **S7.** `--as` completion offers declared types. They are prepended to
  the *lexicographic* list, not merged into the inferred one, and the
  0237 S8 sequencing rule — inferred by score first, lexicographic only
  when the inferred prefix-match yields nothing — is unchanged. A
  declared type can never be in the heat cache: the scoring database is
  built at startup from the descriptor set and does not know it.

  `--as-new` completion offers nothing. It is a declaration; `--as` is
  where an existing name is picked.

  `command_flags("override")` gains `--as-new`.

- **S8.** The selection pane lists declared types in **lexicographic
  mode only**, after the 15 primitive keywords and before the sorted
  real FQDNs, in creation order. Lexicographic-only because a declared
  type has no score and cannot acquire one; listing it among inferred
  candidates would misrepresent it as having been scored and lost. They
  go in `lexico_candidates`, which spec 0305 S2 shares with the
  cold-cache placeholder, so both lists agree.

  They are **not** added to `override_select.rs:672`'s skip list. `none`
  and `message` are skipped from the warming loop because neither has a
  file to load; a declared type has a real target descriptor and warms
  like any other FQDN.

- **S9.** `prefill_override_cmd` never emits `--as-new`. By the time an
  entry exists its type exists, so the pre-filled line spells `--as` —
  and `o`-then-`Enter` stays the no-op spec 0237 S6 promises.

### Persistence

- **S10.** The `:save` YAML gains a top-level `created_types:` list of
  declared FQDNs, alongside the existing `version` and `target` keys.
  `:restore` declares them before applying entries. Without it, every
  entry under a declared anchor restores into
  `type '<FQDN>' not found in descriptor set`.

- **S11.** `:restore` refers, it does not re-declare: S4's
  already-exists refusal is **not** applied on the restore path. If the
  real type has since appeared in the descriptor set, using it is the
  better outcome, and the reader is already warned about that situation
  by the existing descriptor-set hash mismatch.

- **S12.** `YAML_FORMAT_VERSION` stays at 1. The key is additive, absent
  from every file that does not need it, and an older build that ignores
  it fails *loudly* — a per-node "type not found" refusal (spec 0221),
  not the silently-misapplied collection the version check exists to
  prevent. Bumping would make every saved file unreadable by this build
  in exchange for no safety.

  `from_yaml` returns the list alongside the collection and the target
  hashes.

- **S13.** `origin_resolves`'s `FqdnField` arm accepts a declared type
  without asking it to declare the field.

  Its rule today is "the pool resolves the FQDN **and** that message
  declares field `n`", and a declared anchor has no fields by
  construction (N1), so every entry under one would be dropped by
  `retain_resolvable` and G6 would fail with the collection silently
  half-applied. The declared-field test is right for a real type — an
  `fqdn:field` origin naming a field the published schema does not have
  is stale — and it is meaningless for a declared one, where the origin
  *is* the schema.

  This is restore-time validation only. Render-time matching never asked
  the question: `resolve_active_override_entry_index_by_path` matches an
  `FqdnField` origin by comparing the parent's resolved type FQDN and the
  node's wire field number, both read off the tree, and consults no
  descriptor. That is why G2 needs no other change.

## Alternatives considered

### `--as-new --idempotent` (or `--allow-redo`)

Keep the strict "must not exist" check and let a caller opt out of it.
Rejected: the flag would be set in every script and in no interactive
session, which means the context already knows the answer and the flag
is asking the author to restate it. Worse, the failure mode of
forgetting it is the mid-demo error the flag exists to prevent.

What the flag would buy is a diagnostic — "you declared this twice, did
you mean `--as`?" — and S3 keeps that as a message. What it would cost
is the assumption that a declaration can conflict with itself, which N1
makes false.

### `--as message:<FQDN>` — the keyword with a name

Spell the declaration as an extension of spec 0299's keyword, storing it
as `protolens_internal.<name>` so that no synthetic FQDN ever reaches
the reader.

Rejected as strictly more expensive for no gain. It requires the stored
name and the displayed name to differ, so the origin must be projected
in both directions at roughly five sites (`override_display.rs:37`,
`origin_for_kind`'s `FqdnField` arm, `parse_origin`, the manage pane,
completion); it forces `is_override_keyword` from a closed list into a
prefix predicate, which `decode/tests.rs:56-132`'s three-way cross-check
is pinned on; and it needs `origin_for_kind`'s 0309 guard to distinguish
named from unnamed instead of comparing one constant.

A real FQDN costs none of that: `wrapper_target_for` resolves it on its
first rung, `origin_for_kind` and `override_origin_if_it_fits` guard on
`SCHEMA_FREE_MESSAGE_FQDN`/`MESSAGE_KEYWORD` specifically and so let it
through untouched, and the keyword vocabulary does not change at all.

### Auto-create any unresolvable `--as` name

No new flag: `--as mine.Foo` creates `mine.Foo` when the pool has never
heard of it.

Rejected on G4. Every misspelled FQDN would silently succeed and render
as plausible-looking numbered unknowns — the worst kind of wrong,
because nothing on screen says the reader asked for a type that does not
exist. The error message is a feature.

### A reserved package (`local.Foo`, `?.Foo`)

Confine declared types to a prefix, so they are trivially identifiable
and can never collide with a real type.

Rejected on S5's grounds: it destroys the reader's ability to say "I
believe this is `bobapp.v1.Envelope`", which is usually the whole
content of the act. S6's file-name marker gives the identifiability
without touching the name.

### Reusing `google.protobuf.Empty` as the shape

Already rejected by spec 0299 and rejected again here for the same
reasons: not guaranteed present in every pool, and it puts a real,
misleading FQDN in view.

## Test plan

1. `as_new_declares_a_type_the_pool_did_not_have` — `--as-new mine.Foo`
   on a node; the pool resolves `mine.Foo`, the entry stores
   `r#type: Some("mine.Foo")`, and the node renders its payload as
   numbered unknowns, identical to what `--as message` produces for the
   same bytes.
2. `as_new_twice_is_not_an_error` — the second invocation succeeds, sets
   the "reusing it" message, and leaves exactly one registration. This
   is G3 and the reason scripted steps replay.
3. `as_new_refuses_a_real_type` — `--as-new google.protobuf.Duration`
   against a pool containing it errors and names `--as`.
4. `as_new_refuses_a_keyword_and_the_internal_package` — `--as-new bool`
   and `--as-new protolens_internal.foo` both error; afterwards
   `--as bool` still resolves to the primitive.
5. `as_new_and_as_are_mutually_exclusive` — a parse error naming both.
6. `a_declared_type_anchors_an_fqdn_field_origin` — the one that matters.
   Override a parent with `--as-new mine.Foo`, then a child; the default
   origin ladder (spec 0308) yields `mine.Foo:<n>`, and a second,
   structurally identical node elsewhere in the document is covered by
   that one entry. Contrast: the same sequence under `--as message`
   yields `path:field`, per spec 0309.
7. `declared_types_complete_and_list` — `--as mi<Tab>` offers
   `mine.Foo`; `lexico_candidates` contains it after the primitives and
   before the sorted FQDNs; the inferred list does not contain it.
8. `save_restore_round_trips_a_declared_type` — a collection using
   `mine.Foo` saved and restored into a fresh `App` re-declares the type
   and re-applies every entry, with no "not found in descriptor set"
   refusal. Also asserts a v1 file *without* `created_types` still
   loads (S12).
9. `an_old_saved_file_still_loads` — the fixture from
   `from_yaml_accepts_an_entry_with_no_type_key` is unaffected.

## Measured outcome

Implemented 2026-08-17. All eight new tests pass; the protolens suite is
1094 + 25 + 3 green, up 8 from 1086.

**G2 cost nothing, as predicted, and for a reason worth recording.**
Render-time `fqdn:field` matching
(`resolve_active_override_entry_index_by_path`) compares the parent's
resolved type FQDN string against the node's wire field number, both read
off the tree, and consults no descriptor at all. A zero-field anchor
therefore matches its children exactly as a real message would, and not
one line of the render path changed.

**The one thing the design did not foresee** is S13, added during
implementation: `origin_resolves` — restore-time only — did ask the named
message to declare the field, so without it every entry under a declared
anchor would have been silently dropped by `retain_resolvable` and G6
would have failed with a half-applied collection. The asymmetry between
the two paths (render matches by label, restore validates against the
schema) is the whole content of that item.

**Two behaviors observed rather than specified**, both consistent with
S8:

- `t` on a node already overridden to a declared type opens the pane in
  **lexicographic** mode. Spec 0139's ladder looks for the current type
  in the inferred list first, does not find it — it cannot be there — and
  falls back to the fixed universe, which does contain it. The pane
  therefore lands highlighted on the declared type with no extra rule.
- A declared type is offered by `--as` completion from the lexicographic
  branch only, so the 0237 S8 sequencing is untouched: a prefix that also
  matches a scored real type still gets the scored one first.

**`YAML_FORMAT_VERSION` stayed at 1** (S12) and the pre-0315 fixtures
load unchanged, including `from_yaml_accepts_an_entry_with_no_type_key`.
`created_types` is `skip_serializing_if = "Vec::is_empty"`, so a session
that declares nothing writes a byte-identical file to before.
