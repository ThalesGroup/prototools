// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Compile the vendored upb and the shim (spec 0415 S1, S2).

fn main() {
    println!("cargo:rerun-if-changed=src/shim.c");
    println!("cargo:rerun-if-changed=vendor");

    // A debug build compiles C at -O0, where glibc cannot apply the
    // `_FORTIFY_SOURCE` hardening Nix's compiler enables (and says so). The
    // C goes to -Og there instead: GCC's level for debugging, which keeps
    // variables and line stepping usable and still counts as optimized, so
    // the hardening applies. A profile that optimizes keeps its own level
    // (`opt_level` replaces it rather than adding to it): release's -O3,
    // or a size level someone chose.
    let unoptimized = std::env::var("OPT_LEVEL").is_ok_and(|l| l == "0");

    let base = || {
        let mut b = cc::Build::new();
        b.std("c99")
            // Keep upb's symbols out of any shared object this ends up in,
            // so a process holding another upb (Python's protobuf) cannot
            // bind to ours or we to its (spec 0415 S10).
            .flag_if_supported("-fvisibility=hidden");
        if unoptimized {
            b.opt_level_str("g");
        }
        b
    };

    base()
        .include("vendor/upb")
        .include("vendor/utf8_range")
        // upb's own code is not ours to keep warning-free.
        .warnings(false)
        // Makes upb's `UPB_API_INLINE` functions real symbols we can link,
        // as Google's own Rust binding does. Only upb.c may be compiled so:
        // the header then *defines* those functions, and a second
        // translation unit including it would define them twice.
        .define("UPB_BUILD_API", Some("1"))
        .file("vendor/upb/upb.c")
        .file("vendor/utf8_range/utf8_range.c")
        .compile("prototext_upb");

    // The shim sees the same functions as `static inline`, and re-exports
    // the generated ones upb.c does not. upb's header comes in as a system
    // header, so its warnings stay quiet and the shim's own are reported.
    base()
        .flag("-isystem")
        .flag("vendor/upb")
        .flag("-isystem")
        .flag("vendor/utf8_range")
        .file("src/shim.c")
        .compile("prototext_upb_shim");
}
