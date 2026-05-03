// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use clap::{CommandFactory, Parser};
use clap_complete::CompleteEnv;

use prototext::Cli;

#[cfg(test)]
pub mod protocraft;

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    // Dynamic shell completion — same model as Cargo.
    // When PROTOTEXT_COMPLETE=<shell> is set, print the completion script and exit.
    CompleteEnv::with_factory(Cli::command)
        .var("PROTOTEXT_COMPLETE")
        .complete();

    let cli = Cli::parse();

    if let Err(e) = prototext::run::run(cli) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
