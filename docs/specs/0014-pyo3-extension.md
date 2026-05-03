<!--
SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0014 — PyO3 Python extension (`prototext-pyo3`)

**Status:** draft
**App:** prototext

---

## Problem

`prototext-core` exists as a pure-Rust library but has no Python binding.
The internal repo at `../prototools` depends on a PyO3 extension crate
(`ext/prototext/pyo3/`) that wraps the same codec and is used by Python
consumers of the prototext pipeline.  That crate currently depends on the
internal `prototext-core`, which is not the OSS version.

The OSS repo must provide an equivalent Python extension so that:

1. Python consumers can be migrated from the internal extension to the OSS
   one without any API changes.
2. The internal repo can eventually depend on the OSS extension as an
   upstream package rather than maintaining a parallel copy.

---

## Goals

1. Add a new `prototext-pyo3/` crate to the OSS workspace that compiles to
   a Python-importable shared library named `prototext_codec_lib`.
2. Expose exactly the same Python API as the internal
   `ext/prototext/pyo3/src/lib.rs` — same function names, same signatures,
   same default values, same docstrings.
3. Expose exactly the same Rust-level `pub` items (`SCHEMA_CAPSULE_NAME`,
   `SchemaHandle::from_capsule`, `SchemaHandle::from_bytes`,
   `SchemaHandle::render_as_text`) so that other extension crates in the
   same workspace can link against the `rlib` target and share schema handles
   across `.so` boundaries via capsules.
4. Provide a `prototext_post_build` binary that generates a `.pyi` stub file
   via `pyo3-stub-gen`, identical to the internal pattern.
5. Integrate the crate into the Cargo workspace and into `default.nix` so
   that `nix-build -A prototext-codec` produces a `buildPythonPackage`
   derivation that downstream consumers can use as a `propagatedBuildInput`.

---

## Non-goals

- Introducing any new Python API not already present in the internal crate.
- Changing the public Rust API of `prototext-core`.
- Python tests (the Python API is tested end-to-end in the internal repo;
  the OSS repo validates via `cargo test` and the Nix build).

---

## Specification

### 1. Crate layout

```
prototext-pyo3/
├── Cargo.toml
├── pyproject.toml
├── src/
│   ├── lib.rs
│   └── bin/
│       └── post_build.rs
└── prototext_codec_lib/
    └── __init__.py
```

### 2. `Cargo.toml`

```toml
[package]
name    = "prototext_codec"
version = "0.1.0"
edition = "2021"

[lib]
name       = "prototext_codec_lib"
crate-type = ["cdylib", "rlib"]

[dependencies]
prototext-core       = { path = "../prototext-core" }
pyo3                 = { version = "0.26", features = ["extension-module"] }
pyo3_stub_gen        = { version = "0.16", package = "pyo3-stub-gen" }
pyo3_stub_gen_derive = { version = "0.16", package = "pyo3-stub-gen-derive" }

[[bin]]
name = "prototext_post_build"
path = "src/bin/post_build.rs"
```

The `rlib` target allows other extension crates to link against this crate
and call `SchemaHandle::from_capsule` / `SchemaHandle::from_bytes` at the
Rust level.

### 3. `src/lib.rs`

The file is a direct port of `ext/prototext/pyo3/src/lib.rs` from the
internal repo.  The only changes are:

- Copyright header: only the Thales SPDX line is used (the `fred@` personal
  line must not appear in OSS-published files).
- The `use` block accesses `prototext_core` sub-modules, all of which are
  `pub` in the OSS `prototext-core`:
  - `prototext_core::decoder::ingest_pb`
  - `prototext_core::schema::{parse_schema, ParsedSchema}`
  - `prototext_core::serialize::encode_text::encode_text_to_binary`
  - `prototext_core::serialize::render_text::{decode_and_render, is_prototext_text}`
- There is no `serialize::binary` module in the OSS `prototext-core` (the
  internal `encode_to_binary` function does not exist here — see §3.1).

#### 3.1 `format_as_bytes(assume_binary=True)` — OSS behaviour

The OSS `prototext-core` public API is:

| Function | Signature | Behaviour |
|---|---|---|
| `render_as_text` | `(data, schema, opts) -> Result<Vec<u8>, CodecError>` | binary → annotated text |
| `render_as_bytes` | `(data, opts) -> Result<Vec<u8>, CodecError>` | text → binary (or passthrough if already binary) |
| `parse_schema` | `(bytes, name) -> Result<ParsedSchema, SchemaError>` | parses descriptor |

`render_as_bytes` with `assume_binary=True` is a **passthrough**: it returns
the input unchanged.  This is the correct behaviour — prototext is
bit-level faithful and never normalises the wire encoding.

The internal `format_as_bytes(assume_binary=True)` performs an
`ingest_pb` + `encode_to_binary` round-trip.  That round-trip is wrong
by design for a bit-faithful codec: there is nothing to normalise.
The OSS passthrough is the right implementation.

