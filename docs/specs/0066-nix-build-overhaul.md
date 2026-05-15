<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0066 — Nix build system overhaul

**Status:** implemented
**Implemented in:** 2026-05-14
**App:** (build system — not app-specific)

---

## Purpose

`default.nix` has grown organically since spec 0038 and now exhibits several
structural issues that make it fragile, hard to read, and expensive to extend.
A critical concern is that the dev cycle is being negatively impacted by the
time it takes to `nix-build` after each development step.

- Three nearly-identical PyO3 extension patterns with no abstraction.
- A complex three-stage reproto build with a fragile intermediate store path
  that leaks into shell environments.
- A 120+-line monolithic shellHook with no separation of concerns.
- Minor inconsistencies in Cargo flags and variable naming across derivations.
- The `ci` comment is stale.

This spec describes the current state in full and lays out the goals for the
overhaul.

---

## Design drivers

These principles apply to this project and are intended to be reusable as a
reference for other repositories with a similar Nix + Cargo + PyO3 + Python
stack.

### D1 — Compile each source file exactly once

Rust compilation is expensive.  Every Crane derivation in the same build
carries identical `commonArgs` (same `RUSTFLAGS`, `PYO3_PYTHON`, Cargo
profile).  A single shared `depsCache` covers the whole workspace.  No crate
is compiled with different flags in different derivations — that would
invalidate Cargo fingerprints and force a rebuild.

### D2 — Fast `nix-build` iteration cycle

The `ci` target (linters + quick tests) must be cheap to rebuild after a
typical development change.  The `full-tests` target (stress tests, slow
integration tests) is separate and only required before merge.  Derivation
boundaries are drawn so that an isolated source change invalidates the minimum
number of downstream derivations.

### D3 — Nix store as the single source of built artefacts

Compiled artefacts (`.so` files, `.pb` descriptors, generated Python source)
live in the Nix store, not in the working tree.  The dev-shell exposes them
via environment variables (`PYTHONPATH`, `PATH`) pointing at store paths.
The `treeSitterTextproto` derivation is the canonical example of this pattern.
Working-tree writes from the shellHook are eliminated wherever possible.

### D4 — One source of truth per repeated pattern

Repeated structures (PyO3 extension + Python wrapper, proto compilation
lists) are expressed once via a Nix helper function or named constant.
Copy-pasted blocks are a bug: they drift apart under maintenance and create
invisible flag inconsistencies that break Cargo fingerprints.

### D5 — Nix code is written for non-Nix-expert readers

Tricky constructs (Nix string interpolation inside bash strings, Crane
internal variables, `buildDepsOnly` stub behaviour) are explained in inline
comments.  Idiomatic Nix is preferred over clever one-liners.  The file
structure is navigable without deep Nix knowledge.

A high-level pipeline diagram is included near the top of `default.nix` (or
the relevant sub-file) as an ASCII comment block, showing how source paths
and derivations relate.  The diagram is kept deliberately high-level (names
and arrows only, no flag details) so it rarely needs updating.  Example shape:

```
#   ./reproto/  ──[builtins.path]──▶  reprotoSrc
#                                          │
#                                 [pkgs.runCommand]
#                                          │
#                                          ▼
#                                   reprotoSrcFull
#                                          │
#                           ┌──────────────┼──────────────┐
#                           ▼              ▼              ▼
#                       reprotoBare    reproto      reprotoTests …
```

### D6 — Compatible with GitHub CI across four platforms

The Nix CI workflow (`.github/workflows/nix.yml`) runs `nix-build` on four
GitHub-hosted runners:

| Runner | Architecture | OS |
|---|---|---|
| `ubuntu-latest` | x86_64 | Linux |
| `ubuntu-24.04-arm` | aarch64 | Linux |
| `macos-15` | aarch64 (Apple Silicon) | macOS |
| `macos-15-intel` | x86_64 | macOS |

Every Nix derivation in `default.nix` must evaluate and build correctly on
all four platforms.  Known platform-specific constraints:

- **macOS `install_name_tool` path-length limit** — on `macos-15-intel`,
  patching the Mach-O load commands of large binaries fails when the
  replacement path exceeds ~512 bytes.  This is why `prototextExtension`
  cannot chain off `rustClippyPyo3` (the clippy binary's embedded store
  path exceeds the limit).  The CI workflow works around this by skipping
  clippy on `macos-15-intel` (`skip-clippy: true`).
- **`.so` vs `.dylib`** — the shared library extension differs by platform.
  Derivations must select the correct extension at Nix eval time (e.g. via
  `if pkgs.stdenv.isDarwin then "dylib" else "so"`).

The CI workflow uses **DeterminateSystems Magic Nix Cache** to share Nix
store paths across runs and platforms, dramatically reducing rebuild time.
The build system must produce derivations that are cache-friendly: stable
`src` filters, deterministic content, no impure references.

**Release venues** — multiple publication channels are available, each
covering a different subset of the project's artefacts and imposing its own
constraints:

| Venue | Artefacts | Constraints |
|---|---|---|
| GitHub Releases | pre-built binaries (all four platforms) | musl static linking for Linux; plain `cargo build`, no Nix |
| nixpkgs | Nix derivations | must be self-contained, reproducible, pass `nix-build` on all platforms |
| crates.io | Rust crates (`prototext`, library crates) | `cargo publish`; no binary blobs, no path dependencies |
| PyPI | Python wheels (`prototext_codec`, `fdp_scan`, `scoring_graph`, `reproto`, `protoscan`) | `maturin` or `hatch` build; platform tags; no Nix dependency |

The build system should accommodate these constraints so that:
- publishing to each venue is straightforward and documented,
- there is at least a smoke test for each publication path,
- manual steps are minimal and written down.

**Release bundle pattern** — for crates.io and PyPI, a dedicated Nix
derivation (`releaseBundle` or similar) prepares a ready-to-upload directory
tree in the Nix store.  The human's only action is to run a short top-level
script that calls the upload tool in dependency order:

```
$out/
  crates/
    00-prototext-core/    ← path deps rewritten to version deps
    01-score-graph/
    02-prototext/         ← includes pre-generated wkt.rkyv (see below)
    publish.sh            ← calls `cargo publish` in order
  wheels/
    00-prototext-codec/   ← ready-to-upload source tree
    01-fdp-scan/
    02-scoring-graph/
    03-reproto/
    04-protoscan/
    publish.sh            ← calls `twine upload` (or `hatch publish`) in order
```

Neither crates.io nor PyPI requires a link to a Git repository; both accept
a self-contained tarball/source tree produced by the Nix derivation.

**crates.io — path dependency rewriting** — `cargo publish` rejects path
dependencies.  The release derivation rewrites each crate's `Cargo.toml`,
replacing `path = "../sibling"` with `version = "x.y.z"`.  Crates are
published leaf-first so version deps resolve at publish time.

**crates.io — embedded WKT rkyv** — `prototext` with auto-inference enabled
embeds a pre-built Hopcroft DB (`wkt.rkyv`) generated from the WKT descriptor.
This creates a build-time cycle (`prototext-full` → `reproto` → `prototext-bare`),
broken by a Cargo feature flag (`wkt-db`, default on): the bare build
(`--no-default-features`) omits the rkyv; `reproto` depends on the bare build;
the full build's `build.rs` embeds the rkyv produced by a Nix `runCommand`
that calls `reproto --build-schema-db` over the embedded descriptor.  The
release derivation includes the pre-generated `wkt.rkyv` in the crate source
tree so `cargo publish` is self-contained.

For the current overhaul, the GitHub Releases workflow
(`.github/workflows/release.yml`) is treated as a reference implementation:
it is independent of `default.nix` (uses plain `cargo build` with musl
toolchains — musl is a C standard library that links statically into the
binary, producing a self-contained executable that runs on any Linux
distribution without needing shared library compatibility) and is unaffected
by the overhaul.  The other venues are a
non-goal for this spec but the build system must not foreclose them.

### D7 — Classic `default.nix` / `shell.nix`, no flakes

The project stays on the traditional `nix-build` / `nix-shell` interface.
Flakes adoption is out of scope (see O10).  The pinned nixpkgs and crane
revisions remain inline in `default.nix`.

### D8 — Everything Rust compiles for release

Crane builds in release mode by default (via `configureCargoCommonVarsHook`).
`CARGO_PROFILE = "release"` must not be set in `commonArgs` — it is a
Crane-internal shell variable, not a Nix attribute, and setting it as a Nix
derivation attribute has no effect; it only misleads readers.  The intent is
documented in a comment near `commonArgs` instead.  All `cargo build`
invocations in the shellHook pass `--release` explicitly.  Debug builds are
never used.

---

## Observations

### O1 — Three identical PyO3 extension patterns

`prototextExtension`, `fdpscanExtension`, and `scoringGraphExtension` are
structurally identical:

1. `cargoExtraArgs = "${xyzArgs} --lib"`
2. `preBuild`: delete fingerprints for the crate
3. `postBuild`: `cargo run --release -p xyz_extension --bin xyz_post_build`
4. `installPhase`: copy `.so` + `.pyi` into `$out/artifacts/`

Similarly, the three `buildPythonPackage` wrappers (`prototextCodec`,
`fdpScanLib`, `scoringGraphLib`) are structurally identical:

1. `format = "pyproject"`
2. `buildInputs = [ pythonPkgs.hatchling <extension> ]`
3. `patchPhase`: copy `.so` + `.pyi` from extension artifacts into source tree

No Nix helper function abstracts this pattern.  Adding a fourth PyO3 crate
requires copying ~30 lines in two places.

The factoring goal is two-fold: (1) reduce code duplication in `default.nix`,
and (2) reduce the number of times a given source file is compiled.  Rust
compilation is expensive; standardising the compilation flags for all
derivations ensures that, ideally, each source file is compiled only once
(Cargo fingerprints match across derivations that share `depsCache`).

### O2 — Three separate Clippy derivations with overlapping scope

