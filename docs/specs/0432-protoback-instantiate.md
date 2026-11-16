<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0432 — protoback instantiate

Status: implemented
Implemented in: 2026-11-16
App: protoback, reproto
Refs: docs/specs/0424-protoback-reproto-in-rust-on-upb.md (step 8 of its
      work plan; S10, instances reproduced; test plan 6);
      docs/specs/0063-reproto-instantiate-schema.md (the command)

## Background

`reproto-instantiate-schema` writes one pseudo-random binary instance
per message type named, from a `FileDescriptorSet`
(`instantiate.py`, `instantiate_cli.py`). googleapisDb writes 400 of
them, which the stress tests read. Each type gets its own generator,
CPython's `random.Random` seeded with the SHA-256 of `"SEED:.FQDN"`; the
walk draws from it in a fixed order:

- each real oneof (a oneof is taken as synthetic when it holds one field
  and is named `_` + that field's name): `random() < p` to fill it, then
  `choice` of its field;
- each other field, in declaration order: a repeated one draws its count
  (`randint(1 if depth 0 else 0, max(1, round(p * max_repeated)))`), a
  required one is always set, an optional one when `random() < p`;
- `p = (max_depth - depth) / max_depth`; a message field at `max_depth`
  is not generated;
- values: an enum's non-zero values by `choice`, `True`, `s` +
  `randint(1, 9999)`, 1 to 8 random bytes, `uniform(1.0, 1000.0)` for
  floats, `randint(1, 1000)` for every integer type;
- a map entry is generated as a message, then assigned by its key: a
  repeated key keeps the last value, a key not generated is the default.

The message is built with Python protobuf's classes and serialized with
`SerializeToString()`; an empty one is not written.

Two defects of reproto's, found by runs (2026-11-16):

1. **Map order is not reproducible.** upb seeds its string hash per
   process, and `SerializeToString()` writes a map in its hash table's
   order: a map with two string keys or more comes out in either order
   from one run to the next (`google.api.MonitoredResourceMetadata`,
   seed 0: two orders over four runs). Its instances cannot be compared
   byte for byte, and `--seed` does not make them reproducible.
2. **Singular groups are never set.** A singular group (proto2, or an
   editions field with delimited encoding) is assigned with `setattr`,
   which Python protobuf refuses for a message; the `AttributeError` is
   swallowed. Its value's draws are made, the field is left out. A
   repeated group is appended, and set.

## Goals

- **G1.** `protoback instantiate` writes reproto's instances, byte for
  byte, for every type of the custom and googleapis databases.
- **G2.** CPython's `random.Random`, reproduced: a million draws of each
  method against CPython's, for a few seeds.
- **G3.** The harness compares instances: the custom database's types in
  `ci`, googleapis' 400 in `full-tests`.

## Non-goals

- **N1.** A generator of protoback's own (other distributions, other
  values): the instances are reproto's.

## Specification

- **S1. The command.** `protoback instantiate`, a subcommand, with
  `reproto-instantiate-schema`'s options: `--descriptor-set` (and the
  deprecated `--descriptor`), `PROTOTEXT_DESCRIPTOR_SET` (and the
  deprecated `PROTOTEXT_DEFAULT_DESCRIPTOR`), `-O`, `--seed`,
  `--max-depth`, `--max-repeated`, `-q`, the FQDNs; its messages and
  exit statuses; `ROOT/a/b/C.pb` per type, its path on stdout.
- **S2. The generator.** MT19937 seeded by `init_by_array` over the
  32-bit words of the seed integer, little end first; `getrandbits`,
  `_randbelow`'s rejection, `random()` from two draws (53 bits),
  `randint`, `choice`, `uniform` as CPython defines them (spec 0424
  S10).
