// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use prototext_codec_lib::stub_info;

fn main() -> pyo3_stub_gen::Result<()> {
    stub_info()?.generate()?;
    Ok(())
}