There are three clippy derivations: `rustClippy` (workspace-wide),
`rustClippyPyo3` (`-p prototext_codec`), and `rustClippyScoringGraph`
(`-p scoring_graph_extension`).

`rustClippy` already runs `--workspace`, which covers all crates including
`prototext_codec` and `scoring_graph_extension`.  The two narrower derivations
therefore re-lint code that `rustClippy` already checked, at the cost of
additional build time and cache entries.

The motivation for the separate derivations (per the spec 0038 comment) was
that `install_name_tool` path-length limit on macOS prevented chaining
`prototextExtension` off `rustClippyPyo3`.  That concern applies to the
_extension_ derivation, not to keeping three separate clippy derivations in CI.

### O3 — Complex three-stage reproto build

The reproto package is built in three stages:

1. `reprotoBare` — installs reproto without tests; copies `patch/` into `$out`.
2. `reprotoSrcFull` (rename from `reprotoSrcWithCodegen`) —
   (`pkgs.runCommand`) copies `reprotoSrc`, seeds well-known `.proto`
   sources from `pkgs.protobuf`, then runs `patch_reproto.sh` to compile
   `.pb` descriptors in-place.  "Full" signals that this is the complete,
   enriched source tree ready for `buildPythonPackage`.

   `reprotoSrc` and `reprotoSrcFull` are two distinct steps in a pipeline:
   `reprotoSrc` (rename from `reprotoFilteredSrc`) is a `builtins.path`
   snapshot of the raw `./reproto` working-tree directory with unstable
   files (`.pb` outputs, `__pycache__`, result symlinks) filtered out to
   keep the hash stable.  `reprotoSrcFull` takes that snapshot as input,
   copies the filtered tree, adds the well-known `.proto` sources from
   `pkgs.protobuf`, and runs `patch_reproto.sh` to compile the fixture
   `.proto` files into `.pb` descriptors.
3. `reproto` — built from `reprotoSrcFull`; runs tests separately via
   `reprotoTests`.

Regarding a PyPI release using `reprotoSrcFull` as source: this would be
inadvisable.  `reprotoSrcFull` is a Nix store path — it embeds compiled `.pb`
binary descriptors produced inside the Nix sandbox using a pinned `protoc`
version.  A PyPI wheel must be buildable without Nix.  The correct approach
for a PyPI release is to commit the pre-compiled artefacts into the source
tree.  Concretely, the `.pb` files can be post-processed by `prototext decode`
to produce textual `.textpb` representations, which are diff-friendly and
suitable for committing to Git.  The wheel build then either uses those
committed textual artefacts directly or re-encodes them at build time —
either way without requiring Nix or a pinned `protoc`.

Alternatively, a dedicated Nix derivation (`reprotoWheelSrc` or similar)
could prepare exactly the source tree needed for PyPI publication: starting
from `reprotoSrc`, it would run `prototext decode` on the `.pb` files and
produce a source directory that `twine upload` or `hatch publish` can consume
directly.  This approach keeps the preparation step reproducible and
sandboxed, while the actual upload remains a manual one-liner.  This is a
non-goal for the current overhaul but the design must not foreclose it.

The store path of `reprotoSrcFull` leaks into:
- `reprotoTests` (`PYTHONPATH`)
- `pythonLint` (`PYTHONPATH`, `pyrightconfig.json`)
- `pythonRuff` (argument to `ruff check`)
- `stressTests` (`PYTHONPATH`)
- the dev-shell `PYTHONPATH`

This tight coupling means any change to `reprotoSrc` or `patch_reproto.sh`
invalidates every downstream derivation simultaneously.

### O4 — `treeSitterTextproto` uses a split local/remote source

The `treeSitterTextproto` derivation takes its C sources from
`./reproto/tree-sitter-textproto` (local) but fetches `grammar.js` separately
via `treeSitterTextprotoSrc` (a `pkgs.fetchzip`).  The build phase splices them
by copying `grammar.js` into the local source before running
`tree-sitter generate`.

This means the grammar's generated `src/parser.c` is never committed and the
derivation silently depends on `nodejs` and `pkgs.tree-sitter` being available
in the Nix sandbox at build time.

### O5 — `protoPatchPhase` hardcoded and duplicated

`protoPatchPhase` hardcodes three specific `.proto` filenames
(`descriptor.proto`, `knife.proto`, `enum_collision.proto`).  The exact same
list is repeated verbatim in the dev-shell `shellHook`.  Adding a new fixture
schema requires updating both places independently.

### O6 — Monolithic 120-line dev-shell shellHook

The dev-shell `shellHook` performs, inline and sequentially:

1. Save/restore shell options
2. Export `NIXSHELL_REPO`, `PYO3_PYTHON`, `PATH`, `PYTHONPATH`
3. Write `.env` for VS Code / Pylance — two approaches are worth
   considering: (a) keep generating a dedicated file but rename it to
   something visible and self-describing like `python.env` (analogous to
   `ruff.toml`, `rust-toolchain.toml`); or (b) patch `.code-workspace`
   idempotently, anchoring on a well-known comment marker.  Option (a) is
   simpler; option (b) consolidates all VS Code configuration in one place.
   Decision deferred to the Specification phase.
