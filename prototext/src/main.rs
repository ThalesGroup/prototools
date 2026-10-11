// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use clap::{CommandFactory, Parser};
use clap_complete::CompleteEnv;

use prototext::Cli;

mod man;
#[cfg(test)]
pub mod protocraft;

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    // PROTOTEXT_GEN_MAN=<dir>: write the man page and exit (spec 0413 S3).
    man::generate_and_exit_if_requested();

    // Dynamic shell completion — same model as Cargo.
    // When PROTOTEXT_COMPLETE=<shell> is set, print the completion script and exit.
    // Bash completes from the raw command line (spec 0406).
    CompleteEnv::with_factory(Cli::command)
        .var("PROTOTEXT_COMPLETE")
        .shells(prototools_complete::shells())
        .complete();

    let cli = Cli::parse();

    if let Err(e) = prototext::run::run(cli) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
