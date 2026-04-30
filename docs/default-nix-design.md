# default.nix — Design, Analysis, and Proposals

## 1. Overview

`default.nix` is a single-file Nix expression that drives the entire
build, test, lint, and development-shell lifecycle of the prototools
workspace.  It is parameterised by two arguments:

| Argument | Default | Purpose |
|---|---|---|
| `pkgs` | nixos-25.11 @ pinned rev | nixpkgs universe |
| `pythonPkgs` | `pkgs.python312Packages` | Python package set |

The file exposes a flat attribute set at the bottom; every attribute is
a distinct Nix derivation or shell.  `default` and `ci` both point at
the full CI closure.

---

## 2. Crate / Package Topology

The Cargo workspace has three crates:

```
prototext-core   (lib)     — pure Rust codec + schema
prototext        (bin)     — CLI, depends on prototext-core
prototext_codec  (cdylib + bin)  — PyO3 extension, depends on prototext-core
```

`prototext_codec` is special: it links against libpython and therefore
needs `PYO3_PYTHON` / `RUSTFLAGS` that are **incompatible** with the
plain Rust build.  This is the root reason for the two-track build
described below.

---

## 3. Build Stages and Dependency Graph

### 3.1 Shared source filter (`src`)

```
src = cleanSourceWith { filter = ... }
```

Filters the repo to only Cargo-relevant files plus `fixtures/`.
Excludes `reproto/` and `bin/` so that changes there do not invalidate
Rust derivation hashes.

### 3.2 Common argument sets

Two base records are assembled and merged into derivations with `//`:

| Record | Contents |
|---|---|
| `commonArgs` | `src`, pname/version, `strictDeps`, `cargo`/`rustc` |
| `pyo3CommonArgs` | `commonArgs // { PYO3_PYTHON, RUSTFLAGS, pythonBin }` |

### 3.3 Dependency caches (the two Crane tracks)

```
depsCache     = crane.buildDepsOnly (commonArgs // { --exclude prototext_codec })
pyo3DepsCache = crane.buildDepsOnly (pyo3CommonArgs // { -p prototext_codec })
```

These are the most expensive derivations.  Crane's `buildDepsOnly`
compiles only dependency crates; the workspace crate sources are
replaced with stub `lib.rs` / `main.rs` files so that the cache is
reusable whenever only workspace source changes.

`depsCache` covers `prototext-core` + `prototext` (and transitively all
their Cargo dependencies).  `pyo3DepsCache` covers `prototext_codec`
plus `prototext-core` again (because it is a dependency of
`prototext_codec`).

### 3.4 Lint derivations

| Derivation | Crane call | Input cache |
|---|---|---|
| `rustFmt` | `cargoFmt` | none (only source) |
| `rustClippy` | `cargoClippy` | `depsCache` |
| `rustClippyPyo3` | `cargoClippy` | `pyo3DepsCache` |

### 3.5 Test derivation

```
rustTests = crane.cargoTest (commonArgs // {
    cargoArtifacts = depsCache;
    -- adds pkgs.protobuf (protoc) to nativeBuildInputs
})
```

Excludes `prototext_codec`; `protoc` is needed for integration tests.

### 3.6 Final Rust binary

```
prototools = crane.buildPackage (commonArgs // {
    cargoArtifacts = rustTests;   -- chains off tests (not depsCache)
    -p prototext
})
```

By chaining off `rustTests`, Nix implicitly requires tests to have
passed before the binary can exist.

### 3.7 PyO3 extension chain

```
pyo3DepsCache  →  prototextExtension (cdylib + .pyi stub)
                        ↓
                  prototextCodec  (Python wheel via buildPythonPackage)
```

`prototextExtension` uses a `preBuild` hook to wipe Cargo fingerprints
so that the pyo3 build script re-runs (needed to regenerate the C
header).  `postBuild` runs `prototext_post_build` to generate the `.pyi`
stub.  `installPhase` copies `.so` + `.pyi` into `$out/artifacts/`.

`prototextCodec` is a `buildPythonPackage` that patches
`prototext_codec_lib/{.so,.pyi}` in place before building the wheel.

### 3.8 reproto Python chain

```
reprotoFilteredSrc                 (builtins.path filter)
       ↓
reprotoBare                        (buildPythonPackage, doCheck=false)
       ↓
reprotoSrcWithCodegen              (runCommand: seeds .proto files, runs patch_reproto.sh)
       ↓
reproto                            (buildPythonPackage, doCheck=false)
reprotoTests                       (runCommand: pytest)
pythonLint                         (runCommand: pyright)
pythonRuff                         (runCommand: ruff check)
```

`reprotoPropagatedDeps` is a shared list used by `reprotoBare`,
`reproto`, `reprotoTests`, and `pythonLint`, which is the one existing
de-duplication pattern in the file.

### 3.9 CI closure

```
ci = linkFarmFromDrvs "ci" [
    rustFmt rustClippy rustClippyPyo3 rustTests
    prototools prototextCodec
    reproto reprotoTests pythonLint pythonRuff
]
```

