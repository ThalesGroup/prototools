<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# default.nix — Build Analysis and Recommendations

## 1. What was built (nix-build log)

15 derivations were built in dependency order:

```
prototools-deps-deps          (depsCache — Rust deps, non-pyo3)
prototext-codec-deps-deps     (pyo3DepsCache — Rust deps, pyo3)
prototools-fmt-fmt            (rustFmt)
prototools-clippy-clippy      (rustClippy, from depsCache)
prototools-clippy-pyo3-clippy (rustClippyPyo3, from pyo3DepsCache)
prototools-tests-test         (rustTests, from depsCache)
prototext-codec-ext           (prototextExtension, from pyo3DepsCache)
prototools                    (prototools binary, from rustTests)
python3.12-prototext_codec    (prototextCodec Python wheel)
python3-3.12.12-env           (Python env for reproto-tests)
python3.12-reproto            (reproto Python wheel)
python-lint                   (pythonLint — pyright)
python-ruff                   (pythonRuff)
reproto-tests                 (reprotoTests — pytest, 61 tests)
ci                            (linkFarm aggregating all of the above)
+ prototools-dev (dev-shell)
```

## 2. What was built (nix-shell log)

The shell hook compiled only:

```
prototext-core v0.1.2
prototext v0.1.4
Finished `release` profile in 23.34s
wrote man/man1/prototext.1
```

The shell hook triggers `cargo build --release --locked -p prototext` unconditionally
on every nix-shell entry.  None of the Nix store derivations (depsCache etc.) were
reused by the shell build; cargo used its own `target/release/` working tree.

---

## 3. Code analysis of default.nix

### 3.1 Hardcoded values that appear multiple times

#### Python version string

`pythonPkgs.python.pythonVersion` is used twice:

- Line 177: `RUSTFLAGS = "-L ${pythonBin}/lib -lpython${pythonPkgs.python.pythonVersion}";`
- Line 492 (shellHook): `export RUSTFLAGS="-L ${pythonBin}/lib -lpython${pythonPkgs.python.pythonVersion}"`

The expression is identical. The shellHook copies it by hand instead of referencing a
named binding.

#### cargoExtraArgs exclusion pattern `--no-default-features --workspace --exclude prototext_codec`

This exact string appears **four times**:

| Location | Derivation |
|---|---|
| depsCache | `cargoExtraArgs = "--no-default-features --workspace --exclude prototext_codec";` |
| rustClippy | `cargoExtraArgs = "--no-default-features --workspace --exclude prototext_codec";` |
| rustTests | `cargoExtraArgs = "--no-default-features --workspace --exclude prototext_codec";` |
| prototools | `cargoExtraArgs = "--no-default-features --workspace --exclude prototext_codec";` (except scoped to `-p prototext`) |

A change (e.g., adding a new crate to exclude) must be applied in four places.

#### pyo3 crate selector `-p prototext_codec`

Appears three times:

- `pyo3DepsCache`: `cargoExtraArgs = "-p prototext_codec";`
- `rustClippyPyo3`: `cargoExtraArgs = "-p prototext_codec";`
- `prototextExtension`: `cargoExtraArgs = "-p prototext_codec --lib";`

#### `reprotoPropagatedDeps` list

Used in three places:

- `reprotoBare.propagatedBuildInputs`
- `reproto.propagatedBuildInputs` (via `reprotoPropagatedDeps ++ [prototextCodec]`)
- `reprotoTests.buildInputs` (via `reprotoPropagatedDeps ++ [...]`)
- `pythonLint.buildInputs` (via `reprotoPropagatedDeps ++ [...]`)
- `dev-shell` PYTHONPATH expression (via `reproto.propagatedBuildInputs`)

This is already factored into a named binding — good.  But the pattern
`reprotoPropagatedDeps ++ [ prototextCodec pythonPkgs.pytest ... ]` is repeated
verbatim in `reprotoTests` and `pythonLint` with slight variations, and is subtly
different from what `reproto.propagatedBuildInputs` contains.

