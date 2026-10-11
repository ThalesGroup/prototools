// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Build script: compile the textproto grammar's committed parser.
//!
//! `reproto/tree-sitter-textproto/src/parser.c` is generated from
//! `grammar.js` and committed (spec 0408), so building it takes a C compiler
//! and nothing else: no tree-sitter CLI, no Nix derivation, no environment
//! variable. `default.nix`'s `tree-sitter-textproto-check` keeps the
//! committed parser in step with the grammar. `colorize.rs` embeds the
//! highlights query from the same directory.

use std::path::PathBuf;

fn main() {
    let grammar =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../reproto/tree-sitter-textproto");
    let src = grammar.join("src");

    cc::Build::new()
        .include(&src)
        .file(src.join("parser.c"))
        // Generated code: its warnings are not ours to fix.
        .warnings(false)
        .compile("tree-sitter-textproto");

    println!("cargo:rerun-if-changed={}", src.display());
    println!(
        "cargo:rerun-if-changed={}",
        grammar.join("highlights.scm").display()
    );
}