The `format_as_bytes` Python function in this crate therefore calls
`render_as_bytes` directly, which handles both the text→binary and
the already-binary (passthrough) cases.

#### 3.2 Exported Python API (unchanged from internal)

```python
class SchemaHandle:
    """Opaque handle to a parsed protobuf schema."""
    def schema_capsule(self) -> capsule: ...

def register_schema(schema_data: bytes, root_message: str) -> SchemaHandle: ...

def format_as_text(
    data: bytes,
    schema: SchemaHandle | None = None,
    assume_binary: bool = False,
    include_annotations: bool = False,
    indent: int = 1,
) -> bytes: ...

def format_as_bytes(data: bytes, assume_binary: bool = False) -> bytes: ...
```

#### 3.3 Exported Rust-level items (unchanged from internal)

```rust
pub const SCHEMA_CAPSULE_NAME: &CStr;
impl SchemaHandle {
    pub fn from_capsule(capsule: &Bound<'_, PyCapsule>) -> PyResult<SchemaHandle>;
    pub fn from_bytes(schema_bytes: &[u8], root_message: &str) -> Result<Self, SchemaError>;
    pub fn render_as_text(&self, data: &[u8], assume_binary: bool,
                          include_annotations: bool, indent: usize) -> Vec<u8>;
}
pub fn stub_info() -> pyo3_stub_gen::Result<pyo3_stub_gen::StubInfo>;
```

### 4. `src/bin/post_build.rs`

Identical to the internal `ext/prototext/pyo3/src/bin/post_build.rs`:

```rust
use prototext_codec_lib::stub_info;

fn main() -> pyo3_stub_gen::Result<()> {
    stub_info()?.generate()?;
    Ok(())
}
```

### 5. `pyproject.toml`

```toml
[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[project]
name = "prototext_codec"
version = "0.1.0"
description = "Lossless protobuf decoder (Python extension via PyO3)"
requires-python = ">=3.8"

[tool.hatch.build.targets.wheel]
packages = ["prototext_codec_lib"]

[tool.pyo3-stub-gen]
# pyo3-stub-gen writes prototext_codec_lib.pyi next to this file.
```

### 6. `prototext_codec_lib/__init__.py`

```python
from .prototext_codec_lib import SchemaHandle, format_as_bytes, format_as_text, register_schema
from . import prototext_codec_lib

__doc__ = prototext_codec_lib.__doc__
__all__ = ["SchemaHandle", "format_as_bytes", "format_as_text", "register_schema"]
```

### 7. Workspace change

Add `"prototext-pyo3"` to the `members` list in the root `Cargo.toml`.

### 8. `default.nix` changes

#### 8.1 New parameter

```nix
{ pkgs ? ...,
  pythonPkgs ? pkgs.python312Packages,
}:
```

#### 8.2 New variables

```nix
pythonBin        = pythonPkgs.python.withPackages (_: []);
pythonExecutable = "${pythonBin}/bin/python";
```

`pythonBin` provides the C headers and `libpython` needed by pyo3 at link
time.  No Python packages need to be pre-installed for the extension itself.

#### 8.3 `pyo3CommonArgs`

Extends `commonArgs` with the three values required for a PyO3 Crane build:

```nix
pyo3CommonArgs = commonArgs // {
  env.PYO3_PYTHON = pythonExecutable;
  RUSTFLAGS       = "-L ${pythonBin}/lib -lpython${pythonPkgs.python.pythonVersion}";
  nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pythonBin ];
};
```

`RUSTFLAGS` is set uniformly across both the dep-cache and the build
derivation so that the `prototext_post_build` binary (which links against
`libpython`) reuses the same compiled `.rlib` artifacts as the `cdylib`
target.  Cargo fingerprints are exact string comparisons on `RUSTFLAGS`; a
mismatch would force a full recompile.

#### 8.4 `pyo3DepsCache`

```nix
pyo3DepsCache = crane.buildDepsOnly (pyo3CommonArgs // {
  pname          = "prototext-codec-deps";
  cargoExtraArgs = "-p prototext_codec";
  doCheck        = false;
  buildPhaseCargoCommand = "cargoWithProfile build -p prototext_codec";
});
```

`buildPhaseCargoCommand` skips Crane's default `cargo check` pass (which
would recompile `.rmeta` files that are redundant once `.rlib` files are
produced by the build pass).

#### 8.5 `prototextExtension`

```nix
prototextExtension = crane.buildPackage (pyo3CommonArgs // {
  pname          = "prototext-codec-ext";
  cargoArtifacts = pyo3DepsCache;
  cargoExtraArgs = "-p prototext_codec --lib";
  doCheck        = false;
  preBuild = ''
    rm -rf target/release/build/prototext_codec-*
    rm -rf target/release/.fingerprint/prototext_codec-*
  '';
  postBuild = ''
    echo "Generating prototext_codec_lib stubs..."
    cargo run --release -p prototext_codec --bin prototext_post_build
  '';
  installPhase = ''
    mkdir -p $out/artifacts/
    cp target/release/libprototext_codec_lib.so $out/artifacts/prototext_codec_lib.so
    cp prototext-pyo3/prototext_codec_lib.pyi    $out/artifacts/prototext_codec_lib.pyi
  '';
});
```

