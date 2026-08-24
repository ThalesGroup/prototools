// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Copies the descriptor set named by `BOBAPP_DESCRIPTOR_SET` into `OUT_DIR`
//! so that `main.rs` can `include_bytes!` it (spec 0241 S5).

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo::rerun-if-env-changed=BOBAPP_DESCRIPTOR_SET");

    // No fallback to the well-known types (S7).  A bobapp that cannot
    // describe the service it calls is not a bobapp, and the failure would
    // otherwise surface as a runtime "type not found" a long way from here.
    let src = match env::var("BOBAPP_DESCRIPTOR_SET") {
        Ok(v) => PathBuf::from(v),
        Err(_) => {
            eprintln!(
                "\nBOBAPP_DESCRIPTOR_SET is not set.\n\n\
                 It must name a FileDescriptorSet holding the transitive closure of\n\
                 google/maps/places/v1/places_service.proto (spec 0350 S3).\n\n    \
                 BOBAPP_DESCRIPTOR_SET=$(nix-build -A bobapp-desc --no-out-link)/bobapp.desc \\\n      \
                 cargo build --release --manifest-path demo/bobapp/Cargo.toml\n"
            );
            std::process::exit(1);
        }
    };

    println!("cargo::rerun-if-changed={}", src.display());

    let dst = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("bobapp.desc");

    // Read-then-write rather than `fs::copy`: the source is normally a nix
    // store path, which is read-only, and `copy` carries that mode across.  A
    // second build would then find a 0444 destination it may not overwrite,
    // so changing the descriptor set would need a `cargo clean` first.
    let bytes = fs::read(&src).unwrap_or_else(|e| panic!("reading {}: {e}", src.display()));
    fs::write(&dst, &bytes).unwrap_or_else(|e| panic!("writing {}: {e}", dst.display()));
}