Building `ci` forces the entire DAG.

### 3.10 Development shell (`dev-shell`)

`pkgs.mkShell` with `nativeBuildInputs` listing tools (cargo, rustc,
rustfmt, clippy, reuse, gh, protobuf, buf, mandoc, zola, pytest, ruff).

The `shellHook` performs at entry time:
1. Sets `NIXSHELL_REPO`, `PYO3_PYTHON`, `PATH`, `PYTHONPATH`, `.env`.
2. Regenerates `pyrightconfig.json` from `$PYTHONPATH`.
3. Writes `ruff.toml` (exclude `docs/mockup`).
4. Seeds `.proto` files and runs `patch_reproto.sh` if descriptors are absent.
5. Runs `cargo build --release -p prototext` (unconditional Cargo invocation).
6. Sets `RUSTFLAGS` (after the prototext build, to avoid invalidating its fingerprint).
7. Generates man pages via `prototext-gen-man`.
8. Writes `rust-toolchain.toml` to lock rust-analyzer to the nix-shell rustc version.
9. Sources bash completions for `prototext` and `reproto`.

---

## 4. Current Issues and Shortcomings

### 4.1 `prototext-core` compiled three times

The crate `prototext-core` is a dependency of both tracks.  Because
`commonArgs` and `pyo3CommonArgs` differ in `RUSTFLAGS` (and other env
vars), Cargo treats them as separate compilation units.  Concretely:

- `depsCache` compiles `prototext-core` without `RUSTFLAGS` → `.rlib` A
- `pyo3DepsCache` compiles `prototext-core` with `-lpython` → `.rlib` B

`.rlib` A is then used by `rustTests` → `prototools`.
`.rlib` B is used by `prototextExtension`.

This is unavoidable as long as `RUSTFLAGS` differs between tracks.
However, because `prototext-core` does **not** link Python, the `-lpython`
flag is irrelevant to it.  The linker flag forces a redundant
recompilation of a crate that does not need it.

### 4.2 `rustTests` used as artifact input to `prototools` (unnecessary chaining)

`prototools` sets `cargoArtifacts = rustTests`.  The intent is to
express "tests must pass before shipping the binary."  However, this
means Nix also copies test build artifacts (e.g., test harness `.rlib`s)
into the `prototools` derivation sandbox before the final build, bloating
the closure and serialising what could be parallel derivations.

The correct pattern is to let both `rustTests` and `prototools` chain
off `depsCache` (or `rustClippy`), and instead use `ci` to enforce
ordering at the meta-level.  Nix's `ci = linkFarmFromDrvs` already
expresses the full dependency order.

### 4.3 `dev-shell` duplicates tool lists from derivations

`dev-shell.nativeBuildInputs` lists cargo, rustc, rustfmt, clippy,
pytest, ruff manually.  These are the same tools already encoded in
`commonArgs.nativeBuildInputs`, `rustFmt`, `rustClippy`, `rustTests`,
`pythonLint`, `pythonRuff`, etc.  If a tool version is upgraded in one
place (e.g., pytest added to `reprotoTests`), the shell silently diverges.

### 4.4 `PYTHONPATH` in `shellHook` diverges from `reprotoTests`

`shellHook` builds `PYTHONPATH` by calling
`pythonPkgs.makePythonPath (reproto.propagatedBuildInputs ++ [...])`.
`reprotoTests` builds its Python environment independently via
`pythonPkgs.python.withPackages`.  The two lists are duplicated in the
Nix source and can drift.  For example, `tree-sitter` and
`tree-sitter-language-pack` appear in `reprotoTests` and in
`shellHook`'s extra list but not in `reproto.propagatedBuildInputs`.

### 4.5 `pythonLint` missing `tree-sitter*` packages

`pythonLint`'s `withPackages` call omits `tree-sitter` and
`tree-sitter-language-pack`.  If any reproto module conditionally
imports those at type-check time, pyright would fail to resolve them.
(`reprotoTests` includes them; `pythonLint` does not.)

### 4.6 `dev-shell` unconditionally runs `cargo build` on every shell entry

```bash
cargo build --release --locked -p prototext
```

This is guarded only by Cargo's own up-to-date check.  On a clean
checkout or after any Rust source change it recompiles synchronously
during shell startup, blocking the prompt.  There is no progress
feedback and no way to skip it.

### 4.7 `reprotoSrcWithCodegen` is rebuilt when `pyo3DepsCache` changes

`reprotoSrcWithCodegen` depends on `reprotoBare`, which depends on
`reprotoPropagatedDeps`.  If any Python dependency version changes, the
codegen stage (which only shells out to `protoc` and `reproto`) is
re-run unnecessarily.  The codegen output is purely a function of
`reprotoFilteredSrc` and `pkgs.protobuf`, not of Python library versions.

### 4.8 `pyo3DepsCache` uses `buildPhaseCargoCommand` override

```nix
buildPhaseCargoCommand = "cargoWithProfile build -p prototext_codec";
```

