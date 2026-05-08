<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0040 — Import protoscan into oss-prototools

**Status:** draft
**App:** protoscan

---

## Purpose

`protoscan` is a tool that scans binary files (e.g. shared libraries,
executables) for embedded `FileDescriptorProto` blobs and extracts them as
individual `.pb` files, one per proto file descriptor found.  It currently
lives in the private `../prototools` repo as:

- `src/protoscan/` — Python CLI (`cli.py`, `__init__.py`)
- `ext/fdp_scan/` — Rust/PyO3 extension (`fdp_scan_lib`) providing the
  low-level binary scan

The tool contains no proprietary logic and can be open-sourced as-is.

---

## Goals

1. Copy `src/protoscan/` into `protoscan/src/protoscan/` in this repo,
   following the existing `reproto/src/reproto/` layout convention.
2. Copy `ext/fdp_scan/` into `fdp-scan-pyo3/` at repo root, following the
   existing `prototext-pyo3/` layout convention (no `ext/` subdirectory).
3. Carry over the Rust unit tests that live inline in `ext/fdp_scan/src/lib.rs`.
4. Wire `protoscan` into `default.nix`:
   - Build the `fdp_scan_lib` PyO3 extension (`fdpscanExtension`) via Crane,
     reusing the existing `depsCache`.
   - Export `fdp_scan_lib` as a pure-Rust library derivation (`fdpScanLib`)
     analogous to what `../prototools/default.nix` exports as `fdp_scan`.
   - Build a `protoscan` Python package derivation analogous to `reproto`.
5. Add `protoscan` to the `user-shell` and `prototools` bundle (alongside
   `prototext` and `reproto`).
6. Add a `bin/protoscan` wrapper script (same pattern as `bin/reproto`).
7. Wire `protoscan` into `dev-shell`: install the `.so`/`.pyi` artifacts into
   `fdp-scan-pyo3/fdp_scan_lib/` on shell entry (same pattern as
   `prototext-pyo3/` in the dev-shell), add bash completion, and update
   PYTHONPATH.
8. Add REUSE headers to all imported files (copyright: THALES CLOUD SECURISE
   SAS, license: MIT) where not already present.
9. Add a man page stub `man/protoscan.1` (same pattern as `man/reproto.1`).
10. All existing CI checks continue to pass.

---

## Non-goals

- Changing protoscan's behaviour or CLI interface.
- Removing protoscan from `../prototools` (that repo may keep its own copy
  during a transition period).

---

## Specification

### Directory layout after import

```
fdp-scan-pyo3/          ← mirrors prototext-pyo3/ layout
  Cargo.toml
  pyproject.toml
  src/
    lib.rs              ← includes inline #[cfg(test)] unit tests
    bin/
      post_build.rs
  fdp_scan_lib/
    __init__.py
    fdp_scan_lib.pyi
    .gitignore
  .gitignore

protoscan/              ← mirrors reproto/ layout
  pyproject.toml
  src/
    protoscan/
      __init__.py
      cli.py
```

### Cargo workspace

Add `fdp-scan-pyo3` to the `members` list in the root `Cargo.toml`,
alongside the existing `prototext-core`, `prototext-pyo3`, etc.

### Nix build

In `default.nix`:

- Build `fdp_scan_lib` as a Rust extension using the same Crane + PyO3
  pattern as `prototextExtension`, sourced from `fdp-scan-pyo3/`.  Name the
  derivation `fdpscanExtension`.
- Export a pure-Rust Python package wrapping the `.so`/`.pyi` artifacts
  (analogous to the `fdp_scan` derivation in `../prototools/default.nix`).
  Name it `fdpScanLib`.  This is what consumers import as `fdp_scan_lib`.
- Build a `protoscan` Python package (`buildPythonPackage`, `pyproject` format)
  with `fdpScanLib` as a propagated dependency.  Name the derivation
  `protoscan`.  Entry point: `protoscan.cli:main` (verify before wiring).
- Add `protoscan` to `prototools` (the `symlinkJoin` bundle).
- Add `protoscan` to `user-shell` packages and its bash completion to the
  `shellHook`.
- Export `fdpScanLib` from the attribute set so external consumers can depend
  on it directly (analogous to how `../prototools/default.nix` exports
  `fdp_scan`).

### bin/ wrapper

Add `bin/protoscan` (same pattern as `bin/reproto`):

```python
#!/usr/bin/env python
from protoscan.cli import main
if __name__ == "__main__":
    main()
```

Also add the corresponding `bin/protoscan.license` REUSE file.

### dev-shell wiring

In the `dev-shell` `shellHook`:

- Install `.so` and `.pyi` from `fdpscanExtension` into
  `fdp-scan-pyo3/fdp_scan_lib/` on first entry (no-clobber, same pattern as
  `prototext-pyo3/`).
- Add `$PWD/fdp-scan-pyo3` to `PYTHONPATH` so the locally-checked-out wrapper
  package shadows the Nix-store copy (same pattern as `ext/fdp_scan` in
  `../prototools/default.nix`).
- Add bash completion for `protoscan` (same pattern as `prototext`/`reproto`).
- Add `fdp-scan-pyo3/fdp_scan_lib` to the `exclude` list in
  `pyrightconfig.json` (same as `prototext-pyo3/prototext_codec_lib`).

### Man page

Add `man/protoscan.1` with a minimal man page (NAME, SYNOPSIS, DESCRIPTION,
OPTIONS).

### REUSE

Run `reuse annotate --merge-copyrights --copyright "THALES CLOUD SECURISE SAS"
--license MIT` on all newly added files that lack a header.

---

## Implementation notes

- The `fdp_scan` Cargo package currently declares both a `cdylib` (for PyO3)
  and an `rlib` (for the post-build stub-gen binary).  Both must be preserved.
- The `pyo3-stub-gen` post-build step (generating `fdp_scan_lib.pyi`) should
  be run as a `postBuild` hook in the Nix derivation, matching the pattern
  used for `prototextExtension`.
- `protoscan` CLI entry point is `protoscan.cli:main` (verify in `cli.py`
  before wiring).
- The Rust unit tests in `lib.rs` are carried over as-is; add a
  `rustTestsFdpScan` derivation (`crane.cargoTest`) analogous to `rustTests`
  in the existing build, and reference it in the `ci` closure.
- `../prototools/default.nix` exports `fdp_scan` as a pure-Python wrapper
  package (produced by `mkPyPkg`).  Replicate the same pattern here as
  `fdpScanLib` and export it from the attribute set.
