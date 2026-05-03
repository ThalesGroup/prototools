<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0038 — default.nix: unify and deduplicate the build definition

**Status:** implemented
**Implemented in:** 2026-05-03
**App:** (build system — not app-specific)

---

## Purpose

`default.nix` has grown organically and now contains several classes of
duplication that make it fragile and hard to maintain:

- Cargo flag strings hardcoded at multiple call sites
- Two parallel dep-cache chains (`depsCache` / `pyo3DepsCache`) that are
  unnecessary given how pyo3 and Crane actually work
- A `pyo3CommonArgs` attrset that duplicates most of `commonArgs`
- Python dependency lists duplicated between `reprotoTests`, `pythonLint`,
  and the shellHook PYTHONPATH

This spec consolidates those into a single, hierarchic build definition.
The full analysis is in `docs/default-nix-analysis.md`.

---

## Goals

1. Replace the `commonArgs` / `pyo3CommonArgs` split with a single unified
   `commonArgs` that includes `PYO3_PYTHON`, `RUSTFLAGS`, and `pythonBin`.
2. Replace the `depsCache` / `pyo3DepsCache` split with a single `depsCache`
   covering the whole workspace.
3. Eliminate `pyo3CommonArgs`, `pyo3DepsCache`, and the `--exclude
   prototext_codec` exclusion flag from all workspace-wide invocations.
4. Introduce a two-level `commonArgs` → `protocArgs` hierarchy so that
   derivations needing `protoc` inherit it without re-listing inputs.
5. Eliminate the double-compile of `prototext_codec` in `prototextExtension`
   by building the `prototext_post_build` binary in the same cargo invocation
   as the `.so`, and running the already-compiled binary in `postBuild`.
4. Guard the shellHook `cargo build` on binary staleness to avoid ~23s
   recompilation on every `nix-shell` entry when the binary is current.
5. All CI checks (fmt, clippy, clippy-pyo3, tests, reproto tests, pyright,
   ruff) continue to pass.

---

## Non-goals

- Changing any build output or installed artifact.
- Removing `rustClippyPyo3` as a separate derivation (it provides independent
  pyo3-specific clippy enforcement and chains off the shared `depsCache` at no
  extra cost).
- Changing the reproto Python packaging (reprotoBare, reprotoSrcWithCodegen,
  reproto, reprotoTests, pythonLint, pythonRuff) beyond replacing duplicated
  dep lists with `reprotoTestDeps`.
- Addressing nix-shell / nix-build artifact sharing (impossible due to Nix
  sandbox isolation; see analysis §6.7).

---

## Background: why the pyo3 split is unnecessary

Research against authoritative sources (crane FAQ, Nickel project flake.nix)
confirmed:

- `crane.buildDepsOnly` stubs `build.rs` with a dummy source.  The pyo3 build
  script runs fine inside `buildDepsOnly` as long as `PYO3_PYTHON` is present.
  The Nickel project demonstrates this with a single unified `buildDepsOnly`
  covering a full workspace that includes pyo3 crates.
- `-lpython` in `RUSTFLAGS` is a linker flag.  For `rlib` crates it is a no-op
  (the linker is never invoked).  Carrying it globally in `commonArgs` does not
  break non-pyo3 crates.
- The Cargo fingerprint implication is that all derivations in a chain must
  carry the same `RUSTFLAGS`.  A single chain (one `depsCache`) satisfies this
  automatically.
- The only hard requirement is that `PYO3_PYTHON` and `pythonBin` are present
  in `nativeBuildInputs` in every sandbox that compiles `prototext_codec`.
  Setting them globally in `commonArgs` satisfies this.

---

## Specification

### A. Named constants (already partially done in P1–P3)

Keep the bindings introduced in P1–P3:
`workspaceArgs`, `pyo3Args`, `pyo3Rustflags`, `nativeBuildInputsWithProtoc`,
`reprotoTestDeps`.

After this spec, `workspaceArgs` simplifies: the `--exclude prototext_codec`
clause is removed, leaving just `--no-default-features --workspace`.
`pyo3Args` remains for the narrower per-crate invocations
(`rustClippyPyo3`, `prototextExtension`).

### B. Unified commonArgs

Replace:

```nix
commonArgs = {
  inherit src;
  pname             = "prototools";
  version           = "...";
  strictDeps        = true;
  nativeBuildInputs = [ pkgs.cargo pkgs.rustc ];
};

pyo3CommonArgs = commonArgs // {
  env.PYO3_PYTHON   = pythonExecutable;
  RUSTFLAGS         = pyo3Rustflags;
  nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pythonBin ];
};
```

With:

```nix
commonArgs = {
  inherit src;
  pname             = "prototools";
  version           = "...";
  strictDeps        = true;
  nativeBuildInputs = [ pkgs.cargo pkgs.rustc pythonBin ];
  env.PYO3_PYTHON   = pythonExecutable;
  RUSTFLAGS         = pyo3Rustflags;
};
```

