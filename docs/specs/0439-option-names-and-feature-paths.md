<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0439 — option names as googleapis writes them, custom features as paths

Status: implemented
Implemented in: 2026-11-17
App: reproto, protoback
Refs: docs/specs/0434-type-names-as-protoc-resolves-them.md (the lookup
        and the convention this spec applies to option names),
      docs/specs/0435-custom-editions-features.md (N1: the spelling of a
        custom feature, which this spec changes)

## Background

reproto writes a custom option's extension fully qualified with a
leading dot, and a message-valued one, a custom feature included, as one
aggregate:

```proto
option (.google.api.http) = { get: "/v1/x" };
string s = 1 [features.(.pb.cpp) = { string_type: VIEW }];
```

Measured, comments excluded:

| Corpus | Written | Count |
|---|---|---|
| googleapis 83e7037, option names | `(google.api.http)`, full, no dot | 120,262 |
| | `(.google.api.http)`, leading dot | 0 |
| | `(http)`, partial | 0 |
| googleapis, message-valued options | an aggregate, `(x.y) = { … }` | 31,680 |
| | paths, `(x.y).f = V` | 494 (two options, one field each) |
| protobuf 34.1's sources, custom features | paths, `features.(pb.cpp).f = V` | 116 |
| | an aggregate | 0 |

googleapis has no editions file, hence no custom feature.

## Goals

- **G1.** An option's extension is named as googleapis names it: in full
  without the leading dot; the dot only where protoc would resolve the
  plain name to something else.
- **G2.** A custom feature is written as protobuf writes it: a path to
  each leaf, `features.(pb.cpp).string_type = VIEW`.
- **G3.** Custom options keep their aggregate form, googleapis'.

## Non-goals

- **N1.** Names inside an aggregate's text (`[pkg.ext]`): written as
  today.
- **N2.** Built-in options.

## Specification

- **S1. The name.** An extension in an option name, `(E)` (the option's
  own, and a custom feature's, `features.(E)`), is written as spec 0434
  S4 writes a type of another package: its full name without the dot if
  protoc resolves it to the extension, else `.` and the full name. The
  lookup is `OptionInterpreter`'s: `LookupSymbol(E, name_scope)`,
  `LOOKUP_ALL`, the first scope searched being, for a file's options,
  its package (`name_scope` is `package.dummy`), for any other element's,
  the element's parent scope (`name_scope` is its full name); the symbol
  found must be the extension. An extension of the file's own package is
  in full too (googleapis: 0 partial names).
- **S2. Custom features as paths.** A custom feature `(E)` is written
  one statement per field set in its value, in field order:
  `features.(E).f = V` (standalone `option features.(E).f = V;`, among a
  field's options `features.(E).f = V,`); an empty value, `features.(E)
  = {}`. protoc admits nothing else in a feature (`feature_resolver.cc`,
  `ValidateDescriptor`, `ValidateExtension`): a singular message whose
  fields are singular enums or bools, with no oneof and no extension. A
  value that breaks those rules, which a descriptor not built by protoc
  may hold, is written as today's aggregate.
- **S3. Both tools alike.**

## Alternatives considered

- **Paths for every message-valued option** (this spec's first draft).
  googleapis writes 31,680 aggregates against 494 paths: the output would
  stop looking like the sources it recovers.
- **Keep the leading dot.** Always resolvable, but written by no one.

## Test plan

1. `tests/test_option_names.py`, against both tools: option names on a
   file, a message, a field, an enum value and a method, written without
   the dot; one whose plain name a nearer symbol captures, with it; a
   custom feature of two fields, and an empty one, as S2 writes them;
   every options message byte-identical after recompiling.
2. The roundtrip and custom-feature tests, the harness in both modes,
   googleapis rendered and recompiled (its options byte-identical).

## Measured outcome

- Test plan 1: `test_option_names.py` passes on both tools, every options
  message byte-identical after recompiling.
- Test plan 2: reproto's suite 349 passed on binary input and against
  protoback, on text input with the group case skipped; four roundtrip
  fixtures (`enum.proto`, `enum_value.proto`, `name_resolution.proto`,
  `editions_roundtrip.proto`) now spell their option names as googleapis
  does, their only difference from the output; the harness 1,668 cases
  and its graph dumps 1,654 cases identical; ci and nixpkgs-staging pass.
- googleapis, rendered and recompiled: its 7,771 files compile; of its
  99,718 options messages set, 99,709 come back byte for byte. The other
  9 lack `feature_support`, a built-in orphan dropped by design (spec 0438
  N3), set only on descriptor.proto's own feature fields.