4. Generate `pyrightconfig.json` via an inline `python3 -c "…"` script
5. Generate `ruff.toml`
6. Compile prototext fixture `.pb` descriptors (guarded)
7. Run `patch_reproto.sh` to seed reproto codegen (guarded)
8. Build `prototext` binary (guarded on staleness)
9. Export `RUSTFLAGS`
10. Generate man pages into `man/man1/` — the Nix package derivations
    (`prototext`, `reproto`, `protoscan`) each run the same generator
    binaries (`prototext-gen-man`, `reproto.gen_man`, `protoscan.gen_man`)
    during their own `installPhase`, writing into `$out/share/man/man1/`.
    The shellHook re-invokes those same generators independently, writing
    into `$PWD/man/man1/`.  There is no shared script; both paths call the
    same generator entry points directly.
11. Generate `rust-toolchain.toml` and install rustup toolchain — this step
    exists specifically to keep `rust-analyzer` (launched by VS Code)
    happy: rust-analyzer requires a `rust-toolchain.toml` in the workspace
    root to know which toolchain to use, and it must be installed in rustup's
    toolchain registry so rust-analyzer can locate the stdlib sources.
12. Source bash completions for prototext, reproto, protoscan

Each step is independent and could be a named function or separate script.
The current flat structure makes it hard to understand which steps are slow,
skip a step in isolation, or test a step without entering a full nix-shell.

Several shellHook steps copy artefacts from the Nix store into the working
tree (proto compilation, reproto codegen).  The `treeSitterTextproto`
derivation offers a better model: the compiled `.so` stays in the Nix store
and is exposed via `PYTHONPATH` pointing at the derivation output — nothing
is ever written to the working tree.  The overhaul should generalise this
approach and eliminate all working-tree writes that are driven by the
shellHook, where possible.

### O7 — Cargo flag inconsistency between clippy derivations

`rustClippy` uses `cargoExtraArgs = workspaceArgs` which expands to
`--no-default-features --workspace`.

`rustClippyPyo3` uses `cargoExtraArgs = pyo3Args` which expands to
`-p prototext_codec` — without `--no-default-features`.

`rustClippyScoringGraph` uses `cargoExtraArgs = scoringGraphArgs` which
expands to `-p scoring_graph_extension` — also without `--no-default-features`.

This inconsistency means default features are compiled for the two narrow
clippy derivations but not for the workspace-wide one.

### O8 — `ext` Nix/bash visual ambiguity in installPhase

All three extension `installPhase` blocks contain:

```bash
ext=${if pkgs.stdenv.isDarwin then "dylib" else "so"}
```

This is a Nix conditional evaluated at eval time (the result is a literal
string `"so"` or `"dylib"` in the generated bash), but it reads syntactically
as a bash variable assignment.  A reader unfamiliar with Nix string
interpolation may misread this as a runtime shell variable.

General code quality principle: (1) use idiomatic Nix syntax and constructs
wherever possible; (2) assume the developer reading this code is not a Nix
expert; (3) comment generously and explain tricky constructs inline.

### O9 — Stale `ci` derivation comment

The comment on line 895 reads:

> nix-build -A ci builds fmt → clippy → clippy-pyo3 → tests → prototext →
> prototext-codec → reproto → protoscan.

The actual `ci` linkFarm also includes `rustClippyScoringGraph`,
`scoringGraphLib`, `reprotoTests`, `pythonLint`, and `pythonRuff`, none of
which appear in the comment.

### O10 — No Nix flake

The project uses old-style `default.nix` / `shell.nix` with no `flake.nix`
or `flake.lock`.

Nix flakes are an experimental (but widely adopted) Nix feature that wraps a
project's Nix expressions in a `flake.nix` file with a locked
`flake.lock` for all inputs.  The main user-visible differences:

- `nix develop` replaces `nix-shell`; `nix build` replaces `nix-build`.
- Inputs (nixpkgs, crane, …) are pinned in `flake.lock` rather than via
  inline `fetchTarball` / `fetchgit` calls in `default.nix`.
- Downstream projects can reference this repo as a flake input and pin it
  with `nix flake update`.

Adopting flakes would require rewriting `default.nix` as `flake.nix` and
migrating all `nix-shell` / `nix-build` invocations and documentation.  This
is a substantial effort with no immediate functional benefit.

**Decision:** flakes adoption is a non-goal for this overhaul.  The project
stays on the classic `default.nix` / `shell.nix` model (Objective 7).

### O11 — `CARGO_PROFILE` env var usage

`commonArgs` sets `CARGO_PROFILE = "release"` as an environment variable.