Delete `pyo3CommonArgs`.

### C. protocArgs — second-level base for protoc-requiring derivations

Introduce:

```nix
protocArgs = commonArgs // {
  nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.protobuf ];
  patchPhase        = protoPatchPhase;
};
```

`nativeBuildInputsWithProtoc` is then simply `protocArgs.nativeBuildInputs`
and the separate binding can be dropped, or kept as an alias for clarity.

### D. Single depsCache for the whole workspace

Replace:

```nix
depsCache = crane.buildDepsOnly (commonArgs // {
  pname          = "prototools-deps";
  cargoExtraArgs = workspaceArgs;   # --no-default-features --workspace --exclude prototext_codec
});

pyo3DepsCache = crane.buildDepsOnly (pyo3CommonArgs // {
  pname                  = "prototext-codec-deps";
  cargoExtraArgs         = pyo3Args;
  doCheck                = false;
  buildPhaseCargoCommand = "cargoWithProfile build ${pyo3Args}";
});
```

With:

```nix
depsCache = crane.buildDepsOnly (commonArgs // {
  pname          = "prototools-deps";
  cargoExtraArgs = "--no-default-features --workspace";
});
```

Delete `pyo3DepsCache`.

### E. Update all derivations that referenced pyo3CommonArgs / pyo3DepsCache

| Derivation | Old base | New base | cargoArtifacts |
|---|---|---|---|
| `rustClippy` | `commonArgs` | `protocArgs` | `depsCache` |
| `rustClippyPyo3` | `pyo3CommonArgs` | `commonArgs` | `depsCache` |
| `rustTests` | `commonArgs` | `protocArgs` | `depsCache` |
| `prototextExtension` | `pyo3CommonArgs` | `commonArgs` | `depsCache` |

`workspaceArgs` in `rustClippy` and `rustTests` loses the
`--exclude prototext_codec` clause (becomes `--no-default-features
--workspace`).

### F. Eliminate the double-compile in prototextExtension (DEFERRED)

Current `prototextExtension`:
- buildPhase: `cargo build --release -p prototext_codec --lib` → produces `.so`
- postBuild: `cargo run --release -p prototext_codec --bin prototext_post_build`
  → recompiles `prototext_codec` for the binary target (~27s)

The intended fix — passing `--lib --bin prototext_post_build` to build both
targets in one invocation, then running `./target/release/prototext_post_build`
directly — failed in practice.  `prototext_post_build` uses `pyo3-stub-gen`
which introspects the `.so` at runtime via the dynamic linker; running it
outside of `cargo run` (which sets up the correct library search paths and
environment) causes a `NotPresent` panic in `stub_info.rs`.

The root cause is that `pyo3-stub-gen` relies on `cargo run`'s dynamic
linking environment to locate the cdylib.  Eliminating the second compilation
would require either patching `prototext_post_build` to accept an explicit
`.so` path, or using `LD_PRELOAD` / `DYLD_INSERT_LIBRARIES` to inject the
library.  This is deferred to a future spec.

### G. Guard the shellHook cargo build on binary staleness

Replace the unconditional:

```bash
cargo build --release --locked -p prototext
```

With a staleness guard:

```bash
if [[ ! -f "$PWD/target/release/prototext" ]] || \
   [[ "$PWD/prototext/src" -nt "$PWD/target/release/prototext" ]]; then
  cargo build --release --locked -p prototext
fi
```

This saves ~23s on `nix-shell` entry when the binary is already up to date.

---

## Expected impact on build times

| Before | After | Saving |
|---|---|---|
| Two separate dep caches built sequentially | One dep cache | ~106s (pyo3DepsCache check phase) + compression/decompression of second cache |
| `prototextExtension` double-compile | Single compile | ~27s |
| Unconditional shellHook cargo build | Guarded build | ~23s on warm entry |

---

## Files changed

| File | Change |
|---|---|
| `default.nix` | All changes described above |

No other files are affected.

---

## Implementation order

1. Merge `pyo3CommonArgs` into `commonArgs`; verify `nix-build -A rust-fmt`
   still passes (quickest smoke test).
2. Replace `pyo3DepsCache` with the unified `depsCache`; update all
   `cargoArtifacts` references.
3. Introduce `protocArgs`; update `rustClippy`, `rustTests`, `prototools`.
4. Remove `--exclude prototext_codec` from `workspaceArgs`.
5. Fix `prototextExtension` double-compile (item F).
6. Add shellHook staleness guard (item G).
7. Run `nix-build` and verify all 15 derivations pass.
8. Run `nix-shell --run "echo ok"` and verify the guard works correctly on
   both warm (binary present) and cold (binary absent) entries.
