<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0434 — type names as protoc resolves them

Status: implemented
Implemented in: 2026-11-16
App: reproto, protoback
Refs: docs/specs/0424-protoback-reproto-in-rust-on-upb.md (protoback
        reproduces reproto's output; both tools change together),
      docs/specs/0429-protoback-rendering.md (S17: the variant's
        namespace rewriting, which this spec leaves as it is),
      docs/invariants/0001-reproto-type-names-resolve.md (the guarantee)

## Background

reproto wrote a type reference shorter than its full name when it
believed protoc would resolve the short form back to the same type. The
belief was wrong five ways, each reproduced with a stock protoc and the
default variant: the original files compile, the output did not.

| Case | Shape | Written | protoc |
|---|---|---|---|
| A | `package alpha.beta;` declares `Thing` and `extend .alpha.Thing` | `extend Thing` | `alpha.beta.Thing` does not declare the number |
| B | in `p.q.r.use`, a field of type `.p.q.inner.Source`, `p.q.r.inner` a package imported | `inner.Source` | resolved to `p.q.r.inner.Source`, not defined |
| C | in `x.y`, a field of type `.top.kinds.Kind`, `x.y.top` a package imported | `top.kinds.Kind` | resolved to `x.y.top.kinds.Kind`, not defined |
| D | `rpc DoIt(.svc.pkg.DoIt)`, the method named as its message | `rpc DoIt(DoIt)` | `DoIt` is not a message type |
| E | as A, the shadowing message in another file of the package | `extend Request` | `quota.pb.Request` does not declare the number |

protoback had the same five, being byte-identical with reproto. Two
functions shortened names, each with its own model of protoc: `short_ref`
checked a candidate against nodes of the target's own kind only, so a
package (B, C) never shadowed it, and an extendee came as `message:…`, a
prefix no node has (messages are `desc:…`), so its check never fired (A,
E); `shorten_type_name`, for method types, stripped the package and
checked nothing (D).

## The physics: how protoc resolves a type name

Not in protoc's documentation, which says only that names resolve as in
C++, innermost scope first. The rule is in `descriptor.cc` (protobuf
34.1, the protoc both tools are tested with):

- **The walk** (`DescriptorBuilder::LookupSymbolNoPlaceholder`). A name
  with a leading `.` is looked up from the root. Otherwise, from the
  reference's own full name, each enclosing scope in turn, outward, is
  searched for the name's *first component* only; at the root, the whole
  name.
- **The first hit decides.** A dotted name commits to the first hit that
  is an aggregate (`Symbol::IsAggregate`: message, enum, service,
  package) and fails if the rest is not under it ("resolved to …, which
  is not defined"); a non-aggregate hit is passed over. A single name
  returns the first hit, except that under `LOOKUP_TYPES` a hit that is
  not a message or enum is passed over.
- **The mode.** A field's type: `LOOKUP_TYPES` (`CrossLinkField`). An
  extendee, and a method's input and output types: `LOOKUP_ALL`
  (`CrossLinkField`, `CrossLinkMethod`); a field, oneof, enum value,
  extension or method shadows them.
- **The scope.** The reference's full name, minus its last component: a
  field's message; an extension's message or package (where it is
  declared, not its extendee); a method's service; a map field's value,
  the synthesized entry message, where the parser declares it.
- **What is visible** (`FindSymbol`). Symbols of the file itself and of
  the files it imports, with their `import public` chains
  (`RecordPublicDependencies`); a package when the file or one of those
  imports is in it or under it (`IsInPackage`). Every symbol counts:
  packages and their parents, messages, enums, enum values (in the
  enum's enclosing scope), fields, oneofs, extensions, services, methods.

## The convention: how googleapis writes type names

Measured on googleapis at 83e7037 (7,760 files, `preview/` excluded),
comments excluded, by each reference's source span (protoc
`--include_source_info`):

| The target's package, from the file's | Written in full, no dot | Partially qualified | Leading dot |
|---|---|---|---|
| another package (`google.protobuf`, `google.type`, …) | 10,536 | 0 | 0 |
| a parent package | 31 | 1 | 0 |
| a sub-package | 33 | 0 | 0 |

1,338 more references wrap across lines and were not classified; their
first lines begin with full package names. The file's own package's
types are written short, relative to the scope (99 qualified in a sample
of 3,000 files). The leading dot appears twice in the whole corpus, both
in a scope that would capture the plain name.

## Goals

- **G1.** Every type reference either tool writes resolves, under
  protoc's rule, to the type the FDP names: field types (map values
  included), extendees, method input and output types.
- **G2.** Names written as googleapis writes them (S4).
- **G3.** One implementation of the rule in each tool, replacing
  `short_ref` and `shorten_type_name`; the harness stays identical.
- **G4.** The guarantee recorded as an invariant, with its regression
  tests.

## Non-goals

- **N1.** Option names. A custom option is written fully qualified with
  its leading dot (`canonize_opt_name`), which is always correct.
- **N2.** The variant's namespace rewriting: a rewritten name still comes
  out whole, before any of this. Its redesign is separate, after the
  port (spec 0429 S17).
  *Done by spec 0440:* names are now resolved in the output namespace,
  rewritten ones included.
- **N3.** Custom editions features dropped from the output: a defect of
  the features renderer, with its own spec.

## Specification

- **S1. The symbol table** (`names.py`'s `SymbolTable`, protoback's
  `render/names.rs`): every symbol of the physics, by full name, with
  its kind in each loaded file defining it; built once per run.
- **S2. Visibility**: per file rendered, the physics' rule over the input
  FDP's imports. The rendered file imports a subset of these (pruning
  only removes), and a table larger than protoc's is safe: a name is
  accepted only when the simulated lookup reaches the target, and
  protoc, seeing fewer symbols in the scopes inner to the target's,
  reaches it too.
