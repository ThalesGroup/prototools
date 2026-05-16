// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Debug binary: compile a fixture's scoring-graph YAMLs and dump the
//! compiled graph as human-readable YAML to stdout.
//!
//! Usage:
//!   cargo run -p scoring-graph --bin hopcroft_dump -- <fixture-dir>
//!
//! <fixture-dir> must contain an `input/` subdirectory with one or more
//! scoring-graph YAML files (spec 0045 §2 format).  All `*.yaml` files
//! found recursively under `input/` are merged and compiled.

use std::path::PathBuf;

use walkdir::WalkDir;

use score_graph_lib::build_scoring_graph::build_from_strings;

fn main() {
    let fixture_dir: PathBuf = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            eprintln!("usage: hopcroft_dump <fixture-dir>");
            std::process::exit(1);
        });

    let input_dir = fixture_dir.join("input");
    if !input_dir.is_dir() {
        eprintln!("error: {} is not a directory", input_dir.display());
        std::process::exit(1);
    }

    // Collect all *.yaml files recursively under input/.
    let mut yamls: Vec<String> = Vec::new();
    for entry in WalkDir::new(&input_dir).sort_by_file_name() {
        let entry = entry.unwrap_or_else(|e| {
            eprintln!("error reading directory entry: {e}");
            std::process::exit(1);
        });
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
            let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
                eprintln!("error reading {}: {e}", path.display());
                std::process::exit(1);
            });
            yamls.push(content);
        }
    }

    if yamls.is_empty() {
        eprintln!("error: no *.yaml files found under {}", input_dir.display());
        std::process::exit(1);
    }

    let (_rkyv_bytes, yaml) = build_from_strings(&yamls, true).unwrap_or_else(|e| {
        eprintln!("error building graph: {e}");
        std::process::exit(1);
    });

    print!("{}", yaml.unwrap());
}
