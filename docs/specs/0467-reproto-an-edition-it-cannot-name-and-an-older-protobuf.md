<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0467 — reproto: an edition it can't name, and an older protobuf

Status: implemented
Implemented in: 2026-11-23
App: reproto
Refs: docs/specs/0457-protoback-edition-names-from-the-input.md (what
        protoback does with an edition it can't name),
      docs/specs/0460-one-protobuf-release-for-every-descriptor-copy.md
        (the release reproto is built with)

## Background

A downstream build reported reproto writing `edition = "UNKNOWN";` for an
edition 2026 file. reproto as built, on Python protobuf 7.36.2, writes
`edition = "2026";`. The report reproduces exactly when Python protobuf
7.34.1 comes first on `PYTHONPATH`: the closure holds 7.36.2, the import
is 7.34.1. That protobuf's `Edition` enum lacks 1002; the enum is closed,
so the value goes to unknown fields, `edition` reads 0, and
`_edition_name` (`syntax.py`) writes `Edition.Name(0)`'s tail. Only the
pool's W19 hints at it:

```
Warning [W19]: 'a/b.proto' : the descriptor pool refused it: File has syntax="editions", but no edition is specified
edition = "UNKNOWN";
```

protoback reports an edition it can't name as a W6 and leaves the file
unwritten (0457). Nix's Python wrapper doesn't stop an inherited
`PYTHONPATH` from taking precedence, so this can happen to any install.

## Goals

- **G1.** reproto never writes `edition = "UNKNOWN";`: an edition it can't
  name is a W6, as protoback reports it.
- **G2.** An older Python protobuf than reproto's own built-in
  descriptors is said at startup, whatever caused it.

## Non-goals

- **N1.** Reading an edition the imported protobuf lacks from the bytes,
  as protoback does (0457): reproto decodes through Python protobuf, and
  the fix for an older one is the environment (G2), not a second decoder.
- **N2.** Refusing to run on an older protobuf: most inputs need nothing
  it lacks.

## Specification

- **S1. An edition not named.** A file with `syntax: "editions"` whose
  edition is unset, 0, or has no name in the imported `Edition` enum is
  a W6, `'a/b.proto': edition 1002 has no name in this Python protobuf
  (7.34.1)` (or `the edition is unset`), with `file` and `reason`, as
  protoback's (0457 S2); the file isn't written. The wording names the
  Python protobuf, where protoback's names the `descriptor.proto`: no
  case of the differential harness has an edition either tool can't
  name. The value is read from
  the file's unknown fields when the enum dropped it (field 14, a
  varint), so that the message names it.

- **S2. At startup**, reproto compares the `Edition` values of its
  built-in `descriptor.proto` (the variant's, 0460) with the imported
  `descriptor_pb2.Edition`: if the built-in has values the import lacks,
  a W37, once: `Python protobuf 7.34.1 (…/site-packages/google/protobuf)
  is older than reproto's descriptor.proto (protobuf 36.2): edition 2026
  can't be read; check PYTHONPATH`. The same in `reproto --version`'s
  output. W37 is declared in both tools' code tables, which stay alike
  (`--help`, the man pages); only reproto emits it.

- **S3.** protoback is unchanged: it has no Python, and names editions
  from the bytes (0457).

## Alternatives considered

**Comparing version numbers** (`google.protobuf.__version__` against a
release recorded at build time). The enums say what matters, an edition
the import can't read, and need nothing recorded.

**Making the Nix wrapper clear `PYTHONPATH`.** It would hide the cause
from installs outside Nix, and break the dev-shell's use of it.

## Test plan

1. `test_an_unnamed_edition_is_a_w6` — an editions file with edition
   99997 (a value no release names): W6 with its reason, the file not
   written, no `UNKNOWN` anywhere.
2. `test_an_older_protobuf_is_said` — reproto run with a `PYTHONPATH`
   holding an `Edition` enum without a value the built-in has (a stub
   `descriptor_pb2` for the test): the W37 once, naming its path.
3. The suite, on reproto and on protoback, unchanged otherwise.

## Measured outcome

2026-11-23. The downstream report reproduced with Python protobuf 7.34.1
first on `PYTHONPATH`, and not otherwise; reproto as built writes
`edition = "2026";`.

The tests pass: an edition 1003 file is a W6 on both tools, the other
file written, `UNKNOWN` nowhere; W37 with the built-in enum given a value
the imported one lacks. reproto's suite, on reproto and on protoback,
passes (`--version`'s test reads its first line, then the protobuf one).

The dropped value is read from the serialized message
(`_dropped_edition`): types-protobuf 6.32.1 has no stub for
`google.protobuf.unknown_fields`. `unknown-fields-stub-check`, in `ci`,
fails when one appears, naming what to switch back.
