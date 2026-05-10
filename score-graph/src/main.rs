// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

mod build_scoring_graph;
mod score;

use std::path::PathBuf;

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
}

#[derive(Debug, clap::Args)]
pub struct BuildScoringGraphArgs {
    /// Output path for the CompiledGraph binary.
    #[arg(short = 'o', long = "output", value_name = "PATH", required = true)]
    pub output: PathBuf,

    /// Write an interactive HTML visualisation of the compiled graph to PATH.
    #[arg(long = "graph", value_name = "PATH")]
    pub graph: Option<PathBuf>,

    /// Resolve YAML paths relative to DIR; walk DIR recursively for *.yaml if
    /// no explicit YAML_FILES are given.
    #[arg(short = 'I', long = "input-root", value_name = "DIR")]
    pub input_root: Option<PathBuf>,

    /// Suppress the summary written to stderr.
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    /// YAML files or glob patterns produced by reproto --emit-scoring-graph.
    /// If omitted, --input-root is walked recursively for *.yaml.
    #[arg(value_name = "YAML_FILES")]
    pub yaml_files: Vec<String>,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::BuildScoringGraph(args) => build_scoring_graph::run(args),
        Command::Score(args) => score::run(args),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
