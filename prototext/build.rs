// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Build script: produce `$OUT_DIR/*.pb` descriptor sets.
//!
//! The prebuilt `.pb` files are copied from `fixtures/prebuilt/`, which is
//! populated by the Nix `patchPhase` (dev builds and CI) or bundled directly
//! in the `.crate` tarball via the `[package] include` list (crates.io builds).
//!
//! Nix fast path: `DESCRIPTOR_PB`, `KNIFE_PB`, `ENUM_COLLISION_PB`, and
//! `MESSAGE_SET_PB` env vars point to stable Nix store paths so that
//! OUT_DIR contents are byte-for-byte identical across sandboxes, keeping
//! Cargo fingerprints valid (no spurious external-dep recompiles).
//!
//! Files produced:
//!   - `descriptor.pb`     — FileDescriptorSet for google/protobuf/descriptor.proto
//!   - `knife.pb`          — FileDescriptorSet for fixtures/schemas/knife.proto
//!   - `enum_collision.pb` — FileDescriptorSet for fixtures/schemas/enum_collision.proto
//!   - `message_set.pb`    — FileDescriptorSet for fixtures/schemas/message_set.proto
//!
//! `descriptor.pb` is embedded into the binary at compile time via:
//!   `include_bytes!(concat!(env!("OUT_DIR"), "/descriptor.pb"))`
//!
//! The other `.pb` files are used by integration tests only.

fn copy_prebuilt(out_dir: &str, manifest_dir: &str) {
    // Fast path: Nix build supplies stable store-path env vars so that
    // OUT_DIR contents are byte-for-byte identical across all sandboxes and
    // Cargo fingerprints remain valid (no spurious external-dep recompiles).
    if let Ok(descriptor_pb) = std::env::var("DESCRIPTOR_PB") {
        let knife_pb = std::env::var("KNIFE_PB")
            .unwrap_or_else(|_| panic!("DESCRIPTOR_PB is set but KNIFE_PB is not"));
        let enum_collision_pb = std::env::var("ENUM_COLLISION_PB")
            .unwrap_or_else(|_| panic!("DESCRIPTOR_PB is set but ENUM_COLLISION_PB is not"));
        let message_set_pb = std::env::var("MESSAGE_SET_PB")
            .unwrap_or_else(|_| panic!("DESCRIPTOR_PB is set but MESSAGE_SET_PB is not"));
        for (name, src) in &[
            ("descriptor.pb", descriptor_pb),
            ("knife.pb", knife_pb),
            ("enum_collision.pb", enum_collision_pb),
            ("message_set.pb", message_set_pb),
        ] {
            let dst = std::path::Path::new(out_dir).join(name);
            std::fs::copy(src, &dst)
                .unwrap_or_else(|e| panic!("failed to copy {name} from '{src}': {e}"));
        }
        return;
    }
    // Fallback (crates.io / local dev): copy from fixtures/prebuilt/ which
    // patchPhase or a local protoc run must have populated beforehand.
    let prebuilt = std::path::Path::new(manifest_dir).join("fixtures/prebuilt");
    for name in &[
        "descriptor.pb",
        "knife.pb",
        "enum_collision.pb",
        "message_set.pb",
    ] {
        let src = prebuilt.join(name);
        let dst = std::path::Path::new(out_dir).join(name);
        std::fs::copy(&src, &dst).unwrap_or_else(|e| panic!("failed to copy {name}: {e}"));
    }
}

