<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0438 — warning codes, counted, and made fatal on request

Status: implemented
Implemented in: 2026-11-17
App: reproto, protoback
Refs: docs/specs/0433-protoback-command-line.md (stderr as reproto
        writes it; the warning collector),
      docs/specs/0426-protocs-text-of-unknown-fields-read-back.md and
      docs/specs/0436-text-input-names-never-dropped-unsaid.md (the text
        reader's warnings, W1 to W4 there, T1 to T4 here),
      docs/specs/0427-text-input-parsed-with-the-inputs-schema.md
        (`annotation_modules` retired)

## Background

A downstream test wants "no value lost" as an assertion. Today it can
only grep stderr, and that breaks whenever a message's wording changes.
Three things stand in the way.

- **Codes clash, or are missing.** The warning collector has W1 and W3 to
  W7, and an unnamed "prost" warning; the text reader's warnings are W1
  to W4 too, for other things (W4 is an unresolvable type in one, an
  unknown field name in the other); the `.proto` anomalies have A1 to D2;
  a dozen warnings have no code at all.
- **Codes are not shown.** Nothing on stderr says which warning is which.
- **The text reader's warnings are printed once per value**, uncounted:
  one line per occurrence buries them in a 5,000-file input.

And a variant key reproto no longer reads (`annotation_modules`, retired
by spec 0427) is ignored silently, so a stale variant looks meaningful.

## Goals

- **G1.** Every warning has one code, unique across both tools, shown on
  its line: `Warning [CODE]: message`.
- **G2.** The text reader's warnings are counted in the summary, as the
  collector's are.
- **G3.** `--fail-on CODE` makes the listed warnings fail the run, after
  it completes, with exit status 3.
- **G4.** A variant key no tool reads is warned about, once.
- **G5.** No option value is dropped unsaid: one the options type does
  not declare (an internal field read against a descriptor.proto that
  lacks it, a custom option whose declaring file is not loaded) is warned
  about, and written as an orphan.

## Non-goals

- **N1.** `--debug` output. Its lines (`Parsing file …`, a pool's own
  reason) are diagnostics, not warnings: no code, not counted.
- **N2.** Changing what is warned about, beyond G4, G5 and W2's wording.
- **N3.** The built-in orphans (`features`, `feature_support`,
  `verification`, `map_entry`), dropped by design: no code, no warning.
- **N4.** A machine-readable warning file (`--warnings-json`): the codes
  on each line are enough for the downstream use that asked for 0438.

## Specification

- **S1. The codes.** One table, in reproto's `lib/warnings.py` and
  protoback's `events.rs`, the man pages listing it:

  | Code | Warning |
  |---|---|
  | W1 | a file not found on the `-I` path; skipped |
  | W2 | an editions file written as proto2 (`--force-proto2-for-editions`) |
  | W3 | a duplicate symbol pruned |
  | W4 | an unresolvable type |
  | W5 | a missing dependency |
  | W6 | an option value that cannot be rendered: missing from the `.proto` |
  | W7 | a file shadowed by one of the same name under another `-I` root |
  | W8 | a descriptor the pool refuses while loading |
  | W9 | an unparseable file skipped while loading |
  | W10 | a circular dependency |
  | W11 | a prune pattern matching nothing |
  | W12 | a seed pattern matching nothing |
  | W13 | two files rewritten onto one path, the variant's descriptor.proto kept (spec 0437) |
  | W14 | `--dump-resolved-features`: a file not loaded |
  | W15 | an input that cannot be parsed |
  | W16 | a self-dependency stripped |
  | W17 | a variant key no tool reads |
  | W18 | a deprecated option or environment variable |
  | W19 | a file the descriptor pool refuses: its options not rendered (was W6) |
  | W20 | a schema-database entry that cannot be built (was W6) |
  | W21 | a scoring-graph entry that cannot be built (was W6) |
  | W22 | an option field the options type does not declare: an orphan (G5) |
  | T1 | text: a field by number declared nowhere, read as length-delimited (was W1) |
  | T2 | text: a string protoc printed as a message, rebuilt (was W2) |
  | T3 | text: an `[extension]` declared nowhere, skipped (was W3) |
  | T4 | text: a field name declared nowhere, skipped (was W4) |
  | A1–D2 | the `.proto` anomalies, as `anomalies.py` numbers them |

- **S2. The line.** `Warning [CODE]: message`, the message as before but
  without a `Warning:` of its own; colored on a terminal as before. W2's
  message: `'FILE' is an editions file; written as proto2
  (--force-proto2-for-editions)`.
- **S3. Counted, and never wrapped.** The text reader's warnings are
  collected as W6 is: one summary line per message, `(N occurrences)`,
  the files listed under `--detailed-warnings`. Summary lines carry their
  code. No line is wrapped by the tools, on a terminal or not (spec 0433
  S1).
- **S4. `--fail-on CODE`**, repeatable. A code not in S1 is a usage error
  (exit status 2). The run completes as without it, its output written;
  after the summary, if a listed code occurred, on stderr: `Error:
  --fail-on: T4 (3 occurrences), W5 (1 occurrence)`, in S1's order, and
  exit status 3. A run that fails otherwise keeps its own status. `-q`
  hides the warnings, not the counting: the error line is printed and the
  status is 3 all the same.
- **S5. Ignored variant keys.** A key of the variant file outside the keys
  read (`name`, `description`, `descriptor_proto`, `well_known`,
  `import_rewrites`, `namespace_rewrites`, `orphans`): W17, once per key,
  `variant 'NAME': key 'KEY' is not read; it has no effect`.
- **S6. Undeclared option fields (G5).** An options message's unknown
  field (by number) is rendered as an orphan line, `/// option <N> =
  VALUE;` in the `.proto`, removed with the other orphans under
  `--redact-orphans`, and W22 is warned either way: `'FILE': option
  field N of TYPE is declared nowhere: written as an orphan`. VALUE is
  read from the wire alone, every wire type: a varint or a fixed-width
  number in decimal; bytes quoted, `"` and `\` escaped, other bytes
  outside printable ASCII as `\ooo`; a group as its fields, `{ <N>: V
  ... }`, recursively (`{ }` when empty). Unknown fields are those upb
  leaves unparsed: an extension reproto's options pool has not loaded yet
  is one, as it is to reproto. Each comes after the built-in and custom
  options, in wire order.
- **S7. Both tools alike**: the same lines, the same summary, the same
  exit statuses.

## Alternatives considered

- **`--fail-on-warning`, every warning fatal.** Downstream runs expect
  some warnings (a missing dependency it prunes on purpose): it would
  have to remove each one first, or not use the option.
- **Stop at the first listed warning.** The output would be partial, and
  the summary would not say how many values were lost.
- **Renumbering every code** into one contiguous scheme: every spec and
  comment citing W1–W7 and A1–D2 would change, for no reader's benefit.

## Test plan

1. `tests/test_warning_codes.py`, against both tools: a run with W1, W5,
   T4 and an anomaly shows each code on its line; T4 counted in the
   summary; `--fail-on T4` exits 3 with the error line, the output
   written, `-q` too; `--fail-on W99` exits 2; a variant with
   `annotation_modules` gives W17; an unknown option field gives W22 and
   its orphan line, with and without `--redact-orphans`.
2. reproto's suite, binary and text input and against protoback, its
   expectations of warning lines updated.
3. The harness: both tools identical.

## Measured outcome

- Test plan 1: `test_warning_codes.py`, 8 tests, pass on both tools, their
  stderr and output identical; the group case is skipped on text input,
  where protoc prints a group as a message (T1).
- Test plan 2: reproto's suite 347 passed on binary input and against
  protoback, on text input with the group case skipped; the binding's
  own tests (`read_descriptors` now gives each warning with its code).
- Test plan 3: the harness, 1,668 cases, and its graph dumps, 1,654
  cases, identical. The dumps keep reproto's record: what the command
  line warns before the run (deprecations, W17) is counted, not recorded.
- protoback printed only `--phase2-plugin`'s deprecation; it now warns
  for every deprecated option reproto does (W18).
- ci and nixpkgs-staging pass.