The `preBuild` step clears stale Cargo fingerprints for `prototext_codec`
so that the pyo3 build script re-runs in this derivation (same pattern used
in the internal repo).

#### 8.6 `prototextCodec` (the Python package)

```nix
prototextCodec = pythonPkgs.buildPythonPackage {
  pname   = "prototext_codec";
  version = "0.1.0";
  format  = "pyproject";
  src     = ./prototext-pyo3;
  buildInputs = [ pythonPkgs.hatchling prototextExtension ];
  patchPhase = ''
    cp ${prototextExtension}/artifacts/prototext_codec_lib.pyi \
       prototext_codec_lib/prototext_codec_lib.pyi
    cp ${prototextExtension}/artifacts/prototext_codec_lib.so  \
       prototext_codec_lib/prototext_codec_lib.so
  '';
};
```

#### 8.7 New output attribute

```nix
prototext-codec = prototextCodec;
```

After `nix-build -A prototext-codec`, a downstream consumer can do:

```nix
prototoolsOss = import (fetchgit { url = "..."; rev = "..."; sha256 = "..."; })
  { inherit pkgs pythonPkgs; };

# use as a propagatedBuildInput:
propagatedBuildInputs = [ prototoolsOss.prototext-codec ];
```

and in Python:

```python
import prototext_codec_lib
handle = prototext_codec_lib.register_schema(fdp_bytes, "pkg.MyMessage")
text   = prototext_codec_lib.format_as_text(pb_bytes, schema=handle,
             assume_binary=True, include_annotations=True, indent=1)
```

---

## Implementation steps

Each step below is independently buildable and testable.

### Step 1 — Cargo workspace scaffold

Create `prototext-pyo3/Cargo.toml`, `prototext-pyo3/pyproject.toml`,
and `prototext-pyo3/prototext_codec_lib/__init__.py` exactly as specified
in §§2, 5, 6.  Add `"prototext-pyo3"` to the workspace `members` list (§7).

Write a minimal `prototext-pyo3/src/lib.rs` that compiles but exposes
nothing yet:

```rust
use pyo3::prelude::*;

#[pymodule]
fn prototext_codec_lib(_py: Python<'_>, _m: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}
```

Write `prototext-pyo3/src/bin/post_build.rs` as in §4.

**Test:** `cargo build -p prototext_codec` compiles without errors.

### Step 2 — `SchemaHandle` pyclass and Rust-level pub items

Implement the `SchemaHandle` pyclass with `schema_capsule()` and the
three Rust-level pub items (`SCHEMA_CAPSULE_NAME`, `from_capsule`,
`from_bytes`, `render_as_text`) as specified in §§3.3.

Register `SchemaHandle` in the `#[pymodule]` init function.

**Test:** `cargo build -p prototext_codec` compiles.  Optionally,
`cargo test -p prototext_codec` if unit tests are added for
`from_bytes` / `render_as_text`.

### Step 3 — Python functions (`register_schema`, `format_as_text`, `format_as_bytes`)

Implement the three `#[pyfunction]` items as specified in §3.2, calling
the OSS `prototext-core` public API.

Pay attention to §3.1: `format_as_bytes` delegates to `render_as_bytes`
directly — no `ingest_pb` + re-encode.

Register all three functions in the `#[pymodule]` init function.

**Test:** `cargo build -p prototext_codec` compiles.

### Step 4 — pyo3-stub-gen wiring

Add `pyo3_stub_gen` / `pyo3_stub_gen_derive` annotations to all
`#[pyclass]` and `#[pyfunction]` items.  Implement `stub_info()` as
required by `post_build.rs`.

**Test:** `cargo run -p prototext_codec --bin prototext_post_build`
produces `prototext-pyo3/prototext_codec_lib.pyi` with correct
signatures.

### Step 5 — REUSE compliance

Run `reuse annotate` on all new files with the Thales SPDX line only
(no personal `fred@` line).  Run `reuse lint` and confirm it passes.

**Test:** `reuse lint` exits 0.

### Step 6 — `default.nix` integration

Add `pythonPkgs` parameter, `pythonBin`, `pythonExecutable`,
`pyo3CommonArgs`, `pyo3DepsCache`, `prototextExtension`,
`prototextCodec`, and the `prototext-codec` output attribute
as specified in §8.

**Test:** `nix-build -A prototext-codec` succeeds and the result
contains a Python package importable as `prototext_codec_lib`.

---

## References

- `prototext-core/src/lib.rs` — public Rust API being wrapped
- `../prototools/ext/prototext/pyo3/src/lib.rs` — internal crate being ported
- `../prototools/default.nix` — reference for `mkExt` / `mkPyPkg` pattern
- `docs/specs/0011-prost-reflect-schema.md` — schema layer used by this crate
