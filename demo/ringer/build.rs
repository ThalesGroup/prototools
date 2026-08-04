// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Copies the descriptor set named by `RINGER_DESCRIPTOR_SET` into `OUT_DIR`
//! so that `main.rs` can `include_bytes!` it (spec 0241 S5).

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo::rerun-if-env-changed=RINGER_DESCRIPTOR_SET");

    // No fallback to the well-known types (S7).  A ringer that cannot
    // describe the service it calls is not a ringer, and the failure would
    // otherwise surface as a runtime "type not found" a long way from here.
    let src = match env::var("RINGER_DESCRIPTOR_SET") {
        Ok(v) => PathBuf::from(v),
        Err(_) => {
            eprintln!(
                "\nRINGER_DESCRIPTOR_SET is not set.\n\n\
                 It must name a FileDescriptorSet holding the transitive closure of\n\
                 google/maps/routing/v2/routes_service.proto.  Build one with:\n\n    \
                 RINGER_DESCRIPTOR_SET=$(nix-build -A ringer-desc --no-out-link)/ringer.desc \\\n      \
                 cargo build --release --manifest-path demo/ringer/Cargo.toml\n"
            );
            std::process::exit(1);
        }
    };

    println!("cargo::rerun-if-changed={}", src.display());

    let dst = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("ringer.desc");
    fs::copy(&src, &dst)
        .unwrap_or_else(|e| panic!("copying {} to {}: {e}", src.display(), dst.display()));
}