#### `nativeBuildInputs` for protoc phases

`protoPatchPhase` requires `pkgs.protobuf` in `nativeBuildInputs`.  This addition
appears twice:

- `rustClippy`: `nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.protobuf ];`
- `rustTests`:  `nativeBuildInputs = (commonArgs.nativeBuildInputs or []) ++ [ pkgs.protobuf ];`

Note the inconsistency: one uses `commonArgs.nativeBuildInputs`, the other uses
`(commonArgs.nativeBuildInputs or [])` — the `or []` fallback is unnecessary since
`commonArgs` always defines `nativeBuildInputs`.

### 3.2 Compilation of the same source observed in the log

The nix-build log shows `prototext-core` and `prototext` compiled multiple times in
separate sandboxes, which is expected for Nix (each derivation gets its own sandbox).
However, the following compilations are structurally redundant:

| Phase | What is compiled | Time |
|---|---|---|
| pyo3DepsCache buildPhase | Full pyo3+prototext-core tree (`cargo build --release -p prototext_codec`) | ~82s |
| pyo3DepsCache checkPhase | Recompiles prototext-core+prototext for test binaries (`cargo test --no-run`) | ~106s |
| prototextExtension | Recompiles prototext_codec (after clearing fingerprints) | ~27s (stale cache cleared) |
| prototextExtension postBuild | Recompiles prototext_codec again for post_build binary | ~27s |

The `pyo3DepsCache` check phase (`cargo test --release --no-default-features
--workspace --exclude prototext_codec --no-run`) runs inside the pyo3DepsCache
derivation and compiles `prototext-core` and `prototext` — these are non-pyo3 crates
that are separately and more efficiently compiled by `depsCache`. This cross-
contamination happens because `pyo3DepsCache` uses `buildDepsOnly` which internally
runs a check pass.

Within `prototextExtension`, the `preBuild` clears Cargo fingerprints for
`prototext_codec-*` and then the build compiles the `.so`. Then `postBuild` runs
`cargo run -p prototext_codec --bin prototext_post_build`, which causes a second
compilation of `prototext_codec` (the `post_build` binary target). Two compilations
of `prototext_codec` happen back-to-back (lines 308-375 and 448-450).

### 3.3 nix-build vs nix-shell: no artifact sharing

The nix-shell hook runs `cargo build --release --locked -p prototext` in the working
tree's `target/` directory.  The Nix store artifacts (built by nix-build) are in
`/nix/store` and are completely separate from the working tree.  This means:

- After a `nix-build`, entering `nix-shell` recompiles `prototext-core` and
  `prototext` from scratch (23s in the log).
- After a `nix-shell`, `nix-build` does not benefit from the working-tree build.

This is fundamental to how Nix works (sandbox isolation), so it cannot be eliminated
entirely.  But the shell hook could be smarter about skipping the build when the
binary is already up to date (currently it does not check timestamps or hashes).

### 3.4 Structural readability issues

#### `commonArgs` is not truly common

`commonArgs` is defined as a base set, but several derivations redefine
`nativeBuildInputs` from scratch by appending to `commonArgs.nativeBuildInputs`
rather than using a helper.  There is no intermediate binding like `rustCommonArgs`
or `withProtobuf` to group the repeated extensions.

#### `pyo3CommonArgs` is well-factored but incomplete

`pyo3CommonArgs = commonArgs // { env.PYO3_PYTHON = ...; RUSTFLAGS = ...; ... }` is a
clean pattern.  However, `RUSTFLAGS` also appears verbatim in the shellHook (see
§3.1), breaking the single-source-of-truth principle.

#### `buildDepsOnly` check phase is counterproductive for pyo3

