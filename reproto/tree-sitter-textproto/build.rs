// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Compiles the committed parser (spec 0408 S1), with a C compiler alone.

fn main() {
    let src = std::path::Path::new("src");
    cc::Build::new()
        .include(src)
        .file(src.join("parser.c"))
        .warnings(false)
        .compile("tree-sitter-textproto");
    println!("cargo:rerun-if-changed=src/parser.c");
}