- **S3. The pool: the set's own `descriptor.proto`, fixed in both
  tools.** Each tool loads the set into a fresh pool, each file added in
  order, one the pool refuses skipped; a built-in `descriptor.proto` is
  added first only for a set without one (Python protobuf's in reproto,
  the vendored one in protoback, which may differ). reproto loaded the
  set into Python's default pool, which holds the `descriptor.proto` of
  the installed Python protobuf: the set's own copy, conflicting, was
  skipped, so the instances of `descriptor.proto`'s types followed
  whatever version was installed (googleapis: 24 of 43,111 instances,
  the vendored v36.2 adding `EDITION_2026` to Python 7.34.1's). A third
  defect fixed in both tools, by the user's decision.
- **S4. The walk.** reproto's, draw for draw (Background), on upb's
  definitions through `prototext-upb`, building a `DynamicMessage`.
- **S5. Map order, fixed in both tools.** An instance is serialized
  deterministically: reproto's `SerializeToString(deterministic=True)`,
  protoback's upb encoding with `kUpb_EncodeOption_Deterministic`
  (`prototext-upb`'s `encode_deterministic`). Map entries are written in
  upb's deterministic order, the same in both tools, which share upb's
  encoder: by key, ascending, except that a string key that is a prefix
  of another comes after it (upb's comparator negates `memcmp` but not
  the length tie-break, and its encoder writes back to front); the rest
  is unchanged. An instance has no original
  order to keep: it is generated, so a fixed one is all that is needed,
  and sorted is the canonical one.
- **S6. Singular groups, fixed in both tools.** A singular group (and an
  editions field with delimited encoding) is set as a singular message
  is, by `CopyFrom`, in a oneof, required or optional. Its draws are
  unchanged; instances holding one gain it.
- Both are exceptions to spec 0424's rule that reproto stays the
  reference (the user's decision, 2026-11-16), each with a failing test
  first in reproto's suite.

## Alternatives considered

### Map entries in generation order

Writing a map as repeated entries in the order the draws produced its
keys is as reproducible, but Python protobuf's map container cannot
keep an order: reproto would have to build those bytes by hand. Sorted
is one flag in each tool.

### reproto left as is, the harness normalizing map order

protoback would match reproto only after both were re-serialized, and
reproto's instances would stay unreproducible, which defeats `--seed`.

### descriptor.proto matched to Python's built-in

A second vendored copy, kept in step with the Python pin, would make
protoback match today's reproto; but instances would keep depending on
an installed version rather than on the schema given.

### Singular groups reproduced, fixed after the port

The standing rule; set aside by the user for this defect.

## Test plan

1. `random_matches_cpython`: a million draws of each method, against a
   table CPython generated, for a few seeds (spec 0424 test plan 6).
2. The harness: instances of every type of the custom database, and of
   googleapis' 400 (`full-tests`), on both tools.
3. reproto's suite, failing before each fix:
   `test_instances_are_reproducible` (a map of several string keys, in
   fresh processes), `test_map_entries_are_in_upbs_deterministic_order`,
   `test_a_singular_group_is_set` and
   `test_the_sets_own_descriptor_proto_is_used`.

## Measured outcome

- **Step 8** (2026-11-16): `instantiate.rs` (the command, the pool, the
  walk), `instantiate/pyrandom.rs` (CPython's `random.Random`).
  `prototext-upb` gains `encode_deterministic` and `insert_message` (a
  map entry whose value is a message, Python's `m[k].CopyFrom(v)`), each
  tested; upb's deterministic order checked on a map of five keys.
  - `random_matches_cpython`: 66 sequences (6 seeds, 11 methods), a
    million draws each, against SHA-256s CPython generated
    (`instantiate/pyrandom_cases.rs`): all equal, first run.
  - Python's `round` is the banker's (`max(1, round(p * max_repeated))`):
    `round_ties_even`.
  - Every message type of googleapis' database (43,111 instances, map
    entries left out of the types): byte-identical with the fixed
    reproto.
  - The three fixes change no test expectation: `tests/stress` (88
    googleapis types) and `tests/custom` (25) pass with the fixed
    generator; they assert the ranking `list-schemas` gives, not the
    bytes.
  - The harness's instance cases (every WKT type, also with other
    options; every loadable type of each custom fixture; 400 googleapis
    types, evenly spaced; an unknown type; no descriptor): identical,
    exit statuses included. Its type lists are read from the sets'
    wire, the harness running without protobuf.
  - The whole harness, every corpus, on all cores and with `--jobs 1`:
    1,587 cases, all identical.
