+++
title = "prototools docs"
+++

# prototools

A collection of two complementary protobuf tools:

- **prototext** — lossless, bidirectional converter between binary protobuf
  wire format and human-readable text.
- **reproto** — reconstructs `.proto` source files from compiled protobuf
  descriptor sets (`.pb` files produced by `protoc --descriptor_set_out`).

---

## prototext

Lossless, bidirectional converter between binary protobuf wire format and
human-readable text.

### User documentation

- [README](readme) — Overview, installation, and usage guide
- [Man page](man-page) — Reference manual
- [Annotation format](annotation-format) — Wire annotation syntax reference

### Design documentation

- [Design](design) — Architecture, data flow, and key design decisions
- [Performance](performance) — Benchmark results and optimisation history
- [protoc --decode compatibility](protoc-decode-compatibility) — Compatibility
  analysis with `protoc --decode` output
- [protoc --decode anomalous input](protoc-decode-anomalous-input) — How
  protoc behaves on malformed or non-canonical input
- [Fixture coverage model](fixture-macro-reference) — Fixture macro reference
- [Benchmark process](bench-process) — Performance benchmarking methodology
- [Protocraft design](protocraft-design) — Protocraft schema-aware test builder design

### Specs

| Spec | Title | Status |
|------|-------|--------|
| [0003](specs/0003-cli) | `prototext` CLI design | implemented |
| [0004](specs/0004-enum-annotation-syntax) | Enum annotation syntax and `#@` delimiter | implemented |
| [0006](specs/0006-fixture-coverage-model) | Fixture coverage model and gap-filling fixtures | implemented |
| [0007](specs/0007-string-bytes-encoding-policy) | String and bytes field encoding policy | implemented |
| [0008](specs/0008-nan-encoding) | NaN encoding for float and double fields | implemented |
| [0009](specs/0009-protocraft-and-e2e-tests) | Protocraft port and end-to-end test suite | implemented |
| [0010](specs/0010-protoc-compatibility) | protoc --decode compatibility for canonical wire input | implemented |
| [0011](specs/0011-prost-reflect-schema) | Replace hand-rolled schema with prost-reflect | implemented |
| [0012](specs/0012-extension-field-rendering) | Extension field rendering | implemented |
| [0013](specs/0013-protocraft-schema-aware-builder) | Protocraft schema-aware builder | implemented |
| [0014](specs/0014-pyo3-extension) | PyO3 Python extension (`prototext-pyo3`) | draft |

---

## reproto

Reconstructs `.proto` source files from compiled protobuf descriptor sets
(`.pb` files produced by `protoc --descriptor_set_out`).

### User documentation

- [README](reproto-readme) — Overview, installation, and usage guide
- [Man page](reproto-man-page) — Reference manual

### Specs

#### Core design

| Spec | Title | Status |
|------|-------|--------|
| [0021](specs/0021-variant-bundle-layout) | Variant bundle layout and uniform resource loading | implemented |
| [0024](specs/0024-rendering-anomaly-taxonomy) | Rendering anomaly taxonomy and reporting | draft |

#### Proto3 polyglot

| Spec | Title | Status |
|------|-------|--------|
| [0019](specs/0019-proto3-field-labels) | Field labels, synthetic oneofs, default values, json_name, import weak, extensions | implemented |
| [0020](specs/0020-proto3-inconsistency-guards) | Proto3 inconsistency guards (required, groups, message_set_wire_format) | implemented |
| [0023](specs/0023-proto3-custom-options-extend) | Allow `extend *Options` blocks (custom options) | implemented |

#### Editions rendering

| Spec | Title | Status |
|------|-------|--------|
| [0025](specs/0025-editions-rendering-strategy) | Editions rendering strategy — why it is harder than proto3 | — |
| [0026](specs/0026-editions-feature-resolution) | Editions feature resolution engine | implemented |
| [0029](specs/0029-editions-full-output) | Editions rendering: complete edition output | implemented |
| [0030](specs/0030-editions-roundtrip-tests) | Editions roundtrip tests | implemented |

---

## Build system

| Spec | Title | Status |
|------|-------|--------|
| [0038](specs/0038-default-nix-refactor) | `default.nix`: unify and deduplicate the build definition | implemented |
