<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0248 — an extension resolves without loading the world

Status: draft
App: prototext-core, prototext, protolens
Refs: docs/specs/0100-message-set-expansion.md (`ext_to_file`, and the
        `"extendee/number"` loader key it introduced),
      docs/specs/0197-the-descriptor-set-is-loaded-on-demand.md (the
        lazy branch this spec repairs),
      docs/specs/0099-any-lazy-loader.md (`ANY_LOADER`, the hook this
        one is modeled on),
      docs/specs/0216-the-arena-is-a-function-of-the-bytes.md (the
        byte-derived arena, whose superset property this must not
        disturb)

## Background

On the lazy branch, an extension field never resolves. It renders as an
unknown field, with the field number in place of the name and no type.

The reproduction is the descriptor set inspecting itself:

```
protolens --descriptor-set googleapis.desc googleapis.desc export /91/16/5
```

```
lazy pool (index.rkyv present)      eager pool (index.rkyv removed)
──────────────────────────────      ─────────────────────────────────────────
options {  #@ MethodOptions = 4     options {  #@ MethodOptions = 4
  1051: "ad_break,update_mask"        [google.api.method_signature]:
  72295728 {  #@ message                "ad_break,update_mask"  #@ repeated string = 1051
    6: "/v1/{…}"  #@ string           [google.api.http] {  #@ HttpRule = 72295728
    7: "ad_break"  #@ string            patch: "/v1/{…}"  #@ string = 6
  }                                     body: "ad_break"  #@ string = 7
}                                     }
                                    }
```

Same blob, same node; only the sidecar differs. The extendee
(`google.protobuf.MethodOptions`) resolves fine — it is
`google/api/client.proto`, the file that *declares* the extension, that
is missing from the pool. It is in nobody's dependency closure: an
extension is declared by a file the extendee has never heard of.

`render_message`'s schema lookup
(`prototext-core/src/serialize/render_text/mod.rs:532-538`) is

```rust
schema.and_then(|s| {
    if let Some(f) = s.get_field(field_number as u32) { Some(FieldOrExt::Field(f)) }
    else { s.get_extension(field_number as u32).map(FieldOrExt::Ext) }
})
```

`s.get_extension` searches the pool `s` came from, and on the lazy branch
that pool holds only the root's closure.

**The index already has what is needed.** `FdsIndex::ext_to_file` maps
`"google.protobuf.MethodOptions/1051"` → `google/api/client.proto` in
O(1), decoding nothing, and `reproto`'s builder collects both top-level
and message-nested extensions (`reproto/src/reproto/build_index.py:83-93`).
`LazyPool::get_extension` (`prototext-schema/src/lazy_pool.rs:257`) already
turns that into a load. Its only callers are MessageSet expansion
(`prototext/src/run.rs:666`, `protolens/src/tui/override_resolve.rs:338`)
and explicit override operations. **Nothing consults it during ordinary
field rendering.** That is the entire defect.

Both lazy consumers are affected: `prototext`'s CLI (`run.rs:43`) and
`protolens` (`decode.rs:83`).

### How much schema an extension actually costs

Measured on `googleapis.desc` (7 771 files, 58 777 types), by decoding it
as a `FileDescriptorSet` and counting `extendee` declarations:

| | |
|---|---|
| files declaring at least one extension | **12** |
| extension declarations | **29** |
| transitive dependency closure of those 12 files | **26** (0.33% of 7 771) |

Every extendee is a `google.protobuf.*Options` message — 9 on
`MethodOptions`, 9 on `FieldOptions`, 4 each on `ServiceOptions` and
`MessageOptions`, 1 each on `FileOptions`, `EnumOptions`,
`EnumValueOptions`. This is the shape of a proto3 corpus, where the only
legal extendees are option messages.

## Goals

- **G1.** On the lazy branch, an extension field on the wire renders with
  its declared name, type and cardinality — identical to the eager
  branch, byte for byte.
