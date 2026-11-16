<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0001 — every type name reproto writes resolves to its type

App: reproto, protoback
Spec: docs/specs/0434-type-names-as-protoc-resolves-them.md

## The guarantee

Every type reference reproto or protoback writes in a `.proto` file (a
field's type, a map's key and value types, an extendee, a method's input
and output types) resolves, under protoc's name resolution, to the type
the input `FileDescriptorProto` names. Compiling the output yields the
same type names, extendees and method types as the input.

It holds for every reference whose target a loaded file defines. A target
no loaded file defines (a missing import) cannot be checked, and protoc
fails on it anyway. The variant's namespace rewriting is outside it (spec
0434 N2).

## Why it holds

The name written is checked by simulating protoc's lookup
(`LookupSymbolNoPlaceholder`) over the symbols visible from the file;
when no short name resolves to the target, the leading-dot full name is
written, which always does. The simulation sees at least what protoc
sees, which is safe (spec 0434 S2).

## Regression tests

- `reproto/src/reproto/tests/test_type_names.py`: each case compiles the
  originals, renders them, recompiles the output, and compares every
  type name, extendee and method type with the original's. In `ci` for
  reproto (`reprotoTests`) and for protoback (`reprotoTestsOnProtoback`).
- The roundtrip tests (`test_roundtrip.py`), which compare the rendered
  text with the fixtures' sources.
