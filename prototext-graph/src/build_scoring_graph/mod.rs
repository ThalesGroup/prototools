// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

pub(crate) mod graph;
pub(crate) mod hopcroft;
pub(crate) mod load;

pub use load::{FieldLabel, FieldSpec, FieldType, NodeKind, ScoringFile, ScoringMessage};
pub mod serial;

/// Build a compiled scoring graph from in-memory YAML strings.
///
/// Each entry in `scoring_graphs` is the full text of one scoring-graph YAML
/// file (as produced by `reproto --emit-scoring-graphs`).  Always returns the
/// serialised `.rkyv` bytes as the first element.  When `emit_yaml` is `true`,
/// also returns the compiled-graph YAML (§2 format) as the second element;
/// otherwise the second element is `None`.
///
/// This is the library entry point used by the `scoring_graph_lib` PyO3
/// extension (`scoring_graph.build_graph`).
/// Build a compiled scoring graph and return it directly (without serialization).
/// Used by the test suite to inspect the graph without a serialize/deserialize round-trip.
pub fn build_compiled(
    scoring_graphs: &[String],
) -> Result<serial::CompiledGraph, Box<dyn std::error::Error>> {
    let merged = load::merge_from_strings(scoring_graphs)?;
    if merged.states.is_empty() {
        return Err("no scoring-graph entries found in provided YAML strings".into());
    }
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
    Ok(graph::compile(&raw, &reg, &partition, &merged.roots))
}

/// Return type of [`build_from_strings`]: rkyv bytes, optional Hopcroft YAML,
/// optional pre-Hopcroft (initial) YAML.
pub type BuildResult =
    Result<(Vec<u8>, Option<String>, Option<String>), Box<dyn std::error::Error>>;

pub fn build_from_strings(
    scoring_graphs: &[String],
    emit_yaml: bool,
    emit_initial_yaml: bool,
    on_progress: impl FnMut(u64, u64),
) -> BuildResult {
    let merged = load::merge_from_strings(scoring_graphs)?;
    if merged.states.is_empty() {
        return Err("no scoring-graph entries found in provided YAML strings".into());
    }
    build_merged(&merged, emit_yaml, emit_initial_yaml, on_progress)
}

/// [`build_from_strings`] over scoring graphs given as structures (spec
/// 0430 S6): the same merge, the same bytes, without YAML between the
/// producer and the merge.
pub fn build_from_files(
    files: &[ScoringFile],
    emit_yaml: bool,
    emit_initial_yaml: bool,
    on_progress: impl FnMut(u64, u64),
) -> BuildResult {
    let merged = load::merge_files(files)?;
    if merged.states.is_empty() {
        return Err("no scoring-graph entries found in provided scoring graphs".into());
    }
    build_merged(&merged, emit_yaml, emit_initial_yaml, on_progress)
}

fn build_merged(
    merged: &load::Merged,
    emit_yaml: bool,
    emit_initial_yaml: bool,
    on_progress: impl FnMut(u64, u64),
) -> BuildResult {
    let (raw, reg) = graph::build(merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types, on_progress);
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
    let rkyv_bytes = serial::to_bytes(&compiled)?;
    let initial_yaml = if emit_initial_yaml {
        let initial = graph::compile_initial(&raw, &reg, &merged.roots);
        Some(serial::dump_compiled(&initial))
    } else {
        None
    };
    let yaml = if emit_yaml {
        Some(serial::dump_compiled(&compiled))
    } else {
        None
    };
    Ok((rkyv_bytes, yaml, initial_yaml))
}

/// Each stage timed on a real corpus (spec 0443 P7): the per-file scoring
/// YAML under `$PROTOTEXT_GRAPH_TIMING_DIR` (protoback's
/// `--emit-scoring-yaml` output), merged and built as the schema database
/// builds them. Ignored by default; run in release with `--ignored
/// --nocapture`.
#[cfg(test)]
mod timing {
    use std::time::Instant;

    #[test]
    #[ignore]
    fn each_stage_timed() {
        let Some(dir) = std::env::var_os("PROTOTEXT_GRAPH_TIMING_DIR") else {
            eprintln!("PROTOTEXT_GRAPH_TIMING_DIR unset: nothing timed");
            return;
        };
        let mut texts: Vec<(std::path::PathBuf, String)> = walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
            .filter(|e| e.file_name() != "buf.yaml")
            .map(|e| {
                (
                    e.path().to_path_buf(),
                    std::fs::read_to_string(e.path()).unwrap(),
                )
            })
            .collect();
        texts.sort();
        // Parsed outside the clock: protoback hands over structures.
        let files: Vec<super::ScoringFile> = texts
            .iter()
            .map(|(_, t)| super::load::parse_file(t).unwrap())
            .collect();
        let t = Instant::now();
        let merged = super::load::merge_files(&files).unwrap();
        let t_merge = t.elapsed();
        let t = Instant::now();
        let (raw, reg) = super::graph::build(&merged);
        let t_build = t.elapsed();
        let t = Instant::now();
        let partition = super::hopcroft::minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
        let t_min = t.elapsed();
        let t = Instant::now();
        let compiled = super::graph::compile(&raw, &reg, &partition, &merged.roots);
        let t_compile = t.elapsed();
        let t = Instant::now();
        let bytes = super::serial::to_bytes(&compiled).unwrap();
        let t_serial = t.elapsed();
        eprintln!(
            "files {}, states {}, nodes {}\n\
             merge {:?}\nbuild {:?}\nminimize {:?}\ncompile {:?}\nserialize {:?} ({} bytes)",
            files.len(),
            merged.states.len(),
            raw.node_wire_types.len(),
            t_merge,
            t_build,
            t_min,
            t_compile,
            t_serial,
            bytes.len()
        );
    }
}