- **G2.** A descriptor set's extensions cost nothing until a blob
  actually carries one. Startup stays what spec 0197 made it.
- **G3.** One mechanism serving both `prototext` and `protolens`.

## Non-goals

- **N1.** No preloading, in any form — not at `LazyPool::open`, not by
  widening `ensure_loaded`. See *Alternatives*: it is affordable on
  googleapis and unbounded in general, and G2 is the whole point of the
  lazy branch.
- **N2.** No change to `index.rkyv`'s format and no change to `reproto`.
  `ext_to_file` is already complete and already O(1). Nothing needs to
  be added to it.
- **N3.** No change to `Sink::unknown_len_is_message` or to the arena.
  The arena is built with no schema at all and must stay that way; its
  superset property (spec 0216) is what lets a schema-driven render
  produce *fewer* nodes than the arena holds, which is exactly what
  happens when an extension turns a payload the byte walk descended into
  into a scalar string.
- **N4.** `ANY_LOADER`'s manual `clear_any_loader` contract is left as
  it is. The new loader gets a `Drop` guard; retrofitting the old one is
  a separate, mechanical change.

## Specification

- **S1.** `prototext-core` gains, beside `AnyLoader`:

  ```rust
  pub type ExtLoader = Box<dyn FnMut(&str, u32) -> Option<ExtensionDescriptor>>;
  ```

  a thread-local `EXT_LOADER: RefCell<Option<ExtLoader>>`, and an
  `ExtLoaderGuard` returned by `set_ext_loader` whose `Drop` clears it.
  A guard rather than a paired `clear_*` call because this is the second
  such hook and a render can return early on a `CodecError`; leaving a
  stale loader installed hands the *next* render on that thread a
  dangling pointer.

- **S2.** `render_message`'s schema lookup gains a third arm, reached
  only when both `get_field` and `get_extension` have missed:

  ```rust
  EXT_LOADER.with(|l| l.borrow_mut().as_mut()
      .and_then(|load| load(s.full_name(), field_number as u32)))
      .map(FieldOrExt::Ext)
  ```

- **S3.** The arm is inside `schema.and_then(|s| …)`, so a schema-less
  render never reaches it. `ProbeSink` and `ArenaSink` both walk with
  `schema: None` (`arena.rs:277`, `len_field.rs:100`), and so does raw
  mode. This is what keeps N3 true without a second predicate.

- **S4.** The loader must re-resolve through the *pool*, never through
  the `MessageDescriptor` the render is holding:

  ```rust
  lazy.get_extension(extendee, number);          // loads the file
  ctx.pool().get_message_by_name(extendee)?.get_extension(number)
  ```

  `prost_reflect` adds files with `Arc::make_mut`, so a descriptor
  obtained before the load is blind to symbols registered after it. This
  is already written down at `prototext/src/run.rs:657` and is repeated
  here because it is the one way to get this silently wrong: the code
  compiles, and every extension resolves to `None`.

  It also means the loader is re-entered for *every* occurrence of the
  same extension, since `s` never gains it. `ensure_loaded` early-returns
  on `self.loaded`, so a repeat costs two hash lookups and one `format!`.
  Memoizing inside the loader is left out until measured — see the test
  plan's step 4.

- **S5.** `prototext` installs it in `install_any_loader`'s neighborhood
  (`run.rs:644`), reusing that function's raw-pointer pattern and its
  safety argument verbatim.

- **S6.** `protolens` installs it at both of its render sites — the
  document render (`decode.rs:1691`) and the override splice/preview
  render (`override_apply.rs:1182`). Both already hold `&mut
  DescriptorContext` and both derive their `wrapper_desc` *before* the
  render call, so the descriptor is owned and does not conflict with the
  loader's borrow. The second site sits inside `&mut self` and the
  closure it wraps also needs `&mut self.fqdns` and `&mut
  self.render_cache`; these are disjoint fields of `App`, which edition
  2021 closure capture handles, but it is the one place this can fail to
  compile.

