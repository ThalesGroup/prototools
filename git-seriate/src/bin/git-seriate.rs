// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `git seriate`: store snapshots of files, and rebuild a history from them
//! (spec 0452 S8). git runs this binary as `git seriate` when it is on the
//! path.

use std::process::ExitCode;

use git_seriate::cli::{self, Kit, NoExtra};

fn main() -> ExitCode {
    cli::run::<NoExtra>(
        "git seriate",
        |_| Ok(Kit::generic()),
        |never, _, _| match never {},
    )
}