`crane.buildDepsOnly` by default runs `cargo check` as a check phase.  For
`pyo3DepsCache`, this causes a broad workspace check (including non-pyo3 crates) that
duplicates work already done by `depsCache`.  The `buildPhaseCargoCommand` override
bypasses the build phase but not the check phase.

#### shellHook `cargo build` is unconditional

Every `nix-shell` entry triggers a full `cargo build --release` invocation.  Cargo's
incremental compilation means it usually exits quickly if nothing changed, but on a
fresh clone (or after `nix-build` which wrote nothing to `target/`) it does a full
compile. A guard (`if [[ ! -f target/release/prototext ]]`) would save ~23s on first
entry when nix-build artifacts are already installed.

---

## 4. Recommendations

### R1 — Extract repeated `cargoExtraArgs` into named bindings

```nix
# Proposed
workspaceExcludePyo3 = "--no-default-features --workspace --exclude prototext_codec";
pyo3CrateArgs        = "-p prototext_codec";
```

Then:

```nix
depsCache    = crane.buildDepsOnly (commonArgs // { cargoExtraArgs = workspaceExcludePyo3; ... });
rustClippy   = crane.cargoClippy  (commonArgs // { cargoExtraArgs = workspaceExcludePyo3; ... });
rustTests    = crane.cargoTest    (commonArgs // { cargoExtraArgs = workspaceExcludePyo3; ... });
prototools   = crane.buildPackage (commonArgs // { cargoExtraArgs = "--no-default-features -p prototext"; ... });

pyo3DepsCache       = crane.buildDepsOnly (pyo3CommonArgs // { cargoExtraArgs = pyo3CrateArgs; ... });
rustClippyPyo3      = crane.cargoClippy  (pyo3CommonArgs // { cargoExtraArgs = pyo3CrateArgs; ... });
prototextExtension  = crane.buildPackage (pyo3CommonArgs // { cargoExtraArgs = "${pyo3CrateArgs} --lib"; ... });
```

### R2 — Extract `rustflags` and reference it from the shellHook

```nix
pyo3Rustflags = "-L ${pythonBin}/lib -lpython${pythonPkgs.python.pythonVersion}";

pyo3CommonArgs = commonArgs // {
  env.PYO3_PYTHON = pythonExecutable;
  RUSTFLAGS       = pyo3Rustflags;
  ...
};
```

shellHook then uses:

```bash
export RUSTFLAGS="${pyo3Rustflags}"
```

This ensures `RUSTFLAGS` is always consistent with what Crane uses.

### R3 — Normalize the two `nativeBuildInputs` patterns

Replace the inconsistent:

```nix
# rustClippy:
nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.protobuf ];
# rustTests:
nativeBuildInputs = (commonArgs.nativeBuildInputs or []) ++ [ pkgs.protobuf ];
```

with a single shared binding:

```nix
nativeBuildInputsWithProtoc = commonArgs.nativeBuildInputs ++ [ pkgs.protobuf ];
```

and use it in both `rustClippy` and `rustTests`.

### R4 — Suppress the spurious pyo3DepsCache check phase

`pyo3DepsCache` runs `cargo check` over the whole workspace as a side-effect of
`buildDepsOnly`.  This can be suppressed:

```nix
pyo3DepsCache = crane.buildDepsOnly (pyo3CommonArgs // {
  pname          = "prototext-codec-deps";
  cargoExtraArgs = pyo3CrateArgs;
  doCheck        = false;
  checkPhaseCargoCommand = "";   # skip the implicit cargo check pass
  buildPhaseCargoCommand = "cargoWithProfile build ${pyo3CrateArgs}";
});
```

This avoids the ~106s `cargo test --no-run` over the full workspace inside the pyo3
deps sandbox.

### R5 — Guard the shellHook `cargo build` on binary existence

```bash
if [[ ! -f "$PWD/target/release/prototext" ]] || \
   [[ "$PWD/prototext/src/main.rs" -nt "$PWD/target/release/prototext" ]]; then
  cargo build --release --locked -p prototext
fi
```