- **S7.** A miss costs one `format!` and one hash lookup per unknown
  field occurrence, inside `LazyPool::get_extension`. Left as is: the
  key format is `ext_to_file`'s own, and the alternative is a second
  keying scheme in the index for no measured gain.

- **S8.** On the eager branch the loader is still installed and still
  correct — `ctx.lazy` is `None`, the load is a no-op, and the pool
  lookup returns what `s.get_extension` would have returned anyway. No
  branch on which pool is live.

## Alternatives considered

### The index carries reverse-extension edges

`reproto` adds, per file, the files declaring extensions on types that
file defines — either folded into `dep_graph` (so `ensure_loaded`
picks them up for free) or as a new `ext_providers` map. No render-time
change at all, correct by construction, and works from any thread.

Dismissed on G2. Every extendee in the corpus lives in
`google/protobuf/descriptor.proto`, which is in nearly every file's
closure — so "load what extends what I loaded" fires on essentially the
first type resolved, and the lazy pool starts by loading all 26 files
whether or not the blob has a single extension in it. On googleapis that
is 0.33% and genuinely cheap; the objection is not the cost here but
that the cost is a property of the corpus, not of the design. A proto2
corpus may extend arbitrary messages from arbitrarily many files, and
the lazy branch exists precisely so that descriptor-set size stops being
a startup cost. Trading an unbounded liability for a hook that already
has a working precedent (`ANY_LOADER`) is not a good trade.

The narrower variant — hang the reverse edges off the *extendee type*
rather than off the file — does not help: `ensure_loaded` works on
files, `descriptor.proto` arrives through the dependency DFS rather than
through `get_message`, and there is no type-granular moment to hook.

### Preload every extension-declaring file at `LazyPool::open`

The same objection without the index work. Also loses the diagnostic
property that the pool contains only what was asked for.

### Reuse `ANY_LOADER` with the `"extendee/number"` sentinel key

That key form already exists (spec 0100 §5.2) and `run.rs:662` already
parses it. But `AnyLoader` returns `Arc<MessageDescriptor>`, which is all
MessageSet expansion needs — the *payload* type. Ordinary rendering needs
the `ExtensionDescriptor` itself, for the display name, the kind and the
cardinality, and a scalar extension like `method_signature` has no
message descriptor to return at all. Widening `AnyLoader`'s return type
would make every existing caller construct something it does not have.

### Render twice

Render, collect the unknown field numbers whose parent has a known type,
load those extensions, re-render if anything loaded. No new hook and no
mutation mid-render. Dismissed on cost: protolens's document render is
multi-second on a large blob (which is why `main.rs` announces the sweep
and the render separately), so this doubles the slowest thing the app
does, to serve a case that is usually empty.

## Test plan

1. `an_extension_resolves_through_the_lazy_pool` (protolens,
   `decode/tests.rs`) — extend the existing `extension-jit` fixture,
   which already puts `ext.proto` outside `t.Root`'s closure. A blob
   carrying field 100 on a `t.Leaf` must render the extension's bracketed
   name, not `100:`. The existing
   `an_extension_jit_loads_from_outside_the_root_closure` covers the
   `LazyPool` half already and stays as it is.
2. The same fixture, asserting the loaded-file set: opening the pool and
   rendering a blob with *no* extension must leave `ext.proto` unloaded.
   This is G2, and it is the assertion the preloading alternatives fail.
3. `prototext` end-to-end: the same descriptor set decoded with and
   without its `index.rkyv` sidecar must produce byte-identical text.
   This is the reproduction above, reduced to a fixture, and it is the
   only test that would have caught the defect.
4. Measure the repeat-occurrence cost of S4 on `googleapis.desc`, which
   carries thousands of `method_signature`/`http` occurrences, before
   deciding whether the loader needs a memo.
5. `arena_gap` (`protolens/src/decode.rs:787`) over the corpus — the
   superset property, now that a schema can turn a descended payload into
   a scalar. N3 predicts no change; this is what checks it.

## Measured outcome

Filled in at implementation.
