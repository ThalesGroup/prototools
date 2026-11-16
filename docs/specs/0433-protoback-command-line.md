<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0433 — protoback, the command line

Status: draft
App: protoback, reproto
Refs: docs/specs/0424-protoback-reproto-in-rust-on-upb.md (step 9 of its
      work plan; S11, every stream compared; S12, normalizations; S14,
      the command line; test plans 7, 8, 9);
      docs/specs/0041-squashed-warnings.md (the warnings, W1 to W7);
      docs/specs/0422-reproto-completes-in-bash-zsh-and-fish.md (completion)

## Background

After step 8, protoback writes reproto's files, instances and exit
statuses on the whole harness (1,587 cases), but not its stderr: compared
on every stream, 1,572 cases differ there and nowhere else. reproto's
stderr, over the corpus (2026-11-16), is made of:

- the phase labels (`Loading descriptors` … `Rendering proto files`,
  `Collecting scoring data`), each printed when its phase ends;
- `Using embedded fallback: NAME`;
- the warning collector (`lib/warnings.py`): W3 and W7 at once; W1, W4,
  W5, W6 and the `--force-proto2-for-editions` patches counted, flushed
  at the end as a sorted summary, with a hint to `--detailed-warnings`,
  which prints each at once instead; `--quiet` silences warnings;
- the editions notices, seed and prune notices (`Seed not found`, `Did
  you mean`), the schema database's summary (`descriptor:`, `graph:`,
  `index:`, `pyvis …`);
- `--debug-fqdn`'s trace, a line per node (`FQDN S P R desc:.a.B`);
- usage and other errors.

reproto prints through a Rich console. When stderr is not a terminal,
Rich assumes 80 columns and word-wraps: 20-odd lines of the corpus break
mid-message (`Warning: unresolvable type .x.Y (1` then `occurrence)`).
On a terminal it draws transient progress bars and colors.

## Goals

- **G1.** protoback's stdout and stderr are reproto's, when they are not
  a terminal, on every corpus and flag set of the harness, which now
  compares every stream (spec 0424 S11), with S12's normalizations.
- **G2.** On a terminal: progress bars (indicatif) and colors, as
  reproto's are shown.
- **G3.** `--help` in reproto's sections, `--version`, man pages through
  `PROTOBACK_GEN_MAN`, completion through `prototools-complete` (bash,
  zsh, fish; `-I`-relative, blob members), as spec 0424 S14 has them.
- **G4.** `--dump-resolved-features` and `--detailed-warnings`.
- **G5.** reproto's CLI tests pass against protoback (spec 0424 test
  plan 8).

## Non-goals