- **S3. The lookup**, simulated: `resolve(name, scope, mode)`, the walk
  of the physics over S2's symbols, from the scope it gives each kind of
  reference.
- **S4. The name written**, googleapis' convention:
  - a type of the file's own package (the package of a file defining it,
    not a prefix of its name: a sub-package is another package): the
    shortest suffix of its name within the package (`C`, `B.C`, …) that
    resolves to it;
  - any other package's type, well-known types included: its full name
    without the dot, if it resolves to it;
  - otherwise: `.` and the full name;
  - a target no loaded file defines (a missing import; a group the
    output synthesizes) cannot be checked: relative to the innermost
    scope enclosing it, else by the convention alone.
- **S5. One entry point**, `utils.type_ref` (protoback
  `names::type_ref`): the variant's rewriting (N2), then S4. A group's
  declaration writes its message's own name: it declares, it does not
  refer.

## Alternatives considered

- **Qualify every name.** Always correct, but the output stops looking
  like the source it recovers: googleapis writes its own package's types
  short.
- **The shortest name that resolves, whatever its package.** Writes
  `protobuf.Timestamp` and `type.Date` from a `google.*` package, which
  googleapis never does (the table above); and a name's spelling would
  depend on the reader's package.
- **Fix the three causes in place.** Each fix keeps a model of protoc
  that is not protoc's: the old check compared whole paths, protoc
  commits on the first component.
- **Every loaded file visible.** Safe by S2's argument and simpler, but a
  file's names would depend on unrelated files of the set.

## Test plan

1. `tests/test_type_names.py`, run against both tools (`REPROTO`): cases
   A to E, an extendee shadowed by a field and by an enum value, the same
   in the extendee's own package, a map value named as its entry. Each
   compiles the original, renders it, recompiles the output, and checks
   every type name, extendee and method type against the original's.
2. The same file's controls: the convention's spellings, the leading dot
   where a name is captured.
3. The harness, every corpus: both tools identical.
4. googleapis, rendered and recompiled: every reference against the
   original's.

## Measured outcome

- Test plan 1 and 2: 12 tests; 10 failed before the change, all pass on
  both tools; reproto's suite 332 passed. One fixture
  (`editions_roundtrip.proto`) wrote a parent package's type short,
  against the convention; it now writes it in full.
- Test plan 3: 1,668 cases, identical.
- Test plan 4: the 7,771 files rendered compile with protoc 34.1; all
  90,692 type references (types, extendees, method types) equal the
  original's.
- Against the previous output, on googleapis: 3,412 files changed;
  8,930 other packages' types and 8,899 well-known types written in full
  (`type.Interval`, `protobuf.Timestamp` before), 2,832 method types
  without their leading dot, 159 `rpc` lines rewrapped as they now fit,
  and 2 leading dots added (`.google.cloud.osconfig.agentendpoint.v1.OSPolicy.Mode`),
  at the two places googleapis' own source writes one.
