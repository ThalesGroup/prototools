// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Post-build helper for the `scoring_graph` extension module.
//!
//! Generates `scoring_graph_lib.pyi` type stubs via pyo3_stub_gen.
//!
//! Run (from the workspace root):
//!   cargo run --release --package scoring_graph_extension --bin scoring_graph_post_build

use scoring_graph_lib::stub_info;

fn main() -> pyo3_stub_gen::Result<()> {
    stub_info()?.generate()?;
    Ok(())
}
