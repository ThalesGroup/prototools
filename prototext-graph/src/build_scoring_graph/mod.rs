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
