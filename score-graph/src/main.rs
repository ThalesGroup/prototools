// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use score_graph_lib::build_scoring_graph;
use score_graph_lib::build_scoring_graph::BuildScoringGraphArgs;
use score_graph_lib::score;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "score-graph", about = "Scoring graph tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Merge reproto scoring-graph YAMLs into a Hopcroft-deduplicated CompiledGraph.
    BuildScoringGraph(BuildScoringGraphArgs),
    /// Score a binary protobuf against a compiled scoring graph.
    Score(score::ScoreArgs),
    /// Score a binary protobuf against all entries in a compiled scoring graph simultaneously.
    Match(score::MatchArgs),
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::BuildScoringGraph(args) => build_scoring_graph::run(args),
        Command::Score(args) => score::run(args),
        Command::Match(args) => score::run_match(args),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
