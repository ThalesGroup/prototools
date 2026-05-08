// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Post-build helper for the `fdp_scan` extension module.
//!
//! Generates `fdp_scan_lib.pyi` type stubs via pyo3_stub_gen.
//!
//! Run (from the workspace root):
//!   cargo run --release --package fdp_scan_extension --bin fdp_scan_post_build

use fdp_scan_lib::stub_info;

fn main() -> pyo3_stub_gen::Result<()> {
    stub_info()?.generate()?;
    Ok(())
}
