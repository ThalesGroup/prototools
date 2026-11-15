<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0159 — reproto: variant namespace rewriting leaves a file's own `package` stale, producing self-inconsistent descriptors

Status: implemented
App: reproto
Implemented in: 2026-07-22

## Background

`reproto`'s variant mechanism supports `namespace_rewrites` rules
(`ctx.variant_ns_rules`, `apply_variant_namespace()` in `mappings.py`) that
rewrite type references — a `prefix → prefix` substitution, e.g.
`.proto2. → .google.protobuf.` — applied to every `type_name` / `extendee`
/ `input_type` / `output_type` a rendered `FieldDescriptorProto` or
`MethodDescriptorProto` carries. These call sites
(`re_field.py:481,483`, `re_method.py:126,128`) apply the rewrite
unconditionally to every reference they see, regardless of whether that
reference crosses files or points at a sibling declared in the very same
file.

Meanwhile, nothing in the codebase ever rewrites a file's own `package`
field: `FileDescriptorProto.package` is copied verbatim from the parsed
input both in the text-render path (`self.package`, used at the
`package X;` emission, `re_file.py:341-342`) and in the binary
side-channel (`fdp_out.CopyFrom(self.this)`, `re_file.py:504-513` — only
`fdp_out.name` and `fdp_out.dependency[i]` are canonized there, via
`canonize_dependency`, which rewrites *paths* per `import_rewrites`, a
separate rule set from `variant_ns_rules`).

For a file whose own declared `package` happens to match a
`namespace_rewrites` rule, and which contains at least one type reference
back into that same package (either to a sibling declared in the same
file, or to another file that keeps the *original*, un-rewritten
package), this produces a rendered `FileDescriptorProto` that is
internally self-inconsistent: its `package` field says the old
namespace, but its own fields' `type_name`/`extendee` say the new one —
referencing a symbol that is declared nowhere in the produced
`FileDescriptorSet`. Loading such a set into a real
`google.protobuf.descriptor_pool.DescriptorPool` (or
`prost_reflect::DescriptorPool::decode` in `protolens`/`prototext`)
fails, e.g.:

```
TypeError: Couldn't build proto file into descriptor pool: couldn't resolve name '.google.protobuf.FileDescriptorProto'
```

The canonical, and most severe, instance of this is a variant's own
`descriptor_proto:` file (e.g. a corpus using
`proto2/descriptor.proto`, package `proto2`, with a
`namespace_rewrites` rule `.proto2. → .google.protobuf.`): *every* field
in that file is a same-file, same-package self-reference (`DescriptorProto`
referencing `FieldDescriptorProto`, `FileDescriptorProto` referencing
`DescriptorProto`, etc.), so the file's rendered output ends up with
`package proto2;`/`fdp_out.package == "proto2"` while every single field
`type_name` reads `.google.protobuf.*` — a message that doesn't exist
anywhere in the produced set. This is how the bug was found downstream in
`protolens`, loading a `--schema-db-out`-produced `.desc`:

```
error: descriptor '.../protodb.desc': invalid descriptor: name '.google.protobuf.FileDescriptorProto' is not defined
```

`--keep-descriptor-path` (`ctx.keep_variant_descriptor`) is not a
workaround: it correctly suppresses `canonize_dependency`'s path rewrite
for the descriptor_proto file's own `.name`/incoming `.dependency`
entries (`mappings.py:198`), and correctly suppresses
`apply_variant_namespace`'s rewrite in the *text*-render type-shortening
path (`utils.py:172-173`, `utils.py:312-313` — both already guarded by
`if not ctx.keep_variant_descriptor:`), but the *binary* side-channel's
`apply_variant_namespace` calls (`re_field.py:481,483`,
`re_method.py:126,128`) have no such guard and rewrite unconditionally
regardless of the flag. So with `--keep-descriptor-path`, the file's
`.name` and `package` both stay in the original namespace but its binary
`type_name`/`extendee` fields are *still* rewritten to the new one — a
different, but equally invalid, mismatch.

Note this is not limited to a variant's own `descriptor_proto:` file: any
file whose own `package` matches a `namespace_rewrites` rule and that
contains a same-package self-reference is affected identically. This is
already latent (though currently unasserted) in this repo's own
`test_emit_scoring_graphs.py::test_TC6_canonized_output_paths`: its
`phone_number.proto` fixture (`package tutorial;`) declares a nested enum
`PhoneNumber.PhoneType`, referenced from within the same file — under the
test's `.tutorial. → .canonical.tutorial.` rule, the compiled
`type_name` for that field already renders as
`.canonical.tutorial.PhoneNumber.PhoneType` while `fdp_out.package`
renders as `tutorial` (unrewritten) — the test just never asserts on
`fdp.package` or loads the result into a real `DescriptorPool`, so it
does not currently catch this.