This saves ~23s on nix-shell entry when the binary is already current.  Cargo's own
incremental check is still the fallback if the guard is too coarse.

### R6 — Unify reproto test dependency lists

`reprotoTests` and `pythonLint` both construct `reprotoPropagatedDeps ++ [extra ...]`
with overlapping but not identical extras.  Extract a named binding:

```nix
reprotoTestDeps = reprotoPropagatedDeps ++ [
  prototextCodec
  pythonPkgs.pytest
  pythonPkgs."pytest-xdist"
  pythonPkgs.tree-sitter
  pythonPkgs.tree-sitter-language-pack
];
```

Then `reprotoTests` uses `reprotoTestDeps` and `pythonLint` uses
`reprotoPropagatedDeps ++ [ pythonPkgs.pytest pythonPkgs.tree-sitter ... ]` (no
`prototextCodec` since pyright resolves it via `PYTHONPATH`), or alternatively both
share `reprotoTestDeps` if pyright can tolerate the extra package.

### R7 — Consider splitting prototextExtension postBuild into its own derivation

The `prototextExtension` derivation compiles `prototext_codec` twice:
once for the `.so` (buildPhase) and once for the `prototext_post_build` binary
(postBuild).  Splitting the post_build step into a thin `pkgs.runCommand` that runs
the pre-built binary from the extension derivation would eliminate the second
compilation.

---

## 5. Summary table

| Issue | Impact | Fix |
|---|---|---|
| `--no-default-features --workspace --exclude prototext_codec` repeated 4x | Maintenance | R1 |
| `-p prototext_codec` repeated 3x | Maintenance | R1 |
| `RUSTFLAGS` duplicated in pyo3CommonArgs and shellHook | Correctness risk | R2 |
| `nativeBuildInputs or []` inconsistency | Readability | R3 |
| pyo3DepsCache runs full-workspace cargo check | Redundant ~106s compile | R4 |
| shellHook always recompiles prototext | ~23s on every nix-shell entry | R5 |
| reproto test dep lists duplicated | Maintenance | R6 |
| prototextExtension compiles prototext_codec twice | ~27s redundant compile | R7 |

---

## 6. Envisioned target layout of default.nix

The goal is a file where every value is defined exactly once, derivations inherit
from named bases, and the reader can understand what each derivation adds without
hunting for scattered overrides.

### 6.1 Proposed section order

```
1. Inputs and nixpkgs pin          (already good)
2. External tools: crane           (already good)
3. Source filters: src, reprotoFilteredSrc
4. Python interpreter handles: pythonBin, pythonExecutable
5. Named constants — ALL flag strings and shared lists
       workspaceArgs, pyo3Args, pyo3Rustflags,
       nativeBuildInputsWithProtoc,
       reprotoPropagatedDeps, reprotoTestDeps
6. Shared build argument sets (base attrsets)
       commonArgs, pyo3CommonArgs
7. Shared build snippets: protoPatchPhase
8. Rust/Crane derivations (in dependency order)
       depsCache → rustFmt, rustClippy
       pyo3DepsCache → rustClippyPyo3
       rustTests → prototools
       prototextExtension → prototextCodec
9. Python/reproto derivations (in dependency order)
       reprotoBare → reprotoSrcWithCodegen → reproto
       reprotoTests, pythonLint, pythonRuff
10. CI aggregate: ci
11. Dev shell: dev-shell
12. Output attrset
```

The key discipline: sections 3–7 contain **no derivations**, only values.  Every
`crane.*` or `buildPythonPackage` call appears in sections 8–11.  This makes it
easy to find every derivation and to see at a glance what is shared.

### 6.2 Named constants section (full proposed content)