- **N1.** `--debug`'s development traces, byte for byte: protoback's
  own (the user's decision). The harness compares every stream but
  stderr where `--debug` is given.
- **N2.** The look of a terminal session, byte for byte: Rich's bars and
  indicatif's differ; G2 is the same information.

## Specification

- **S1. One line per message, fixed in reproto.** reproto's console
  stops hard-wrapping (Rich's `soft_wrap`): a message is one line,
  piped or on a terminal, so that logs stay greppable; protoback prints
  the same lines. An exception to spec 0424's rule that reproto stays
  the reference (the user's decision, 2026-11-16), with a failing test
  first.
- **S2. The collector, ported.** protoback's `Events` gains reproto's
  collector whole: the immediate warnings, the counters, the flush and
  its order, `--detailed-warnings`, `--quiet`.
- **S3. Every message, at reproto's place.** The phase labels, the
  fallback notices, the editions, seed and prune notices, the database's
  summary, the errors, each printed where reproto prints it, in the same
  order relative to the others.
- **S4. `--debug-fqdn`**, reproduced (the user's decision); `--debug`,
  protoback's own (N1).
- **S4b. Each warning once.** Under `--emit-binary` and
  `--schema-db-out`, reproto renders a node at depth d about 2^d times
  (spec 0429 S11), repeating its warnings. protoback renders once, and
  emits each warning once: it does not emulate the repetition (the
  user's decision). Measured on the 73 import fixtures, reproto's stderr
  is the same with and without `--emit-binary`: the repetition does not
  show. Should a corpus show it, reproto is fixed to render once.
- **S5. The terminal.** On a terminal, each counted phase shows an
  indicatif bar, removed when it ends, its label then printed as when
  piped; errors, warnings and notices in reproto's colors (bold red,
  yellow, blue).
- **S6. `--help` and `--version`**, the options in reproto's sections
  and order, their texts; compared by options and texts (spec 0424 S12).
- **S7. Man pages and completion**, as protoscan's: `PROTOBACK_GEN_MAN`
  through `clap_mangen`, `PROTOBACK_COMPLETE` through
  `prototools-complete`, with spec 0422's `-I`-relative paths and blob
  members.
- **S8. The harness compares every stream**, the program's name
  normalized (`--rename`), stderr left out where `--debug` is given.

Stages, each with its gate:

1. stderr when piped (S1–S4, S8): the harness, every stream, every
   corpus, `--jobs 1` and all cores.
2. The terminal (S5): a test under a pseudo-terminal.
3. `--help`, `--version` (S6).
4. Man pages, completion (S7): test plan 9 of spec 0424.
5. `--dump-resolved-features` (G4).
6. reproto's CLI tests against protoback (G5); the final timings.

## Alternatives considered

### Rich's wrapping reproduced

protoback would word-wrap at 80 columns (or `$COLUMNS`) when stderr is
not a terminal, as Rich does: more code for a behavior that splits log
lines; set aside by the user for S1.

### `--debug` byte for byte

Its traces follow reproto's internals, which protoback's do not mirror
(phases computed differently, the pool emulated); set aside by the user.

## Test plan

1. reproto: `test_a_long_message_is_one_line` (fails before S1).
2. The harness, every stream compared (S8), every corpus, `--jobs 1` and
   all cores.
3. A pseudo-terminal test: bars drawn and removed, labels kept.
4. `--help` compared by options and texts; the man page generated;
   completion tests in `prototools-complete`'s style.
5. reproto's CLI tests with `REPROTO` set to protoback.

## Measured outcome

- **Stage 1, stderr when piped** (2026-11-16):
  - reproto: `soft_wrap` on its console (`lib/console.py`);
    `test_a_long_message_is_one_line` failed before.
  - protoback: `console.rs` (warnings silenced by `--quiet`; information,
    attention, labels); `events.rs` becomes reproto's collector (W1 to
    W7, the prost count, the flush and its order, `--detailed-warnings`,
    the fallback and pruned files' registrations), printing as it
    records; the labels at reproto's places (phase 2's before its
    circular-dependency warnings, phase 5's before `--debug-fqdn`'s
    trace, phase 7's even in a dry run; `progress_lazy`'s never when
    piped); each file's render returns what it reports (the anomalies'
    lines, W6, the prost count), replayed in file order, a failed file a
    W6; the database's W6, label and summary; the flush before an error
    the phases raise, after one they print; phase 1 stops at a plugin's
    failure, as reproto's exception does.
  - reproto's own rules kept: A2's line is never printed (`suppress_stderr`
    since reproto's commit 0ef3aa2, `--force-proto2-output` being an
    opt-in), A1's is left to the prost count under
    `--force-proto2-for-editions`.
  - A W6 for a scoring graph the pool refused takes the fixed reason of
    spec 0429 S16, where reproto prints upb's text; no corpus raises it.
  - The harness: `--rename` repeatable (the usage lines' program
    names), `NORMALIZE` for protoback's `--phase2-plugin` warning (spec
    0431 S3). Every stream compared, every corpus, all cores: 1,587
    cases, all identical. `ci`'s `protobackHarness` compares every stream.
