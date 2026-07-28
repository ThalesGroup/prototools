<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0198 — a batch subcommand that only starts up

Status: implemented
Implemented in: 2026-07-28
App: protolens
Refs: docs/specs/0123-protolens-batch-mode.md (the `export` subcommand,
        and its "batch mode resolves no theme" rule),
      docs/specs/0168-protolens-resolve-root-type-before-decode.md
        (G1/G4/G6, the announced startup phases),
      docs/specs/0197-the-descriptor-set-is-loaded-on-demand.md
        (§Measured outcome, which had to borrow `export /`)

## Background

Startup is the phase of protolens most worth measuring and the one with
no way to measure it. A TUI launch ends in an event loop that never
returns; a batch `export` returns, but only after doing a second, large
piece of work that has nothing to do with starting up.

Spec 0197's end-to-end numbers were taken with `export /` for exactly
this reason, and the choice is visible in the numbers: on a 25 MB
descriptor set `export /` renders the whole document to prototext and
writes it to stdout, which is the same order of magnitude as the startup
it was supposed to be timing. The measurement survived only because the
effect being measured was 60×, large enough to swamp the contamination.
A 5% regression would not survive it.

What is wanted is a subcommand that performs every phase a TUI launch
performs, in the same order, and then exits — so `hyperfine` (or the
stderr phase lines the TUI already prints) attributes wall time to
startup and to nothing else.

The phases, in the order `main` runs them:

| phase | announced as |
|---|---|
| descriptor-set load | (spec 0197's warning, only on fallback) |
| root-type inference | `inferring root type...` |
| decode + render | `rendering root node as <T>...` |
| `App::new` | `indexing <n> lines...` |

`App::new` is the one that matters most and the one `export` obscures
least clearly: it builds both line maps, seeds the override collection
and runs a full `render_overrides` pass — 5.2 s on a 1.1 MB descriptor
set, longer than the render before it.

## Goals

- **G1.** `protolens exit <blob>` performs every startup phase a TUI
  launch performs, in the same order, then exits `0` without entering
  the alternate screen.
- **G2.** It performs no work a TUI launch does not — no path
  resolution, no extraction, no output.
- **G3.** It prints the same phase lines to stderr that a TUI launch
  does, so one run attributes its own wall time.
- **G4.** It performs no terminal I/O at all, so a measurement reports
  protolens rather than the benchmark harness's tty.

## Non-goals

- **N1.** A `--timing` / `--json` phase report. The four stderr lines
  plus `hyperfine` are the measurement; a structured report is a second
  format to keep correct for no additional fact.
- **N2.** Making `export` cheaper, or changing what it measures.
- **N3.** A "render N frames and exit" mode. That is a *frame* benchmark
  rather than a startup one, it needs a real terminal, and
  `PROTOLENS_TRACE` already covers the frame question.
- **N4.** Any flag on `exit`. It takes the root command's options
  (`--descriptor-set`, `--type`, `--raw`, `--indent`, the blob) and
  nothing of its own.

## Specification

### S1. `Command::Exit`

A second variant on the batch subcommand enum (`main.rs:120`), with no
fields:

```rust
/// Run every startup phase and exit, without entering the interactive
/// TUI — the startup-time benchmark target (spec 0198).
Exit,
```

and a dispatch arm (`main.rs:453`) that is exactly
`Some(Command::Exit) => ExitCode::SUCCESS`. Everything before the
dispatch — `DescriptorContext::load`, root-type resolution, `decode`,
`App::new`, the `override_preview_byte_budget` assignment — already runs
unconditionally for every mode, which is what makes G1 and G2 hold by
construction rather than by a list of calls kept in sync by hand.

### S2. `exit` announces; `export` stays silent

`let announce = cli.command.is_none();` (`main.rs:371`) becomes

```rust
let announce = matches!(cli.command, None | Some(Command::Exit));
```

Spelled as a positive list rather than as `!matches!(.., Export)`, so a
future subcommand is silent by default and has to opt in — the direction
a mistake should fall, since `protolens/tests/batch_export.rs:287`
asserts a successful `export` writes nothing at all to stderr, and that
assertion is the contract every *other* batch subcommand should inherit.

`exit` opts in because its whole purpose is the phase breakdown: without
the lines, a run reports one number and the phase that regressed has to
be found by bisection.

### S3. `exit` resolves no theme and probes no terminal

`exit` keeps the existing `Some(_)` arm of the theme match
(`main.rs:414-430`): `ThemeKind::Dark`, and no `prime_supports_rgb()`.

This is not the "batch mode has plain output" argument spec 0123 made —
`exit` produces no output at all. It is G4. Both `theme::resolve_system`
(via `terminal_light::luma`) and `theme::prime_supports_rgb` (via
`xtgettcap_reports_rgb`) write an escape sequence to the terminal and
wait for a reply under a timeout. Under `hyperfine` there is no terminal
to reply, so what those two contribute to a measurement is the timeout,
not any protolens work. Excluding them costs nothing else: the theme is
stored by `App::new` and first consulted when a frame is drawn, and
`exit` draws none.

The consequence to state plainly is that `exit` under-reports a real TUI
launch by however long those two probes take. That is the correct trade
for a regression benchmark, whose job is to be sensitive to code changes
and insensitive to everything else.

## Alternatives considered

### A1. A `--exit-after-startup` flag on the root command

Rejected. The root command's flags configure the TUI; a *mode* is a
subcommand here, which is the shape spec 0123 established for `export`.
A flag would also have to be excluded by hand from every future
subcommand.

### A2. Keep benchmarking with `export /`

Rejected — see Background. It measures startup plus a full document
render plus an I/O write, and the last two grow with the blob exactly as
the first does, so the contamination cannot be normalized away.

### A3. Have `exit` stop short of `App::new`

Cheaper, and defensible if `exit` is read as "measure the *decode*".
Rejected: `App::new` is the largest unannounced phase, and it is the one
most likely to regress, since `render_overrides` walks the whole
document. A startup benchmark that omits it measures the wrong half.

### A4. Name it `noop`, `bench`, or `startup`

`exit` is what was asked for and it is the most honest of the four: it
says what the command does, not what it is for. `bench` would imply the
command does the timing.

## Test plan

1. **`exit` exits 0 and writes nothing to stdout.** Over the existing
   integration-test fixture, in the style of
   `protolens/tests/batch_export.rs`.
2. **`exit` announces its phases.** Assert stderr contains
   `rendering root node as` and `indexing`, which is S2 stated as an
   observation and the assertion that fails if `announce` is ever
   narrowed back to `is_none()`.
3. **`export` still writes nothing to stderr.** The existing assertion
   at `batch_export.rs:287`, which is what S2's positive spelling
   protects; named here because it is the test S2 could plausibly break.
4. **`exit` accepts the root options.** One run with
   `--descriptor-set` + `--type` and one with `--raw`, both exiting 0 —
   the two paths that skip inference (spec 0168 G6), and therefore the
   two whose phase lines differ.

## Open questions

**Q1. Should `exit` print a final line with the total elapsed time?**
Not proposed. `hyperfine` and `/usr/bin/time` both report it more
accurately than the process can report on itself, and a self-reported
total invites treating the four phase lines as a timing API rather than
as progress messages that happen to be timestamped by the reader.

## Measured outcome

Release build, 2026-07-28. Blob and descriptor set are the same file,
`/tmp/pdb.desc` (1.1 MB), decoded as `google.protobuf.FileDescriptorSet`
— 276 541 rendered lines, which is large enough for the render to
dominate:

| | `exit` | `export /` |
|---|---|---|
| wall time, three runs | 0.21 / 0.21 / 0.20 s | 0.23 / 0.25 / 0.26 s |
| peak RSS | 129.8 MB | 140.4 MB |

So `export /` charges startup measurement about 20% in time and 8% in
resident memory that is not startup at all — the second render plus the
write. That is the contamination G2 removes, and it is comfortably
larger than the 5% regression the background section worried about
missing.

On the googleapis corpus (`googleapis.desc`, 25.6 MB, with its
sidecars, blob
`instances/google/container/v1beta1/ParallelstoreCsiDriverConfig.pb`)
both variants sit at 0.01-0.03 s and 28 MB after spec 0197, so that
input no longer discriminates between them: on-demand loading made the
descriptor set free, and the blob is 190 bytes. It remains the right
input for measuring *loading*, and `/tmp/pdb.desc` the right one for
measuring *rendering*.

Output of `exit` on the `/tmp/pdb.desc` run, confirming G3 (the phase
lines are exactly the interactive ones, and nothing lands on stdout):

```
protolens: rendering root node as google.protobuf.FileDescriptorSet (1 MB)...
protolens: indexing 276541 lines...
```