Investigation of Crane's source (`configureCargoCommonVarsHook.sh`) shows
that `CARGO_PROFILE` is a Crane-internal shell variable (not exported, not
natively understood by Cargo).  Crane's `cargoWithProfile` helper reads it to
inject `--profile $CARGO_PROFILE` into every `cargo` invocation.  The hook
already sets `CARGO_PROFILE=${CARGO_PROFILE-release}` as its default, so
setting it explicitly to `"release"` in `commonArgs` is redundant.  A
variable that appears to be used but is in fact ignored is worse than no
variable at all: a reader who does not know Crane internals will assume it
controls compilation and may be confused when tracing why release mode is
active.

**Decision:** remove `CARGO_PROFILE = "release"` from `commonArgs`.  Instead,
add a comment near `commonArgs` explaining that Crane builds in release mode
by default (via the `configureCargoCommonVarsHook`) and that all shellHook
`cargo build` invocations must pass `--release` explicitly (see also D8).
Update D8 accordingly.

---

## Report on objectives vs. current state

### Objective 1 — Restructure top-level targets

The current model has `ci` (everything) and `default = ci`.  The new model
calls for:
- `ci` — builds all packages + runs quick tests and linters (replaces the
  current flat `ci` linkFarm; name retained as it is idiomatic for a
  "does everything needed to call the code shippable" target)
- `full-tests` — depends on `ci` + runs all remaining tests (stress tests,
  slow integration tests)
- `user-shell` — unchanged (`shell.nix`)
- `dev-shell` — unchanged (`dev-shell.nix`)

`ci` is the right name: it is the standard Nix convention for the attribute
that a CI system builds (`nix-build -A ci`), and it communicates intent
clearly to any reader.  `packages` is more of a flake convention and would
be unfamiliar here.  The key change from the current state is not the name
but the split: slow tests move out of `ci` and into `full-tests`.

### Objective 2 — Maximum artefact sharing

The single `depsCache` already handles Rust.  The main sharing gap is
between `nix-build` and `nix-shell`: Nix sandbox isolation makes sharing
Cargo build artefacts between them impossible in principle.

For Python, `reprotoSrcFull` is a shared store path, but it is referenced
directly by six different derivations and the shellHook (see O3).  "Leaks"
means: every consumer embeds the store path of `reprotoSrcFull` in its own
derivation closure, so any change to `reprotoSrc` or `patch_reproto.sh`
invalidates all six at once rather than just the one that actually changed.

Two mitigations are worth applying:

1. **Strict source filtering** — `reprotoSrc` (and thus `reprotoSrcFull`)
   must include only the files strictly necessary for the subsequent build
   steps.  Any spurious file inclusion (e.g. test fixtures, docs, editor
   config) causes unnecessary cache misses.  The `lib.fileset` API should be
   used to enumerate inputs precisely.

2. **Modularisation** — splitting `reprotoSrc` into finer-grained derivations
   (e.g. separating the `protoc`-compiled `.pb` files from the Python source
   copy) would allow each consumer to depend only on what it actually needs.
   However, the current coupling is inherent to the reproto build structure:
   the `.pb` files are produced alongside the Python source and all consumers
   need both.  The benefit of splitting is marginal unless the two parts
   change at different rates.  This is deferred to a future spec.

For PyO3 extensions, the three Crane `buildPackage` calls
(`prototextExtension`, `fdpscanExtension`, `scoringGraphExtension`) are
structurally identical.  A Nix helper function (a `let`-bound function in
`default.nix` that takes `{ crateName, cargoArgs, pyo3Dir, libName }` and
returns the full `crane.buildPackage` + `buildPythonPackage` pair) would
express the pattern once, keep fingerprints consistent, and guarantee that
all three extensions use identical `commonArgs` — no risk of one drifting
to a different set of `RUSTFLAGS` or `PYO3_PYTHON`.

### Objective 3 — Dev-shell hook compiles locally + prints recap

The shellHook already compiles locally (guarded `cargo build`).  What is
missing is the recap message — a clear printout of what was run (e.g.
`cargo build --release -p prototext`).  The O6 observation (monolithic hook)
is directly relevant here; structuring it as named steps would make it
natural to print a summary.

### Objective 4 — Dev-shell hook prepares pyrightconfig, env files

This is already done, but lives in the 120-line monolith (O6).  The overhaul
should extract it into a clearly named step.

### Objective 5 — Nix-build guide

Not currently present.  A `docs/nix-build-guide.md` is needed, covering the
Crane dep-cache strategy, the PyO3 extension pattern, the reproto codegen
stage, the `ci` / `full-tests` split, and the limitations (sandbox isolation,
no artefact sharing with dev-shell).

### Objective 6 — Detailed design

Goes in the Specification section of this spec, to be written once Goals
are confirmed.

### Objective 7 — Follow the general approach of the current default.nix

No flakes, same Crane usage, same Python packaging approach.  This rules
out a full migration to `flake.nix` (O10 is therefore a non-goal; see also
D7).  `CARGO_PROFILE = "release"` is removed from `commonArgs` per O11 and
D8; the intent is documented in a comment instead.

### Objective 8 — Maybe split default.nix into multiple files

Splitting is idiomatic Nix (each file is an expression returning an attrset,
imported with `import ./file.nix { inherit pkgs …; }`).  A natural split:

- `nix/rust.nix` — Crane derivations (depsCache, clippy, tests, binaries,
  PyO3 extensions)
- `nix/python.nix` — Python packages (prototextCodec, fdpScanLib,
  scoringGraphLib, reproto stages, protoscan)
- `nix/shells.nix` — user-shell, dev-shell
- `default.nix` — thin entry point: imports the above, composes `ci`,
  `full-tests`, shells, exposes all attributes

This is a legitimate approach and keeps each file under ~150 lines.

### Open question

Should O2 (redundant clippy derivations) be addressed?  Collapsing the three
into one workspace clippy would save CI time.  The original rationale for
keeping them separate was a macOS-specific build constraint: on
`macos-15-intel`, the `install_name_tool` command (used by the Nix stdenv to
patch binary load paths after the build) fails when the replacement path
exceeds ~512 bytes.  Chaining `prototextExtension` off `rustClippyPyo3`
means `prototextExtension` inherits `rustClippyPyo3`'s large `cargoArtifacts`
store path, which gets embedded in the binary's Mach-O load commands and
overflows the limit.  The separate derivations were introduced to break that
chain — but this applies to the _extension_ derivations, not to the clippy
checks themselves.  Merging the three clippy derivations into one does not
change what `prototextExtension` chains off of, so the macOS constraint is
irrelevant to this question.  Recommendation: drop `rustClippyPyo3` and
`rustClippyScoringGraph`; a single workspace-wide clippy is sufficient.

---

## Goals

1. Split `default.nix` into focused sub-files under `nix/`: `rust.nix`,
   `python.nix`, `shells.nix`; `default.nix` becomes a thin entry point.
2. Add a high-level pipeline diagram as a comment block near the top of each
   sub-file (D5).
3. Introduce a `makePyo3Extension` helper function that abstracts the
   repeated Crane build + `buildPythonPackage` pattern for all three PyO3
   extensions (O1, D4).
4. Rename `reprotoFilteredSrc` → `reprotoSrc` and `reprotoSrcWithCodegen` →
   `reprotoSrcFull` throughout (O3).
5. Tighten `reprotoSrc` source filtering using `lib.fileset` to include only
   files strictly needed by downstream derivations (Objective 2).
6. Drop `rustClippyPyo3` and `rustClippyScoringGraph`; replace with a single
   workspace-wide `rustClippy` using `--no-default-features --workspace`
   (O2, O7).
7. Remove `CARGO_PROFILE = "release"` from `commonArgs`; replace with an
   explanatory comment (O11, D8).
8. Split the current flat `ci` linkFarm into `ci` (packages + quick
   tests/linters) and `full-tests` (stress tests, slow integration tests)
   (D2).
9. Restructure the dev-shell `shellHook` as a sequence of named Bash
   functions, one per logical step, each printing a one-line recap of the
   command it ran (O6, Objective 3).
10. Decide and implement the VS Code `.env` file approach: rename to
    `python.env` (option a) or patch `.code-workspace` idempotently (option
    b) (O6 step 3).
11. Eliminate the double-compile in the three PyO3 extension derivations by
    building `--lib` and `--bin <crate>_post_build` in a single `cargo build`
    invocation and running the binary directly (S9).
12. Write `docs/nix-build-guide.md` covering the overall architecture, the
    Crane dep-cache strategy, the `makePyo3Extension` pattern, the reproto
    codegen pipeline, the `ci`/`full-tests` split, and the dev-shell
    conventions (Objective 5).

---

## Non-goals

- Adopting Nix flakes (O10, D7).
- Patching `prototext_post_build` to accept an explicit `.so` path (not
  needed — see S9; the fix is in the Nix derivation, not in Rust code).
- Publishing to PyPI, crates.io, or nixpkgs (D6 release venues; future work).
- Modularising `reprotoSrcFull` into finer-grained sub-derivations (Objective
  2, deferred).
- Changing any installed binary or Python package behaviour.
- Changing the GitHub Actions workflows.

---

## Specification

### S1 — File layout

```
default.nix          # thin entry point: imports nix/*.nix, exposes all attrs
nix/
  rust.nix           # Crane derivations: depsCache, fmt, clippy, tests,
                     #   prototext binary, three PyO3 extensions
  python.nix         # Python packages: prototextCodec, fdpScanLib,
                     #   scoringGraphLib, reprotoSrc, reprotoSrcFull,
                     #   reprotoBare, reproto, protoscan, lint/ruff checks
  shells.nix         # user-shell, dev-shell (shellHook)
```

`default.nix` imports each sub-file with `import ./nix/rust.nix { inherit
pkgs crane pythonBin …; }` and assembles `ci`, `full-tests`, and the
top-level attribute set.  No build logic lives in `default.nix` itself.

Each sub-file begins with a pipeline diagram comment (D5).

### S2 — `makePyo3Extension` helper

Defined in `nix/rust.nix` as a `let`-bound function:

```nix
makePyo3Extension = {
  # Cargo crate name (e.g. "prototext_codec")
  crateName,
  # Extra cargo args for the lib build (e.g. "-p prototext_codec")
  cargoArgs,
  # Path to the Python package source directory
  pyDir,
  # Base name of the .so / .pyi artefacts (e.g. "_pt_codec")
  libName,
}:
let
  ext = crane.buildPackage (commonArgs // {
    pname          = "${crateName}-extension";
    cargoExtraArgs = "${cargoArgs} --lib";
    doCheck        = false;
    cargoArtifacts = depsCache;
    # Build both the cdylib and the stub-generator binary in one invocation
    # (avoids a second full compilation of the crate — see S9).
    buildPhaseCargoCommand = "cargo build --release -p ${crateName} --lib --bin ${crateName}_post_build";
    preBuild       = "rm -f target/release/.fingerprint/${crateName}-*/invoked.timestamp";
    # Run the already-compiled stub-generator directly, with CARGO_MANIFEST_DIR
    # set so pyo3-stub-gen can locate pyproject.toml (the NotPresent panic
    # observed in spec 0038 was caused by this variable being absent, not by
    # a dynamic linking issue).
    postBuild      = "CARGO_MANIFEST_DIR=$PWD/${crateName} ./target/release/${crateName}_post_build";
    installPhase   = ''
      mkdir -p $out/artifacts
      cp target/release/lib${libName}.* $out/artifacts/
      cp target/release/${libName}.pyi  $out/artifacts/ 2>/dev/null || true
    '';
  });
in pythonPkgs.buildPythonPackage {
  pname     = crateName;
  version   = "0.1.0";
  format    = "pyproject";
  src       = pyDir;
  buildInputs = [ pythonPkgs.hatchling ext ];
  patchPhase  = ''
    cp ${ext}/artifacts/${libName}.* .
  '';
};
```

The three extensions are then expressed as:

```nix
prototextCodec    = makePyo3Extension { crateName = "prototext_codec";       … };
fdpScanLib        = makePyo3Extension { crateName = "fdp_scan";               … };
scoringGraphLib   = makePyo3Extension { crateName = "scoring_graph_extension"; … };
```

### S3 — Single `rustClippy`

Replace `rustClippy`, `rustClippyPyo3`, `rustClippyScoringGraph` with a
single derivation:

```nix
rustClippy = crane.cargoClippy (commonArgs // {
  cargoArtifacts  = depsCache;
  cargoExtraArgs  = "--no-default-features --workspace";
  cargoClippyExtraArgs = "-- -D warnings";
});
```

The `ci` CI workflow already skips clippy on `macos-15-intel` via
`skip-clippy: true`; that behaviour is unaffected.

### S4 — `ci` and `full-tests` targets

```nix
ci = pkgs.linkFarmFromDrvs "ci" [
  rustFmt rustClippy rustTests
  prototext prototextCodec fdpScanLib scoringGraphLib
  reproto protoscan
  reprotoTests pythonLint pythonRuff
];

full-tests = pkgs.linkFarmFromDrvs "full-tests" [
  ci stressDb stressTests
];
```

`default = ci` is retained so that `nix-build` with no `-A` flag still
builds the main target.

### S5 — `reprotoSrc` and `reprotoSrcFull` naming and source filter tightening

All occurrences of `reprotoFilteredSrc` are renamed to `reprotoSrc` and all
occurrences of `reprotoSrcWithCodegen` are renamed to `reprotoSrcFull`
throughout the Nix files.

As a final step of this spec, once all other changes are in place and
`nix-build -A ci` passes, the `reprotoSrc` filter is tightened using
`lib.fileset` to include only the files strictly needed by downstream
derivations.  The current `builtins.path` filter uses a predicate function;
`lib.fileset` provides a composable, explicit enumeration that makes the
included set auditable.  The expected result is a smaller, more stable store
hash for `reprotoSrc` that changes only when relevant source files change.

### S6 — Remove `CARGO_PROFILE = "release"` from `commonArgs`

Delete the line.  Add a comment above `commonArgs`:

```nix
# Crane defaults to --profile release via configureCargoCommonVarsHook.
# All cargo build invocations in the shellHook must pass --release explicitly.
```

### S7 — Restructured shellHook

The shellHook in `nix/shells.nix` is rewritten as a sequence of named Bash
functions called in order.  Each function prints a one-line header before
running and, where it executes a `cargo` or Python invocation, echoes the
exact command line.  Skeleton:

```bash
_hook_env()        { … }   # export NIXSHELL_REPO, PYO3_PYTHON, PATH, PYTHONPATH
_hook_python()     { … }   # write python.env, pyrightconfig.json, ruff.toml
                           #   (used by pyright, pylance, ruff — not vscode-specific)
_hook_protos()     { … }   # compile fixture .pb descriptors (guarded: skipped
                           #   if outputs are already up to date)
_hook_codegen()    { … }   # run patch_reproto.sh (guarded)
_hook_cargo()      { … }   # cargo build --release -p prototext (guarded)
_hook_rust()       { … }   # export RUSTFLAGS; write rust-toolchain.toml;
                           #   rustup toolchain install (keeps rust-analyzer happy)
_hook_man()        { … }   # generate man pages into man/man1/
_hook_completions(){ … }   # source bash completions

_hook_env
_hook_python
_hook_protos
_hook_codegen
_hook_cargo
_hook_rust
_hook_man
_hook_completions
```

