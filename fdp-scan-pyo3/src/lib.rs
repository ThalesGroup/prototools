// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// PyO3 essentials
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::Bound;

// Stub-generation helpers
use pyo3_stub_gen::derive::gen_stub_pyfunction;

// The scanner itself is the `fdp-scan` crate (spec 0407 S1); this crate is
// its Python binding, for reproto (`reproto -I <blob>`).

// ── scan ─────────────────────────────────────────────────────────────────────

/// Scan a binary buffer for FileDescriptorProto candidates.
///
/// Returns a list of (start, end) byte offsets for each candidate found.
#[gen_stub_pyfunction]
#[pyfunction]
fn scan(buffer: Bound<'_, PyBytes>) -> PyResult<Vec<(usize, usize)>> {
    let bytes = buffer.as_bytes();
    let candidates = fdp_scan::scan(bytes);
    Ok(candidates)
}

// ── Python module ─────────────────────────────────────────────────────────────

/// The Python module definition.
#[pymodule]
fn fdp_scan_lib(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(scan, m)?)?;
    Ok(())
}

/// Gather stub info for pyo3-stub-gen (called by the post_build binary).
///
/// The `use fdp_scan_lib::stub_info` in post_build.rs forces this lib to be
/// linked into the binary, ensuring all inventory items are present.
///
/// Uses std::env::var (runtime) rather than env!() (compile-time) so that the
/// binary works correctly when Cargo reuses it from a prior build's artifact
/// cache (e.g. when running under Crane/Nix where different derivations use
/// different sandbox paths).  The installPhase sets CARGO_MANIFEST_DIR before
/// invoking the binary.
pub fn stub_info() -> pyo3_stub_gen::Result<pyo3_stub_gen::StubInfo> {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR must be set when running fdp_scan_post_build"),
    );
    let mut info =
        pyo3_stub_gen::StubInfo::from_pyproject_toml(manifest_dir.join("pyproject.toml"))?;
    // Generate aside, into target/stubs/<crate dir>/, never over the
    // committed stub; the Nix build compares the two (spec 0409 S2).
    let crate_dir = manifest_dir
        .file_name()
        .expect("CARGO_MANIFEST_DIR has a final component");
    info.python_root = manifest_dir.join("../target/stubs").join(crate_dir);
    Ok(info)
}

// The unit tests are this library compiled as a test binary, which runs
// Python and so links libpython; the extension itself does not. See build.rs
// (spec 0402 S6).
#[cfg(test)]
include!(concat!(env!("OUT_DIR"), "/libpython.rs"));
