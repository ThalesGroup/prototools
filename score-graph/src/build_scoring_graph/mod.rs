// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

pub(crate) mod graph;
pub(crate) mod hopcroft;
pub(crate) mod load;
pub mod serial;
mod show;

use std::path::{Path, PathBuf};

use clap::Args;
use globset::Glob;
use walkdir::WalkDir;

#[derive(Debug, Args)]
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

/// Build a compiled scoring graph from in-memory YAML strings.
///
/// Each entry in `scoring_graphs` is the full text of one scoring-graph YAML
/// file (as produced by `reproto --emit-scoring-graphs`).  Returns the
/// serialised `.rkyv` bytes (the baked graph), ready to be written to disk.
///
/// This is the library entry point used by the `scoring_graph_lib` PyO3
/// extension (`scoring_graph.build_graph`).
pub fn build_from_strings(
    scoring_graphs: &[String],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let merged = load::merge_from_strings(scoring_graphs)?;
    if merged.states.is_empty() {
        return Err("no scoring-graph entries found in provided YAML strings".into());
    }
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg);
    let compiled = graph::compile(&raw, &reg, &partition, &merged);
    serial::to_bytes(&compiled)
}

pub fn run(args: BuildScoringGraphArgs) -> Result<(), Box<dyn std::error::Error>> {
    // ── 1. Collect YAML paths ─────────────────────────────────────────────────
    let yaml_paths = collect_yaml_paths(&args)?;
    if yaml_paths.is_empty() {
        return Err("no YAML files found".into());
    }

    // ── 2. Load and merge ─────────────────────────────────────────────────────
    let merged = load::load_and_merge(&yaml_paths)?;
    let yaml_count = yaml_paths.len();
    let types_before = merged.states.len();

    // ── 3. Build raw graph ────────────────────────────────────────────────────
    let (raw, reg) = graph::build(&merged);

    // ── 4. Hopcroft minimization ──────────────────────────────────────────────
    let partition = hopcroft::minimize(&raw, &reg);
    let states_after = partition.num_blocks();

    // ── 5. Serialize ──────────────────────────────────────────────────────────
    let compiled = graph::compile(&raw, &reg, &partition, &merged);
    let bytes_written = serial::write(&compiled, &args.output)?;

    // ── 5b. Optional HTML visualisation ──────────────────────────────────────
    if let Some(ref graph_path) = args.graph {
        show::write_html(graph_path, &raw, &partition, &compiled)?;
        if !args.quiet {
            eprintln!("  graph:  {}", graph_path.display());
        }
    }

    // ── 6. Summary ────────────────────────────────────────────────────────────
    if !args.quiet {
        let ratio = states_after as f64 / types_before.max(1) as f64;
        eprintln!("score-graph build-scoring-graph: loaded {yaml_count} YAML files");
        eprintln!(
            "  message types (before dedup): {:>10}",
            fmt_int(types_before)
        );
        eprintln!(
            "  states (after Hopcroft):      {:>10}  (dedup ratio: {ratio:.3})",
            fmt_int(states_after)
        );
        eprintln!(
            "  transitions:                  {:>10}",
            fmt_int(compiled.transitions.len())
        );
        eprintln!(
            "  root entries:                 {:>10}",
            fmt_int(compiled.roots.len())
        );
        eprintln!(
            "  output: {} ({:.1} KiB)",
            args.output.display(),
            bytes_written as f64 / 1024.0
        );
    }

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn collect_yaml_paths(
    args: &BuildScoringGraphArgs,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let root = args
        .input_root
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));

    if args.yaml_files.is_empty() {
        // Walk input_root recursively for *.yaml
        let paths = WalkDir::new(&root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
            .map(|e| e.into_path())
            .collect();
        return Ok(paths);
    }

    // Resolve each argument relative to root
    let mut paths = Vec::new();
    for pattern in &args.yaml_files {
        let candidate = Path::new(pattern);
        let resolved = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        };

        // Directory — walk recursively for *.yaml
        if resolved.is_dir() {
            for entry in WalkDir::new(&resolved)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
            {
                paths.push(entry.into_path());
            }
            continue;
        }

        // Glob pattern — match relative to the glob's own base directory
        if pattern.contains('*') || pattern.contains('?') {
            // Determine the non-glob prefix as the walk root
            let base = candidate
                .components()
                .take_while(|c| !c.as_os_str().to_string_lossy().contains(['*', '?']))
                .collect::<PathBuf>();
            let walk_root = if base.as_os_str().is_empty() {
                root.clone()
            } else if base.is_absolute() {
                base
            } else {
                root.join(base)
            };
            let glob = Glob::new(pattern)?.compile_matcher();
            for entry in WalkDir::new(&walk_root)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
            {
                // Match against path relative to root so the pattern works as typed
                let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
                if glob.is_match(rel) {
                    paths.push(entry.into_path());
                }
            }
            continue;
        }

        // Literal file path
        paths.push(resolved);
    }
    // Deduplicate while preserving order
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));
    Ok(paths)
}

fn fmt_int(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}