The VS Code env file is renamed from `.env` to `python.env` (option a from
O6 step 3) — simpler than patching `.code-workspace`, and consistent with the
visible-name convention used by `ruff.toml` and `rust-toolchain.toml`.

### S8 — Nix-build guide

`docs/nix-build-guide.md` is written covering:

- Overall architecture and file layout (`default.nix`, `nix/*.nix`)
- The Crane dep-cache strategy and why all derivations share `depsCache`
- The `makePyo3Extension` pattern and how to add a new PyO3 crate
- The reproto codegen pipeline (`reprotoSrc` → `reprotoSrcFull`)
- The `ci` / `full-tests` split and when to use each
- Dev-shell conventions: what the shellHook does, what gets compiled locally,
  how to re-enter the shell after a source change
- Platform notes: macOS `install_name_tool` limit, `.so` vs `.dylib`
- Limitations: Nix sandbox isolation, no artefact sharing between `nix-build`
  and `nix-shell`

### S9 — Eliminate the PyO3 extension double-compile

Background: spec 0038 §F deferred this fix, attributing the failure to
`pyo3-stub-gen` requiring `cargo run`'s dynamic linker environment.  On
closer inspection, `pyo3-stub-gen`'s `StubInfoBuilder::build()` collects
type registrations via the `inventory` crate's static constructors, which
are embedded in the `prototext_codec_lib` rlib at link time.  The
`prototext_post_build` binary links against that rlib directly — no dynamic
loading of the `.so` occurs.  The `NotPresent` panic observed in spec 0038
was caused by a missing `CARGO_MANIFEST_DIR` when running the binary outside
of `cargo run`, not by a dynamic linking issue.

The fix:

1. In `buildPhaseCargoCommand`, build both targets in one invocation:
   ```bash
   cargo build --release -p <crate> --lib --bin <crate>_post_build
   ```
2. In `postBuild`, run the binary directly with `CARGO_MANIFEST_DIR` set:
   ```bash
   CARGO_MANIFEST_DIR=$PWD/<crate-dir> \
     ./target/release/<crate>_post_build
   ```

This eliminates the second full compilation of the crate (~27s per
extension, ~81s total across three extensions).  No changes to Rust source
code are required.  The fix applies identically to all three extensions
(`prototext_codec`, `fdp_scan_extension`, `scoring_graph_extension`) and is
incorporated into the `makePyo3Extension` helper (S2).

---

## Files changed

| File | Change |
|---|---|
| `default.nix` | Rewritten as thin entry point; all build logic moves to `nix/` |
| `nix/rust.nix` | New — Crane derivations, `makePyo3Extension` helper |
| `nix/python.nix` | New — Python packages, reproto pipeline |
| `nix/shells.nix` | New — user-shell, dev-shell with restructured shellHook |
| `shell.nix` | Updated to import `(import ./default.nix {}).user-shell` |
| `dev-shell.nix` | Updated to import `(import ./default.nix {}).dev-shell` |
| `docs/nix-build-guide.md` | New — build system guide |

---

## Implementation order

1. Create `nix/` directory; split `default.nix` into `rust.nix`,
   `python.nix`, `shells.nix` with no logic changes — verify `nix-build -A
   ci` still passes.
2. Apply renames: `reprotoFilteredSrc` → `reprotoSrc`,
   `reprotoSrcWithCodegen` → `reprotoSrcFull`.
3. Remove `CARGO_PROFILE = "release"`; add explanatory comment (S6).
4. Collapse three clippy derivations into one `rustClippy` (S3).
5. Introduce `makePyo3Extension` incorporating the double-compile fix (S2,
   S9); replace the three copy-pasted blocks.
6. Split `ci` / `full-tests` (S4).
7. Restructure shellHook as named functions; rename `.env` → `python.env`
   (S7).
8. Run `nix-build -A ci`, `nix-build -A full-tests`, and
   `nix-shell dev-shell.nix --run "echo ok"` on all four platforms.
9. Write `docs/nix-build-guide.md` (S8).
10. Add pipeline diagram comment blocks to each `nix/*.nix` file (D5).
11. Tighten `reprotoSrc` filter using `lib.fileset`; verify `nix-build -A ci`
    still passes and the store hash for `reprotoSrc` is smaller (S5).

---

## Test plan

- `nix-build -A ci` passes on all four CI platforms.
- `nix-build -A full-tests` passes.
- `nix-shell shell.nix --run "echo ok"` exits cleanly.
- `nix-shell dev-shell.nix --run "echo ok"` exits cleanly and prints a recap
  of each shellHook step.
- `reuse lint` passes.
- `pyright` and `ruff` pass inside the dev-shell.