This overrides Crane's default dep-only check pass with a full build.
The comment says "skip the default cargo check pass: the build pass
produces .rlib files that are a strict superset."  While correct, this
means `pyo3DepsCache` is heavier than necessary: it builds the full
`cdylib` linkage, not just the dep `.rlib`s.

---

## 5. Proposals

### P1 — Extract a shared `testAndLintDeps` Python list

Replace the three duplicated Python dep lists (`reprotoTests`,
`pythonLint`, `shellHook`) with a single named list:

```nix
reprotoTestDeps = reprotoPropagatedDeps ++ [
    prototextCodec
    pythonPkgs.pytest
    pythonPkgs."pytest-xdist"
    pythonPkgs.tree-sitter
    pythonPkgs.tree-sitter-language-pack
];
```

Use it in `reprotoTests`, `pythonLint`, and as the basis for
`shellHook`'s `PYTHONPATH`.  This eliminates §4.4 and §4.5 at once.

### P2 — Decouple `prototools` from `rustTests` artifact chain

Change `prototools` to chain off `depsCache` (or `rustClippy`):

```nix
prototools = crane.buildPackage (commonArgs // {
    cargoArtifacts = depsCache;   # was: rustTests
    ...
});
```

Keep `ci` enforcing `rustTests` before `prototools` is considered
complete by including both in `linkFarmFromDrvs`.  This allows
`prototools` and `rustTests` to build in parallel (Nix evaluates
parallel derivations automatically), reducing wall-clock CI time.

### P3 — Separate RUSTFLAGS from pyo3 dependency compilation

The `-lpython` RUSTFLAGS flag is a **linker** flag irrelevant during
`rustc` compilation of pure-Rust crates.  Consider passing it only at
the final link step (via `cargoExtraArgs` or a custom `buildPhase`) so
that `prototext-core` artifacts from `depsCache` can be reused by the
pyo3 track.  This is non-trivial because Crane / Cargo fingerprints
include env vars, but it would eliminate the duplicate compilation of
`prototext-core` (§4.1).

A lighter alternative: accept the duplicate compilation but document it
explicitly, and ensure `pyo3DepsCache` uses `buildDepsOnly` with the
default check pass (not the overridden full build of §4.8).

### P4 — Guard `cargo build` in `shellHook` behind a hash check

Instead of always running `cargo build`, check whether the binary is
already up to date:

```bash
if ! command -v prototext &>/dev/null || \
   [[ "$(cargo metadata --no-deps -q 2>/dev/null \
         | jq -r '.packages[]|select(.name=="prototext")|.version')" \
      != "$(prototext --version 2>/dev/null | awk '{print $2}')" ]]; then
    cargo build --release --locked -p prototext
fi
```

Or simply add a `|| true` + informational echo so the shell starts
immediately and warns the user to run `cargo build` manually.

### P5 — Derive `dev-shell.nativeBuildInputs` from build derivations

Instead of duplicating tool names, compose the shell inputs from the
same attribute sets used by CI derivations:

```nix
dev-shell = pkgs.mkShell {
    inputsFrom = [ depsCache pyo3DepsCache ];
    nativeBuildInputs = [
        pkgs.rustfmt pkgs.clippy pkgs.reuse pkgs.gh
        pkgs.mandoc pkgs.zola pkgs.buf
        pythonPkgs.pytest pythonPkgs."pytest-xdist"
        pythonPkgs.ruff
    ];
    ...
};
```

`inputsFrom` pulls in `cargo`/`rustc`/`protobuf` from the cited
derivations, avoiding silent version drift.

### P6 — Split `reprotoSrcWithCodegen` dependency on Python version

Move the codegen stage's `buildInputs` to only `reprotoBare` (for the
`bash` invocation) and `pkgs.protobuf`.  The key change is to not chain
`reprotoSrcWithCodegen` off the full propagated Python deps, so that a
Python dep upgrade doesn't invalidate the codegen Nix hash.

```nix
reprotoSrcWithCodegen = pkgs.runCommand "reproto-src-with-codegen" {
    buildInputs = [ reprotoBare pkgs.protobuf ];
    # reprotoBare is small — only needs click/lark/protobuf at runtime
    # to run patch_reproto.sh; no other propagatedBuildInputs needed here.
} ''...''
```

---

## 6. Summary Table

| # | Issue | Severity | Proposal |
|---|---|---|---|
| 4.1 | `prototext-core` compiled twice | Medium | P3 |
| 4.2 | `prototools` chains off `rustTests` needlessly | Low | P2 |
| 4.3 | Shell tool list duplicated from derivations | Low | P5 |
| 4.4 | `PYTHONPATH` in shell diverges from `reprotoTests` | Medium | P1 |
| 4.5 | `pythonLint` missing `tree-sitter*` | Low-Medium | P1 |
| 4.6 | Blocking `cargo build` on every shell entry | Low | P4 |
| 4.7 | `reprotoSrcWithCodegen` over-sensitive to Python deps | Low | P6 |
| 4.8 | `pyo3DepsCache` does full build instead of dep-only | Low | P3 note |
