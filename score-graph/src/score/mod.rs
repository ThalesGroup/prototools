// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Score a binary protobuf against a compiled scoring graph.

mod load;
mod walk;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use prototext_core::serialize::encode_text::encode_text_to_binary;
use prototext_core::serialize::render_text::is_prototext_text;

pub fn run(args: ScoreArgs) -> Result<(), Box<dyn std::error::Error>> {
    // ── 1. Load compiled graph ────────────────────────────────────────────────
    let graph = load::load_graph(&args.graph)?;

    // ── 2. Find root state ────────────────────────────────────────────────────
    let root_state = graph
        .roots
        .iter()
        .find(|r| r.fqdn.as_str() == args.entry.as_str())
        .ok_or_else(|| format!("entry '{}' not found in graph", args.entry))?
        .state_id;

    // ── 3. Load protobuf bytes (auto-detect prototext text format) ────────────
    let raw = std::fs::read(&args.proto).map_err(|e| format!("{}: {e}", args.proto.display()))?;
    let pb_bytes: Vec<u8> = if is_prototext_text(&raw) {
        encode_text_to_binary(&raw)
    } else {
        raw
    };

    // ── 4. Score ──────────────────────────────────────────────────────────────
    let score = walk::score(&pb_bytes, root_state.to_native(), &graph);

    // ── 5. Print result ───────────────────────────────────────────────────────
    if score.vetoed {
        println!("Vetoed");
    } else {
        println!(
            "matches={} unknowns={} non_canonical={} score={}",
            score.matches,
            score.unknowns,
            score.non_canonical,
            score.score(),
        );
    }

    Ok(())
}

/// Args for the `score` subcommand (also used by main.rs).
#[derive(Debug, clap::Args)]
pub struct ScoreArgs {
    /// Compiled scoring graph (.bin) produced by build-scoring-graph.
    #[arg(value_name = "GRAPH")]
    pub graph: PathBuf,

    /// Fully-qualified entry-point message type (e.g. google.rpc.Status).
    #[arg(value_name = "ENTRY")]
    pub entry: String,

    /// Binary protobuf file to score.
    #[arg(value_name = "PROTO")]
    pub proto: PathBuf,
}
