// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

pub(crate) mod graph;
pub(crate) mod hopcroft;
pub(crate) mod load;
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
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types);
    Ok(graph::compile(&raw, &reg, &partition, &merged.roots))
}

pub fn build_from_strings(
    scoring_graphs: &[String],
    emit_yaml: bool,
) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error>> {
    let merged = load::merge_from_strings(scoring_graphs)?;
    if merged.states.is_empty() {
        return Err("no scoring-graph entries found in provided YAML strings".into());
    }
    let (raw, reg) = graph::build(&merged);
    let partition = hopcroft::minimize(&raw, &reg, &raw.node_wire_types);
    let compiled = graph::compile(&raw, &reg, &partition, &merged.roots);
    let rkyv_bytes = serial::to_bytes(&compiled)?;
    let yaml = if emit_yaml {
        Some(serial::dump_compiled(&compiled))
    } else {
        None
    };
    Ok((rkyv_bytes, yaml))
}
