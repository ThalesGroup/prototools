<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0419 — fuzzing upb with ClusterFuzzLite

Status: draft
App: prototext-upb, CI
Refs: docs/specs/0415-prototext-on-upb.md (test plan 3, which this
      carries out; S4, the gate; the ASan run on stable it extends);
      docs/specs/0182-sharpen-the-decode-stress-generator.md (N1: why
      `cargo-fuzz` was deferred, and the coverage argument it asked for);
      docs/specs/0171-wire-format-bounds-arithmetic-and-recursion-depth-caps.md
      (N5: fuzzing as follow-on work)

## Background

prototools now parses untrusted descriptors with upb, vendored C (spec
0415). A descriptor set is input like any other: users pass their own,
protoscan extracts them from binaries, reproto writes them. What tests
upb's parser and def builder today:

- `tests/pool.rs` mutates a real set 20,000 times with a fixed seed
  (bit flips, insertions, deletions, truncations), loads each one and
  walks every definition, on stable Rust in `cargo test`;
- `scripts/upb-asan` runs those tests with `upb.c` built with
  AddressSanitizer.

Both are blind mutation from one seed. Neither steers toward code it has
not reached, and the gate (0415 S4) refuses most mutated files before
upb's parser sees them, so the mutations mostly exercise the gate.
Specs 0171 and 0182 deferred coverage-guided fuzzing because it needs a
nightly toolchain and sanitizer builds outside `nix-build -A ci`.
ClusterFuzzLite, Google's GitHub Action for libFuzzer, brings both in
its own container, and keeps a corpus between runs.

## Goals

- **G1.** Coverage-guided fuzzing of what upb does with untrusted bytes:
  parsing a `FileDescriptorProto`, building its definitions, and the
  queries the renderer then makes of them, with both upb and the Rust
  wrapper under AddressSanitizer.
- **G2.** It runs without anyone remembering to: briefly on each push
  that touches `prototext-upb`, at length weekly, and not at all in a
  week with no push.
- **G3.** A crash fails a visible workflow run, and once fixed becomes a
  regression input that `cargo test` replays on stable, in `ci`.
- **G4.** Nothing changes for the dev shell, `nix-build -A ci` or the
  published crates: no nightly toolchain, no new workspace member.

## Non-goals

- **N1.** Fuzzing the renderer's wire-format walk (`render_message`),
  which spec 0182 covers with its own oracle; or the scoring walk (0182
  N2). Both could get targets later; this spec sets up the machinery on
  the code that is newest and in C.
- **N2.** MemorySanitizer, and UBSan on the Rust side. MSan needs an
  instrumented standard library; ASan is what OSS-Fuzz runs for Rust.
- **N3.** Upstream reporting. A crash inside upb is reported to the
  protobuf project by hand, with a reproducer, once it is understood.
- **N4.** OSS-Fuzz itself. It wants a project with a user base and a
  maintainer contact; ClusterFuzzLite is its self-serve subset.

## Specification

- **S1. Targets.** A `cargo-fuzz` crate at `prototext-upb/fuzz/`,
  excluded from the workspace (`exclude` in the root `Cargo.toml`, and
  from the Nix sources), with four targets:
  - `load_set`: the input as a `FileDescriptorSet`, through
    `Pool::load_set` on a gated pool (the production path), then a walk
    of every loaded definition: names, fields, kinds, cardinality,
    packing, presence, oneofs, enum values, extensions, declaration
    paths.
  - `add_file_ungated`: the input as one `FileDescriptorProto`, through
    `Pool::add_file` on a pool *without* the gate, after the well-known
    types are loaded so imports can resolve; then the same walk. This is
    the target that reaches upb's parser and def builder with what the
    gate would refuse.
  - `add_file_gated`: the same with the gate, so the gate's own probe is
    fuzzed against the files it admits and refuses.
  - `descriptor`: the input through the descriptor layer:
    `FileDescriptorSet::decode`, `files_of_set` and
    `SourceCodeInfo::of_file`, and re-encoding what decoded.