```nix
# ---------------------------------------------------------------------------
# Named constants — single source of truth for all repeated values.
# ---------------------------------------------------------------------------

pythonBin        = pythonPkgs.python;
pythonExecutable = "${pythonBin}/bin/python";

# Cargo flags for the non-pyo3 workspace (all crates except prototext_codec).
workspaceArgs = "--no-default-features --workspace --exclude prototext_codec";

# Cargo package selector for all pyo3 derivations.
pyo3Args = "-p prototext_codec";

# RUSTFLAGS for linking against CPython.  Must be identical in pyo3CommonArgs
# and the shellHook so that Cargo fingerprints stay aligned.
pyo3Rustflags = "-L ${pythonBin}/lib -lpython${pythonPkgs.python.pythonVersion}";

# nativeBuildInputs for any Crane derivation that runs protoc (patch phase).
nativeBuildInputsWithProtoc = commonArgs.nativeBuildInputs ++ [ pkgs.protobuf ];

# Runtime Python deps for reproto (no test tools, no codec extension).
reprotoPropagatedDeps = [ ... ];

# Full Python dep set for tests, lint, and dev-shell PYTHONPATH.
reprotoTestDeps = reprotoPropagatedDeps ++ [
  prototextCodec
  pythonPkgs.pytest
  pythonPkgs."pytest-xdist"
  pythonPkgs.tree-sitter
  pythonPkgs.tree-sitter-language-pack
];
```

### 6.3 Unifying commonArgs and pyo3CommonArgs (validated by research)

The current split into `commonArgs` / `pyo3CommonArgs` and `depsCache` /
`pyo3DepsCache` was investigated against authoritative sources:

- The **crane FAQ** (crane.dev/faq/rebuilds-pyo3.html) only recommends setting
  `PYO3_PYTHON` explicitly to avoid `$PATH`-triggered rebuilds.  It does not
  recommend a separate dep cache for pyo3 crates.
- The **Nickel project** (github.com/tweag/nickel), a real-world crane + pyo3
  workspace, uses a **single** `buildDepsOnly` with `env.PYO3_PYTHON` and
  `python3` in `nativeBuildInputs` for the full workspace — no `--exclude`, no
  separate pyo3 cache.
- `crane.buildDepsOnly` stubs `build.rs` with a dummy source.  The pyo3 build
  script runs fine inside `buildDepsOnly` as long as `PYO3_PYTHON` is available.
  Nickel confirms this in practice.
- `-lpython` in `RUSTFLAGS` is a linker flag.  For `rlib` crates it is a no-op
  (the linker is never invoked).  For fingerprinting purposes, Cargo includes
  `RUSTFLAGS` in every crate's fingerprint — so all derivations in a chain must
  carry the same value, but there is no correctness problem in carrying it
  globally.

**Conclusion:** the `pyo3CommonArgs` / `pyo3DepsCache` split is not idiomatic and
not necessary.  The correct target is a single `commonArgs` that includes
`PYO3_PYTHON`, `pyo3Rustflags`, and `pythonBin`, and a single `depsCache`
covering the whole workspace.  The `--exclude prototext_codec` flag (and the
`workspaceArgs` binding) disappears; `pyo3CommonArgs` and `pyo3DepsCache` are
eliminated entirely.

### 6.4 Base argument sets (hierarchic composition)

With the unified `commonArgs`, the hierarchy simplifies to two levels:

```nix
# Level 0 — base for ALL Crane derivations (Rust + pyo3)
commonArgs = {
  inherit src;
  pname             = "prototools";
  version           = "...";
  strictDeps        = true;
  nativeBuildInputs = [ pkgs.cargo pkgs.rustc pythonBin ];
  env.PYO3_PYTHON   = pythonExecutable;
  RUSTFLAGS         = pyo3Rustflags;
};

# Level 1 — derivations that also invoke protoc (patch phase)
protocArgs = commonArgs // {
  nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.protobuf ];
  patchPhase        = protoPatchPhase;
};
```

Every derivation then adds only what is truly specific to it:

```nix
# Single shared dep cache for the whole workspace
depsCache  = crane.buildDepsOnly (commonArgs // { pname = "prototools-deps"; });

rustFmt    = crane.cargoFmt      (commonArgs // { pname = "prototools-fmt"; });

rustClippy = crane.cargoClippy  (protocArgs  // {
  pname                = "prototools-clippy";
  cargoArtifacts       = depsCache;
  cargoExtraArgs       = "--no-default-features --workspace";
  cargoClippyExtraArgs = "--all-targets -- --deny warnings";
});

rustClippyPyo3 = crane.cargoClippy (commonArgs // {
  pname                = "prototools-clippy-pyo3";
  cargoArtifacts       = depsCache;
  cargoExtraArgs       = "-p prototext_codec";
  cargoClippyExtraArgs = "--all-targets -- --deny warnings";
});

rustTests  = crane.cargoTest    (protocArgs  // {
  pname          = "prototools-tests";
  cargoArtifacts = depsCache;
  cargoExtraArgs = "--no-default-features --workspace";
});

prototools = crane.buildPackage (protocArgs  // {
  pname             = "prototools";
  cargoArtifacts    = rustTests;
  cargoExtraArgs    = "--no-default-features -p prototext";
  nativeBuildInputs = protocArgs.nativeBuildInputs ++ [ pkgs.installShellFiles ];
  doCheck           = false;
  postInstall       = ...;
  meta              = ...;
});

prototextExtension = crane.buildPackage (commonArgs // {
  pname          = "prototext-codec-ext";
  cargoArtifacts = depsCache;
  cargoExtraArgs = "-p prototext_codec --lib";
  ...
});
```

Note that `rustClippy` and `rustTests` now use `--no-default-features --workspace`
(without `--exclude prototext_codec`) — the pyo3 crate is included in the workspace
build because `PYO3_PYTHON` is available in all sandboxes.  `rustClippyPyo3` is
retained as a separate target so that pyo3-specific clippy lints can be run
independently (and because it chains off the same `depsCache`, adding no extra
compilation cost).

### 6.5 Eliminating the remaining double-compile in prototextExtension

`prototextExtension` currently builds `prototext_codec` twice: once as a `--lib`
(the `.so`) and once via `cargo run --bin prototext_post_build` (the stub generator).
The proposed fix is to add `prototext_post_build` to the primary build target:

```nix
cargoExtraArgs = "${pyo3Args} --lib --bin prototext_post_build";
```

Then `postBuild` can run the already-compiled binary directly from `target/release/`
instead of triggering a second `cargo run` compilation pass.

### 6.6 shellHook alignment with nativeBuildInputsWithProtoc

Currently the shellHook lists tools manually (`cargo`, `rustc`, `rustfmt`, etc.)
without referencing any of the named Nix bindings.  Two improvements:

- **`RUSTFLAGS`**: already references `pyo3Rustflags` after P1 (done).
- **PYTHONPATH**: already references `reprotoTestDeps` after P3 (done).
- **Remaining manual tool list**: cannot easily reference Nix attrsets from inside a
  bash string, so this stays manual — but a comment linking it to `commonArgs` would
  at least make the relationship explicit.

### 6.7 What would remain legitimately different between nix-build and nix-shell

Even with the above improvements, one structural asymmetry is unavoidable:

| Aspect | nix-build | nix-shell |
|---|---|---|
| Rust build | isolated Nix sandbox, fixed store paths | working tree `target/`, incremental |
| Python path | computed from store derivations | `$PWD/reproto/src` + `reprotoTestDeps` |
| protoc seed | via `reprotoSrcWithCodegen` derivation | conditional bash guard in shellHook |
| prototext binary | built by `prototools` derivation | `cargo build --release -p prototext` |

The `nix-shell` will always do some work that `nix-build` already did, because Nix
sandbox isolation forbids sharing `target/`.  The guard proposed in P4 (check binary
staleness before recompiling) is the practical mitigation.
