<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0431 — protoback, plugins

Status: implemented
Implemented in: 2026-11-16
App: protoback
Refs: docs/specs/0424-protoback-reproto-in-rust-on-upb.md (step 7 of its
      work plan; S9, plugins out of process; test plan 5);
      docs/specs/0369-fdp-plugin-applied-at-every-parse.md (the plugin contract: each
      descriptor freshly parsed, before anything reads it, changed in
      place; a failure aborts the run)

## Background

protoback refuses `--fdp-plugin` and `--phase2-plugin` (step 7). reproto
`exec()`s the plugin file in its own process and calls `fdp_plugin(fdp)`
on each `FileDescriptorProto` it parses, at four places:

1. each file as it is read (`load.parse_qfile`), a text file then
   parsed with descriptor.proto alone, where without a plugin it is
   only scanned for its imports;
2. each embedded fallback (`_load_embedded_fallback`);
3. each text file read again in import order (`_read_text_files`);
4. each file merged into the pool (phase 2's rank loop).

The same file reaches the plugin several times, freshly parsed each
time; spec 0369 S1 makes its result depend on the descriptor alone. An
exception it raises becomes a `PluginError`, which ends the run:
`Error: FILE: plugin raised T on 'NAME': MESSAGE`, exit status 1. A
plugin file without its entry point fails at startup, before any file is
read.

protoback cannot `exec()` Python. Spec 0424 S9 decided the plugin runs
in a process of its own, any language through a framed protocol, Python
plugins through an adapter protoback embeds.

## Goals

- **G1.** `--fdp-plugin FILE` (and the deprecated `--phase2-plugin`)
  gives reproto's outputs, on the harness's plugin cases and on
  reproto's plugin tests (`test_fdp_plugin.py`) run against protoback.
- **G2.** `--fdp-plugin-cmd CMD`: a plugin in any language, through the
  protocol (S2). protoback's own: reproto keeps `--fdp-plugin` alone.
- **G3.** A plugin that raises, exits, or answers malformed fails the
  run, naming the file it was given.
- **G4.** One call per distinct descriptor (S5), counted by a test.

## Non-goals

- **N1.** Python plugins' `ctx`: `--phase2-plugin`'s `phase2_plugin(ctx,
  fdp)` gets `None` (spec 0424 S9).
- **N2.** Messages beyond the plugin's own errors: step 9.

## Specification

- **S1. Where the plugin runs.** At reproto's four places (Background),
  on the descriptor as parsed there; the descriptor it returns replaces
  it. With a plugin, a text file is parsed when read, with
  descriptor.proto alone, as reproto parses it.
- **S2. The protocol.** protoback starts the plugin once per run, before
  phase 1: `--fdp-plugin-cmd CMD` as `sh -c CMD`, so that quoting,
  environment assignments and pipelines work as in git's aliases and
  hooks. It talks to it over its stdin and stdout in frames: a varint
  length, then that many bytes. Its stderr is protoback's.
  - Each frame protoback writes is a serialized `FileDescriptorProto`.
  - Each frame the plugin writes starts with a status byte: `0`, the
    changed descriptor follows; `1`, an error message follows (UTF-8),
    which fails the run as the plugin's own (S4).
  - The plugin first writes one frame unprompted: `0` alone when it is
    ready, `1` and a message when it cannot run (a Python plugin without
    its entry point). protoback waits for it before reading any file.
- **S3. The Python adapter.** `--fdp-plugin FILE` runs `python3`, or
  `$PROTOBACK_PYTHON`, on an adapter protoback embeds
  (`plugin/adapter.py`, given to `python3 -c`). It `exec()`s `FILE`, checks its entry
  point with reproto's messages, answers the ready frame, then calls
  `fdp_plugin(fdp)` on each descriptor, parsed and serialized with the
  installed Python `protobuf`. `--phase2-plugin` calls
  `phase2_plugin(None, fdp)`, after reproto's deprecation warning,
  extended to say that `ctx` is `None`. An exception becomes an error
  frame with reproto's text.
- **S4. Failures.** A `PluginError` ends the run with exit status 1:
  - an error frame: `Error: FILE: plugin raised T on 'NAME': MESSAGE`
    for the adapter, reproto's text; for a command, its message;
  - the plugin exiting, or a frame that is not a status byte and a
    payload, or a descriptor that does not parse: `Error: CMD: plugin
    failed on 'NAME': WHAT`;
  - at startup, an error frame or no ready frame: the same, without a
    file name.
- **S5. One call per distinct descriptor.** protoback keeps each answer
  by the bytes it sent, and sends a descriptor once.
- **S6. Threads.** One plugin process; calls go through a lock. The four
  places are in phases 1 and 2, which run on one thread.

## Alternatives considered

### Embedding Python

Calling `fdp_plugin` in-process through PyO3 would keep reproto's
behavior with no protocol, but links libpython into protoback and pins
its version; ruled out by spec 0424 S9.

### No ready frame

Simpler for a command plugin, but a Python plugin without its entry
point would then fail on the first file instead of at startup, as
reproto fails; a plugin that has nothing to report writes two bytes.

### `--fdp-plugin-cmd` in reproto too

Either tool would then run a command plugin, and the harness would
compare them directly; but it is a second implementation of the
protocol to keep in step, in the tool being replaced. The harness checks
a command plugin against the same plugin through `--fdp-plugin` instead.

### CMD split into argv

Predictable and shell-free, but without the quoting, `FOO=1` prefixes
and pipelines a command line is expected to take.

## Test plan

1. reproto's `test_fdp_plugin.py`, with `REPROTO_CMD` set to protoback.
2. The harness: plugin cases on the `plugin_subject` fixtures, through
   `--fdp-plugin`.
3. `a_command_plugin_changes_a_descriptor`: a plugin written as a shell
   or Python command, through `--fdp-plugin-cmd`, against the same
   plugin through `--fdp-plugin`.
4. `a_plugin_that_exits_raises_or_misframes_fails_naming_the_file`.
5. `each_distinct_descriptor_is_sent_once`: a command plugin counting
   its calls.

## Measured outcome

- **Step 7** (2026-11-16): `plugin.rs` (the process, the frames, the
  cache), `plugin/adapter.py`, and the four places: `load::parse_qfile`
  (a text file parsed with descriptor.proto alone when there is a
  plugin, its imports the answer's), `phase1::embedded_fallback` and
  `read_text_files` (the imports the answer's, the contents unpatched,
  as reproto's `qf.desc` setter has it), `phase2::merge` (the answer
  merged). A failure is kept and ends the run after the phase it
  happened in: nothing is written, but that phase's remaining files are
  read first (their plugin calls skipped), where reproto stops at once.
  - The text parse with descriptor.proto alone moved to `prototext-upb`
    (`text_format::parse_descriptor`); the Python binding's
    `parse_descriptor_text` calls it.
  - reproto's `test_fdp_plugin.py` against protoback: 7 of 8 pass. The
    eighth checks the output, which is right, then the stripped import's
    warning on stderr, which protoback prints in step 9 (N2).
  - The harness's plugin cases (an import and a field added, also under
    `--emit-binary`; `--phase2-plugin`; text input; a plugin that
    raises; one without its entry point): identical, exit statuses
    included. The whole harness, every corpus, on all cores and with
    `--jobs 1`: 1,573 cases, all identical.
  - `protoback/tests/plugin.rs`: a command plugin in bare Python changes
    a descriptor; each distinct descriptor is sent once (fails with the
    cache off); exiting, a wrong status byte, an answer that is not a
    descriptor, an error frame, no ready frame: exit status 1, the file
    or the command named, nothing written.
  - Not measured: what a plugin costs on googleapis, the adapter against
    a command plugin and reproto's in-process call. Deferred until the
    port is complete (the user's decision).