- **S2. Oracles,** beyond "no crash, no sanitizer report": after
  `load_set`, every file is either loaded or skipped, never both;
  `loaded` plus `skipped` accounts for every file of the set; each loaded
  file is found by name; `Pool::generation` equals the number loaded.
  Debug assertions are on (`upb_str`'s UTF-8 check among them).
- **S3. Seed corpus,** committed under `prototext-upb/fuzz/corpus/<target>/`:
  the repository's own descriptors (`editions.pb`, `nested.pb`,
  `prototext-core/fixtures/descriptor.pb`, the embedded well-known
  types), each set also split into its single files for the
  `add_file_*` targets. A libFuzzer dictionary of `descriptor.proto`'s
  tags and keywords (`proto2`, `editions`, `google.protobuf.`).
- **S4. Build.** `.clusterfuzzlite/` holds `project.yaml` (`language:
  rust`), a `Dockerfile` from `gcr.io/oss-fuzz-base/base-builder-rust`
  copying the repository, and `build.sh` running `cargo fuzz build -O
  --debug-assertions` in `prototext-upb/fuzz` and copying the targets,
  seed archives and dictionary to `$OUT`. The image's `CFLAGS` carry the
  sanitizer and coverage flags, and the `cc` crate passes them to
  `upb.c`, so upb is instrumented as the Rust code is; `build.sh`
  checks that it was (an ASan symbol in the target), since an
  uninstrumented `upb.c` would fuzz without seeing upb's memory errors.
- **S5. Workflows,** in `douzebis/prototools` only (as `nix.yml`):
  - `fuzz-change.yml`: on a push to `main` or a pull request touching
    `prototext-upb/**` or `.clusterfuzzlite/**`, the `build_fuzzers` and
    `run_fuzzers` actions in `code-change` mode, address sanitizer,
    `fuzz-seconds: 600` in total.
  - `fuzz-weekly.yml`: on a weekly schedule and on demand. A first step
    ends the run when `main` has no commit in the past seven days. Then
    `batch` mode, `fuzz-seconds: 3600`, then `prune` mode, which
    minimizes the corpus `batch` grew.
  - The corpus and crashes are kept in GitHub Actions artifacts, the
    action's default storage: no second repository and no token.
- **S6. Crashes.** A crash fails the run and uploads its reproducer as
  an artifact; GitHub mails the failure. Once fixed, the reproducer is
  committed under `prototext-upb/tests/fuzz-regressions/`, and a test in
  `tests/pool.rs` loads every file there through the same code as the
  target, on stable, in `ci`.
- **S7. Reproduction** is documented in `prototext-upb/fuzz/README.md`:
  with nightly and `cargo-fuzz`, `cargo fuzz run <target> <file>`; or,
  without either, the regression test once the file is committed.

## Alternatives considered

### A workflow of our own around `cargo fuzz`

Corpus storage, minimization, crash triage, sanitizer flags for the C
side and the scheduling would all be ours to write and keep working;
ClusterFuzzLite is those, maintained, and the same infrastructure as
OSS-Fuzz.

### More blind mutations in `cargo test`

Spec 0182's approach, and kept: it runs everywhere, on stable. But it
does not learn, and most of its mutations stop at the gate. It stays
the floor; this is the search above it.

### Fuzz upb only through the gate

It is the production path, but the gate refuses most malformed files
before upb parses them, so upb's parser would be fuzzed only with what
the gate already accepts. `add_file_ungated` is what tests upb on its
own; the gate is not a reason to leave upb unfuzzed.

### A storage repository for the corpus

ClusterFuzzLite can keep the corpus in a second git repository, which
survives the 90-day artifact retention. It needs a token with write
access in the repository's secrets; artifacts need nothing, and a weekly
run refreshes them well inside 90 days.

## Test plan

1. `build.sh` runs locally in the base-builder image and produces four
   instrumented targets.
2. Each target runs for a minute on its seed corpus without a finding,
   and its coverage grows past the seeds.
3. A deliberately planted bug (an out-of-bounds read in a scratch copy
   of the shim) is found by `add_file_ungated` in `code-change` mode,
   and the run fails with its reproducer attached.
4. `fuzz-weekly.yml` skips a week with no commit, and runs on demand.
5. The regression test replays an example input on stable, inside
   `nix-build -A ci`.
6. `ci`, `cargo package` and `reuse lint` pass: the fuzz crate is
   invisible to all three.

## Measured outcome

Filled in at implementation.