`apply_variant_namespace` has one more call site,
`syntax.py:329` (`allow_extend_block`), which is unaffected: it is a
predicate query ("is this extendee — canonically — one of the nine
`*Options` message types") used to decide proto3 extend-block legality,
not an output-emission rewrite, so it has no bearing on this bug.

## Goals

- **G1.** A rendered `FileDescriptorProto`'s own `package` field is
  rewritten through `ctx.variant_ns_rules` (the same rules
  `apply_variant_namespace` already applies to type references),
  treating `package` as an implicit `.` + `package` + `.` for matching
  purposes — applied uniformly to *every* file (not special-cased to the
  variant's own `descriptor_proto:` file), at both output points:
  - the text-render `package X;` statement (`re_file.py`, around line
    341-342),
  - the binary side-channel's `fdp_out.package`
    (`re_file.py`, around line 504-513).

  This does **not** change any internal bookkeeping — `self.package`
  itself, `self.prefix`, `self.ref`, `self.type_name`, and
  `_initialize_from_message`'s use of `self.package` for FQDN
  registration/scope-tracking remain on the *original* namespace
  throughout, exactly as `canonize_dependency` already leaves
  `self.name`'s internal use untouched and only rewrites the value at
  the specific emission point.

- **G2.** `re_field.py`'s and `re_method.py`'s binary side-channel
  `apply_variant_namespace` calls (`type_name`/`extendee`/`input_type`/
  `output_type`) are guarded by `not ctx.keep_variant_descriptor`,
  matching the guard already present on the equivalent text-render calls
  in `utils.py` (`parse_fqdn`, `shorten_type_name`). This brings the
  binary and text output paths into agreement about whether namespace
  rewriting is active at all.

- **G3.** With G1 and G2 together, every rendered file is internally
  self-consistent in both configurations, for *every* file (not just a
  variant's own `descriptor_proto:`):
  - default (`keep_variant_descriptor == False`): `.name`, `package`, and
    all type references are rewritten together into the new namespace —
    loading the result into a `DescriptorPool` succeeds.
  - `--keep-descriptor-path` (`keep_variant_descriptor == True`):
    `.name`, `package`, and all type references are *all* left in the
    original namespace — loading the result into a `DescriptorPool`
    succeeds too, under the original names.

## Non-goals

- N1: No change to `import_rewrites`/`canonize_dependency`'s existing
  path-rewriting logic for `.name`/`.dependency` — untouched, still the
  correct mechanism for file *paths*, entirely separate from
  `namespace_rewrites`/`package`.
- N2: No attempt to *shorten* a rewritten type reference relative to the
  new (rewritten) package in `.proto` text output.
  `shorten_type_name`'s existing behavior of returning the full
  rewritten FQDN unshortened whenever `apply_variant_namespace` changes
  the name (`utils.py:313-315`) is unchanged — this spec fixes validity/
  self-consistency, not verbosity.
- N3: No change to `syntax.py:329`'s `allow_extend_block` use of
  `apply_variant_namespace` — it is a predicate, not an output-emission
  rewrite, and is unaffected by this bug (see Background).
- N4: No change to `canonize_dependency`'s existing path-level
  `--keep-descriptor-path` semantics (`mappings.py:198`) — this spec adds
  an analogous namespace-level guard using the same
  `ctx.keep_variant_descriptor` flag at the two previously-unguarded
  binary-side call sites, it does not touch the existing path-level one.
- N5: No new collision detection for the package rewrite's output. Spec
  0158's schema-db canonical-name collision check already covers any
  resulting `.name` collision generically, downstream of this fix; no
  separate mechanism is needed for `package` values.
- N6: No change to `_phase6_summoning`/`_phase7_output`'s summoning,
  suppression, or ordering logic (specs 0056/0150) — this is purely a
  content-rewriting fix at the two existing emission points.

## Specification

### `reproto/src/reproto/mappings.py`

New helper, placed after `apply_variant_namespace`:

```python
def apply_variant_namespace_to_package(ctx: Context, package: str) -> str:
    """Rewrite a file's own `package` field through the same
    variant_ns_rules that apply_variant_namespace applies to type
    references, treating `package` as an implicit '.' + package + '.'
    for matching purposes (spec 0159).

    Callers are responsible for checking ctx.keep_variant_descriptor
    first — this function itself applies the rules unconditionally,
    matching apply_variant_namespace's own unconditional-application
    contract.
    """
    if not package:
        return package
    rewritten = str(apply_variant_namespace(ctx, Ref(f'.{package}.')))
    return rewritten.strip('.')
```

### `reproto/src/reproto/re_file.py`

Import the new helper alongside the existing `canonize_dependency`
import (top of file):

```python
from .mappings import apply_variant_namespace_to_package, canonize_dependency
```

Text-render `package` statement (around line 341-342):

```python
        # Package
        if self.package != "":
            out_package = (
                self.package if ctx.keep_variant_descriptor
                else apply_variant_namespace_to_package(ctx, self.package)
            )
            out.append(BlockLine(f'package {out_package};', depth))
            out.append(BlockLine('', depth))
        out.append_div_maybe(depth)
```

Binary side-channel (around line 509-513), right after the existing
`.name`/`.dependency` canonization:

```python
            fdp_out.name = canonize_dependency(ctx, fdp_out.name)
            for i, dep in enumerate(fdp_out.dependency):
                fdp_out.dependency[i] = canonize_dependency(ctx, dep)
            # package: canonize via variant namespace rules, so
            # self-referencing type names (rewritten below and in
            # ReFieldDescriptorProto/ReMethodDescriptorProto) stay
            # consistent with this file's own declared package (spec 0159)
            if not ctx.keep_variant_descriptor:
                fdp_out.package = apply_variant_namespace_to_package(ctx, fdp_out.package)
```

### `reproto/src/reproto/re_field.py`

Guard the existing binary side-channel rewrite (around line 477-483):

```python
            # type_name / extendee: canonize via variant namespace rules
            # (spec 0086), unless --keep-descriptor-path suppresses
            # namespace rewriting entirely (spec 0159 — matches the
            # existing precedent in utils.py's shorten_type_name/
            # parse_fqdn for the text-render path)
            if not ctx.keep_variant_descriptor:
                from .mappings import apply_variant_namespace
                from .fake_types import Ref as _Ref
                if field_out.type_name:
                    field_out.type_name = str(apply_variant_namespace(ctx, _Ref(field_out.type_name)))
                if field_out.extendee:
                    field_out.extendee = str(apply_variant_namespace(ctx, _Ref(field_out.extendee)))
```

### `reproto/src/reproto/re_method.py`

Same guard, around line 122-128:

```python
            # input_type / output_type: canonize via variant namespace
            # rules (spec 0086), unless --keep-descriptor-path suppresses
            # namespace rewriting entirely (spec 0159)
            if not ctx.keep_variant_descriptor:
                from .mappings import apply_variant_namespace
                from .fake_types import Ref as _Ref
                if method_out.input_type:
                    method_out.input_type = str(apply_variant_namespace(ctx, _Ref(method_out.input_type)))
                if method_out.output_type:
                    method_out.output_type = str(apply_variant_namespace(ctx, _Ref(method_out.output_type)))
```

## Test plan

- New test module `reproto/src/reproto/tests/test_variant_package_rewrite.py`
  (self-contained, no dependency on external/private fixtures — mirrors
  `test_schema_db_collision.py`'s protoc-based harness):
  - **G1/G3 default-mode regression** (the reported bug): a minimal
    `legacy/proto/schema.proto` (`package legacy;`, message `Outer`
    referencing sibling message `Inner` declared in the same file) used
    as a variant's own `descriptor_proto:`, with `import_rewrites`
    `legacy/proto/ → canonical/` and `namespace_rewrites`
    `.legacy. → .canonical.`, plus a second file `client.proto`
    (`import "legacy/proto/schema.proto"`, referencing `legacy.Outer`
    from a different package). Run `reproto --schema-db-out=...`, then
    load the resulting `.desc` into a fresh
    `google.protobuf.descriptor_pool.DescriptorPool()` and assert it
    succeeds; assert `canonical.Outer`'s `inner` field resolves to a
    `canonical.Inner` message declared in the same set (today this
    fails: `Outer.inner.type_name` becomes `.canonical.Inner` while the
    message is still declared under `.legacy.Inner`).
  - **G2 `--keep-descriptor-path` regression**: same fixtures, same
    variant, run with `--keep-descriptor-path` added. Assert the run
    succeeds, `.desc`'s `schema.proto` entry keeps `.name ==
    "legacy/proto/schema.proto"` and `package == "legacy"`, and
    `Outer.inner.type_name == ".legacy.Inner"` (no rewriting at all —
    fully self-consistent under the *original* namespace). Today this
    configuration is the "doubly-broken" state from the bug report:
    `.name`/`package` stay `legacy`-rooted but `type_name` is still
    wrongly rewritten to `.canonical.Inner`.
- Extend `test_emit_scoring_graphs.py::test_TC6_canonized_output_paths`
  (§86.4 area) with two new assertions on the already-produced
  `phone_number.pb` (no fixture changes needed — the self-reference is
  already present via `PhoneNumber.PhoneType`, just currently
  unasserted):
  - `pn_fdp.package == "canonical.tutorial"` (was `"tutorial"`,
    unrewritten, before this fix),
  - the `type` field's `type_name` on `PhoneNumber` starts with
    `.canonical.tutorial.` (already true before this fix — asserting it
    alongside the new `package` assertion documents that both are now
    consistent with each other).
- Regression: existing `test_schema_db.py`, `test_schema_db_collision.py`,
  and `test_variant.py` tests continue to pass unchanged — none of them
  assert on `FileDescriptorProto.package`'s rewritten value, and none of
  them exercise `--keep-descriptor-path` together with a non-empty
  `namespace_rewrites` rule set today.
- Regression: run the existing `test_emit_scoring_graphs.py` and
  `test_roundtrip.py` suites in full — G1's package rewrite is new
  behavior only when `namespace_rewrites` is non-empty (the default
  built-in `google-protobuf` variant ships `namespace_rewrites: []`, so
  `apply_variant_namespace`/`apply_variant_namespace_to_package` are
  no-ops for it — no behavior change expected for any test not using a
  custom variant with non-empty `namespace_rewrites`).
