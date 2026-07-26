// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! PyO3 module definition for `prototext_graph`.
//!
//! Exposes `build_graph(scoring_graphs: list[str], emit_yaml: bool = False, emit_initial_yaml: bool = False) -> tuple[bytes, str | None, str | None]`
//! to Python.

use std::collections::HashMap;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3_stub_gen::derive::gen_stub_pyfunction;

use prototext_graph::build_scoring_graph::build_from_strings;
use prototext_graph::fds_index::{canonical_map, to_bytes as fds_index_to_bytes, FdsIndex};

// ── API functions ─────────────────────────────────────────────────────────────

/// Build a compiled (baked) scoring graph from in-memory YAML strings.
///
/// Parameters
/// ----------
/// scoring_graphs : list[str]
///     YAML content of each per-file scoring graph, as produced by
///     ``reproto --emit-scoring-graphs``.  One entry per file; order does
///     not matter.
/// emit_yaml : bool, optional
///     When True, also return the minimised (post-Hopcroft) compiled graph as
///     a human-readable YAML string (spec 0059 §2 format).  Default False.
/// emit_initial_yaml : bool, optional
///     When True, also return the raw (pre-Hopcroft) compiled graph as a
///     human-readable YAML string (spec 0059 §2 format).  Default False.
///
/// Returns
/// -------
/// tuple[bytes, str | None, str | None]
///     First element: serialised ``.rkyv`` content (the baked graph).
///     Second element: minimised YAML when ``emit_yaml=True``, else ``None``.
///     Third element: raw YAML when ``emit_initial_yaml=True``, else ``None``.
///
/// Raises
/// ------
/// RuntimeError
///     If any YAML string is malformed or the graph cannot be built.
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (scoring_graphs, emit_yaml = false, emit_initial_yaml = false, on_progress = None))]
fn build_graph<'py>(
    py: Python<'py>,
    scoring_graphs: Vec<String>,
    emit_yaml: bool,
    emit_initial_yaml: bool,
    on_progress: Option<Py<PyAny>>,
) -> PyResult<(Bound<'py, PyBytes>, Option<String>, Option<String>)> {
    let result = py.detach(|| {
        build_from_strings(
            &scoring_graphs,
            emit_yaml,
            emit_initial_yaml,
            |current, total| {
                if let Some(ref cb) = on_progress {
                    Python::attach(|py| {
                        let _ = cb.call1(py, (current, total));
                    });
                }
            },
        )
        .map_err(|e| e.to_string())
    });
    let (rkyv_bytes, yaml, initial_yaml) = result.map_err(PyRuntimeError::new_err)?;
    Ok((PyBytes::new(py, &rkyv_bytes), yaml, initial_yaml))
}

/// Serialize an FdsIndex to rkyv bytes with the PTSGRAPH header (version 4).
///
/// Parameters
/// ----------
/// type_to_file : dict[str, str]
///     Fully-qualified type name (no leading dot) → proto file name.
/// file_to_span : dict[str, tuple[int, int]]
///     Proto file name → (start, end) byte offsets in the raw .pb file.
/// dep_graph : dict[str, list[str]]
///     Proto file name → list of direct import file names.
/// ext_to_file : dict[str, str]
///     "extendee_fqdn/field_number" → proto file name (spec 0100 §4).
///
/// Returns
/// -------
/// bytes
///     Serialized ``index.rkyv`` content (PTSGRAPH header + rkyv payload).
///
/// Raises
/// ------
/// RuntimeError
///     If serialization fails.
#[gen_stub_pyfunction]
#[pyfunction]
fn build_fds_index<'py>(
    py: Python<'py>,
    type_to_file: HashMap<String, String>,
    file_to_span: HashMap<String, (u64, u64)>,
    dep_graph: HashMap<String, Vec<String>>,
    ext_to_file: HashMap<String, String>,
) -> PyResult<Bound<'py, PyBytes>> {
    // The parameters stay plain `HashMap` so neither `FromPyObject` nor
    // `pyo3-stub-gen`'s `PyStubType` has to be generic over the hasher. Their
    // iteration order is per-process random, so they go through
    // `canonical_map` (spec 0177 S1) — one sort per map, at build time, over
    // an index that is written exactly once.
    let index = FdsIndex {
        type_to_file: canonical_map(type_to_file),
        file_to_span: canonical_map(file_to_span),
        dep_graph: canonical_map(dep_graph),
        ext_to_file: canonical_map(ext_to_file),
    };
    let bytes = fds_index_to_bytes(&index).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    Ok(PyBytes::new(py, &bytes))
}

// ── Python module ─────────────────────────────────────────────────────────────

/// Rust extension for building compiled scoring graphs.
#[pymodule]
fn prototext_graph_lib(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(build_graph, m)?)?;
    m.add_function(wrap_pyfunction!(build_fds_index, m)?)?;
    Ok(())
}

/// Gather stub info for pyo3-stub-gen (called by the post_build binary).
///
/// Uses std::env::var (runtime) rather than env!() (compile-time) so that the
/// binary works correctly when Cargo reuses it from a prior build's artifact
/// cache (different sandbox paths under Crane/Nix).
pub fn stub_info() -> pyo3_stub_gen::Result<pyo3_stub_gen::StubInfo> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR must be set when running scoring_graph_post_build");
    let pyproject = std::path::Path::new(&manifest_dir).join("pyproject.toml");
    pyo3_stub_gen::StubInfo::from_pyproject_toml(pyproject)
}