/// Build the WKT scoring graph (`wkt.rkyv`) from `wkt/SOURCES`.
///
/// Three modes, tried in order:
///
/// - **`prebuilt-wkt` feature** (nixpkgs build): copy pre-generated files
///   from `wkt/prebuilt/` committed to git — no `reproto` invocation needed.
/// - **`WKT_RKYV` env var set** (`default.nix` full build): the files were
///   produced by the `wktRkyv` Nix derivation; copy from the store path.
/// - **Otherwise** (crates.io / local dev): compile `wkt.desc` from SOURCES
///   via `protoc`, then invoke `reproto --schema-db-out`.
#[cfg(feature = "wkt-db")]
fn build_wkt_graph(out_dir: &str, manifest_dir: &str) {
    use std::path::Path;
    #[cfg(not(feature = "prebuilt-wkt"))]
    use std::process::Command;

    let wkt_rkyv_dst = format!("{out_dir}/wkt.rkyv");
    let wkt_index_dst = format!("{out_dir}/wkt_index.rkyv");

    // Fast path: nixpkgs build — copy pre-generated files committed to git.
    #[cfg(feature = "prebuilt-wkt")]
    {
        let prebuilt = Path::new(manifest_dir).join("wkt/prebuilt");
        for (name, dst) in &[
            ("wkt.rkyv", &wkt_rkyv_dst),
            ("wkt_index.rkyv", &wkt_index_dst),
        ] {
            let src = prebuilt.join(name);
            std::fs::copy(&src, dst)
                .unwrap_or_else(|e| panic!("failed to copy {name} from wkt/prebuilt/: {e}"));
        }
    }

    // default.nix full build and crates.io / local dev paths.
    #[cfg(not(feature = "prebuilt-wkt"))]
    {
        // Fast path: default.nix full build pre-supplies both files.
        if let Ok(prebuilt) = std::env::var("WKT_RKYV") {
            std::fs::copy(&prebuilt, &wkt_rkyv_dst)
                .unwrap_or_else(|e| panic!("failed to copy WKT_RKYV '{prebuilt}': {e}"));
            let prebuilt_index = std::env::var("WKT_INDEX")
                .unwrap_or_else(|_| panic!("WKT_RKYV is set but WKT_INDEX is not"));
            std::fs::copy(&prebuilt_index, &wkt_index_dst)
                .unwrap_or_else(|e| panic!("failed to copy WKT_INDEX '{prebuilt_index}': {e}"));
            return;
        }

        // Compile wkt.desc from SOURCES using system protoc.
        let sources_path = Path::new(manifest_dir).join("wkt/SOURCES");
        let sources_text =
            std::fs::read_to_string(&sources_path).expect("failed to read wkt/SOURCES");
        let proto_files: Vec<&str> = sources_text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();

        let wkt_desc_path = format!("{out_dir}/wkt.desc");

        let mut cmd = Command::new("protoc");
        cmd.arg(format!("--descriptor_set_out={wkt_desc_path}"));
        cmd.arg("--include_imports");
        for f in &proto_files {
            cmd.arg(f);
        }
        let status = cmd
            .status()
            .unwrap_or_else(|e| panic!("failed to run protoc: {e}"));
        assert!(status.success(), "protoc failed with status {status}");

        // Run reproto --build-schema-db to produce schemas.desc + schemas/hopcroft.rkyv.
        // -I takes a directory; the positional arg is the .desc filename relative to it.
        // --build-schema-db writes <stem>.desc and <stem>/hopcroft.rkyv.
        let schemas_desc = format!("{out_dir}/schemas.desc");
        let reproto_bin = std::env::var("REPROTO_BIN").unwrap_or_else(|_| "reproto".to_string());
        let status = Command::new(&reproto_bin)
            .arg(format!("--build-schema-db={schemas_desc}"))
            // Required, not optional: protoscan scores descriptors under
            // Policy::Scan against this graph, and that policy asserts the
            // graph carries range data (spec 0238 S9, spec 0239 S2). The
            // other two producers of this artifact — default.nix's wktRkyv
            // derivation and wkt/prebuilt/ — must agree.
            .arg("--emit-extension-ranges")
            .arg(format!("-O{out_dir}/reproto-out"))
            .arg(format!("-I{out_dir}"))
            .arg("wkt.desc")
            .status()
            .unwrap_or_else(|e| panic!("failed to run reproto '{reproto_bin}': {e}"));
        assert!(
            status.success(),
            "reproto --build-schema-db failed with status {status}"
        );
        // Copy schemas/hopcroft.rkyv → wkt.rkyv (the path embedded via include_bytes!).
        std::fs::copy(format!("{out_dir}/schemas/hopcroft.rkyv"), &wkt_rkyv_dst)
            .unwrap_or_else(|e| panic!("failed to copy hopcroft.rkyv: {e}"));
        // Copy schemas/index.rkyv → wkt_index.rkyv (the path embedded via include_bytes!).
        std::fs::copy(format!("{out_dir}/schemas/index.rkyv"), &wkt_index_dst)
            .unwrap_or_else(|e| panic!("failed to copy index.rkyv: {e}"));
    }
}

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");

    copy_prebuilt(&out_dir, &manifest_dir);

    #[cfg(feature = "wkt-db")]
    build_wkt_graph(&out_dir, &manifest_dir);

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=wkt/SOURCES");
    println!("cargo:rerun-if-changed=fixtures/schemas/knife.proto");
    println!("cargo:rerun-if-changed=fixtures/schemas/enum_collision.proto");
    println!("cargo:rerun-if-changed=fixtures/schemas/message_set.proto");
}
