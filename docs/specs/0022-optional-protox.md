<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0022 — Optional `protox` dependency via Cargo feature flag

**Status:** implemented
**App:** prototext
**Implemented in:** 2026-05-01

---

## Problem

`prototext`'s `build.rs` uses the `protox` crate (a pure-Rust protobuf
compiler) to compile three `.proto` schemas into `.pb` descriptor sets at
Cargo build time:

- `google/protobuf/descriptor.proto` → `descriptor.pb` (embedded in the binary)
- `fixtures/schemas/knife.proto` → `knife.pb` (used by integration tests)
- `fixtures/schemas/enum_collision.proto` → `enum_collision.pb` (used by integration tests)

`protox` is a non-trivial transitive dependency.  Some users object to it.
At the same time, it cannot simply be removed: the `.pb` files must come from
somewhere, binary files must not be committed to git, and the crate must
continue to build with `cargo build` alone (no external tools) for crates.io
consumers.

---

## Goals

1. Make `protox` optional via a Cargo feature flag so users who object to it
   can opt out.
2. When `protox` is disabled, the Nix build supplies pre-compiled `.pb` files
   via a `patchPhase` that runs `protoc` before `cargo build`.
3. crates.io consumers continue to build with `cargo build` alone, using
   `protox` (on by default).
4. No binary files committed to git.

---

## Non-goals

- Removing `protox` as the default for crates.io builds.
- Supporting non-Nix builds without `protox` (users who disable the feature
  outside Nix must supply the `.pb` files themselves).
- Changing the compiled output or behavior of `prototext`.

---

## Specification

### 1. Cargo feature flag

In `prototext/Cargo.toml`, make `protox` optional and on by default:

```toml
[features]
default = ["protox"]
protox = ["dep:protox"]

[build-dependencies]
prost  = "0.14"
protox = { version = "0.9", optional = true }
```

### 2. `build.rs` — two code paths

`build.rs` switches on the feature:

```rust
fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set");

    #[cfg(feature = "protox")]
    compile_with_protox(&out_dir, &manifest_dir);

    #[cfg(not(feature = "protox"))]
    copy_prebuilt(&out_dir, &manifest_dir);

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=fixtures/schemas/knife.proto");
    println!("cargo:rerun-if-changed=fixtures/schemas/enum_collision.proto");
}
```

**`compile_with_protox()`** — identical to the current `compile()` calls,
using `protox::compile()`.

**`copy_prebuilt()`** — copies the three `.pb` files from
`fixtures/prebuilt/` (populated by the Nix `patchPhase`) into `$OUT_DIR`:

```rust
#[cfg(not(feature = "protox"))]
fn copy_prebuilt(out_dir: &str, manifest_dir: &str) {
    let prebuilt = std::path::Path::new(manifest_dir).join("fixtures/prebuilt");
    for name in &["descriptor.pb", "knife.pb", "enum_collision.pb"] {
        let src = prebuilt.join(name);
        let dst = std::path::Path::new(out_dir).join(name);
        std::fs::copy(&src, &dst)
            .unwrap_or_else(|e| panic!("failed to copy {name}: {e}"));
    }
}
```

### 3. `fixtures/prebuilt/` — gitignored, Nix-populated

Add `prototext/fixtures/prebuilt/` to `.gitignore` (or a local
`.gitignore` inside `prototext/fixtures/`).  The directory does not exist in
the git tree; it is created by the Nix `patchPhase`.

### 4. Nix `patchPhase` in `default.nix`

In the Crane derivation that builds `prototext` (currently `commonArgs` /
`prototools`), add a `patchPhase` that:

1. Creates `prototext/fixtures/prebuilt/`.
2. Compiles the three schemas with `protoc`.
3. Passes `--no-default-features` to Cargo so `protox` is not compiled.

```nix
prototools = crane.buildPackage (commonArgs // {
  ...
  nativeBuildInputs = (commonArgs.nativeBuildInputs or []) ++ [
    pkgs.protobuf
    pkgs.installShellFiles
  ];

  patchPhase = ''
    mkdir -p prototext/fixtures/prebuilt

    protoc \
      --descriptor_set_out=prototext/fixtures/prebuilt/descriptor.pb \
      google/protobuf/descriptor.proto

    protoc \
      --descriptor_set_out=prototext/fixtures/prebuilt/knife.pb \
      --proto_path=prototext/fixtures/schemas \
      knife.proto

    protoc \
      --descriptor_set_out=prototext/fixtures/prebuilt/enum_collision.pb \
      --proto_path=prototext/fixtures/schemas \
      enum_collision.proto
  '';

  cargoExtraArgs = "--no-default-features -p prototext";
  ...
});
```

The same `patchPhase` and `--no-default-features` must be applied to all
Crane derivations that compile `prototext` code: `rustClippy`, `rustTests`,
and `prototools`.  `depsCache` and `rustFmt` do not need it — `depsCache`
only builds dependencies (not `prototext` itself), and `rustFmt` never
compiles code so `build.rs` never runs.

---


---

## References

- `prototext/build.rs` — current `protox`-based build script.
- `prototext/Cargo.toml` — build dependencies.
- `default.nix` — Crane derivations for `depsCache`, `rustTests`,
  `rustClippy`, `prototools`.
