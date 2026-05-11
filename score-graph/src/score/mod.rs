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

fn load_pb(path: &PathBuf) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let raw = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if is_prototext_text(&raw) {
        Ok(encode_text_to_binary(&raw))
    } else {
        Ok(raw)
    }
}

// ── score subcommand ──────────────────────────────────────────────────────────

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

    // ── 3. Load protobuf bytes ────────────────────────────────────────────────
    let pb_bytes = load_pb(&args.proto)?;

    // ── 4. Score ──────────────────────────────────────────────────────────────
    let score = walk::score(&pb_bytes, root_state.to_native(), &graph);

    // ── 5. Print result ───────────────────────────────────────────────────────
    if score.vetoed {
        println!("Vetoed");
    } else {
        println!(
            "matches={} unknowns={} mismatches={} non_canonical={} score={}",
            score.matches,
            score.unknowns,
            score.mismatches,
            score.non_canonical,
            score.score(),
        );
    }

    Ok(())
}

/// Args for the `score` subcommand.
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

// ── match subcommand ──────────────────────────────────────────────────────────

pub fn run_match(args: MatchArgs) -> Result<(), Box<dyn std::error::Error>> {
    // ── 1. Load compiled graph ────────────────────────────────────────────────
    let graph = load::load_graph(&args.graph)?;

    // ── 2. Load protobuf bytes ────────────────────────────────────────────────
    let pb_bytes = load_pb(&args.proto)?;

    // ── 3. Score all entries simultaneously ──────────────────────────────────
    let mut results = walk::score_all(&pb_bytes, &graph);

    // ── 4. Sort and print ─────────────────────────────────────────────────────
    // Non-vetoed first, sorted by score descending, ties broken by fqdn.
    results.sort_by(|a, b| match (a.vetoed, b.vetoed) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        (true, true) => a.fqdn.cmp(&b.fqdn),
        (false, false) => b.score().cmp(&a.score()).then(a.fqdn.cmp(&b.fqdn)),
    });

    let non_vetoed: Vec<_> = results.iter().filter(|r| !r.vetoed).collect();
    let to_print = match args.top {
        Some(n) => &non_vetoed[..n.min(non_vetoed.len())],
        None => &non_vetoed[..],
    };

    for r in to_print {
        println!(
            "entry={} matches={} unknowns={} mismatches={} non_canonical={} score={}",
            r.fqdn,
            r.matches,
            r.unknowns,
            r.mismatches,
            r.non_canonical,
            r.score(),
        );
    }

    if args.all {
        let vetoed: Vec<_> = results.iter().filter(|r| r.vetoed).collect();
        for r in vetoed {
            println!("entry={} Vetoed", r.fqdn);
        }
    }

    Ok(())
}

/// Args for the `match` subcommand.
#[derive(Debug, clap::Args)]
pub struct MatchArgs {
    /// Compiled scoring graph (.bin) produced by build-scoring-graph.
    #[arg(value_name = "GRAPH")]
    pub graph: PathBuf,

    /// Binary protobuf file to score against all entries.
    #[arg(value_name = "PROTO")]
    pub proto: PathBuf,

    /// Print only the top N entries by score.
    #[arg(long, value_name = "N")]
    pub top: Option<usize>,

    /// Include vetoed entries in output (printed after non-vetoed, marked "Vetoed").
    #[arg(long)]
    pub all: bool,
}
