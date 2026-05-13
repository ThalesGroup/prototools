// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! PyO3 module definition for `scoring_graph`.
//!
//! Exposes `build_graph(scoring_graphs: list[str], emit_yaml: bool = False) -> tuple[bytes, str | None]`
//! to Python.

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3_stub_gen::derive::gen_stub_pyfunction;

use score_graph_lib::build_scoring_graph::build_from_strings;

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
///     When True, also return the compiled graph as a human-readable YAML
///     string (spec 0059 §2 format).  Default False.
///
/// Returns
/// -------
/// tuple[bytes, str | None]
///     First element: serialised ``.rkyv`` content (the baked graph).
///     Second element: compiled-graph YAML string when ``emit_yaml=True``,
///     otherwise ``None``.
///
/// Raises
/// ------
/// RuntimeError
///     If any YAML string is malformed or the graph cannot be built.
#[gen_stub_pyfunction]
#[pyfunction]
#[pyo3(signature = (scoring_graphs, emit_yaml = false))]
fn build_graph<'py>(
    py: Python<'py>,
    scoring_graphs: Vec<String>,
    emit_yaml: bool,
) -> PyResult<(Bound<'py, PyBytes>, Option<String>)> {
    let (rkyv_bytes, yaml) = build_from_strings(&scoring_graphs, emit_yaml)
        .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    Ok((PyBytes::new(py, &rkyv_bytes), yaml))
}

// ── Python module ─────────────────────────────────────────────────────────────

/// Rust extension for building compiled scoring graphs.
#[pymodule]
fn scoring_graph_lib(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(build_graph, m)?)?;
    Ok(())
}

/// Gather stub info for pyo3-stub-gen (called by the post_build binary).
pub fn stub_info() -> pyo3_stub_gen::Result<pyo3_stub_gen::StubInfo> {
    let pyproject = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("pyproject.toml");
    pyo3_stub_gen::StubInfo::from_pyproject_toml(pyproject)
}
