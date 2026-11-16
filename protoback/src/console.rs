// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What protoback prints on stderr, as reproto's console and logging
//! handlers print it (`lib/console.py`, `cli.py`; spec 0433 S3): warnings,
//! silenced by `--quiet`; information, attention and errors, always; the
//! phase labels.
//!
//! A message is one line, whatever its length (spec 0433 S1). Styles and
//! progress bars, on a terminal only, are stage 2.

use std::sync::OnceLock;

struct Console {
    quiet: bool,
}

static CONSOLE: OnceLock<Console> = OnceLock::new();

/// Set once, when the command line is read: `--quiet`.
pub fn init(quiet: bool) {
    let _ = CONSOLE.set(Console { quiet });
}

fn quiet() -> bool {
    CONSOLE.get().is_some_and(|c| c.quiet)
}

/// `cli_warning`: silenced by `--quiet`.
pub fn warning(message: &str) {
    if !quiet() {
        eprintln!("{message}");
    }
}

/// `cli_info`.
pub fn info(message: &str) {
    eprintln!("{message}");
}

/// `cli_attention`.
pub fn attention(message: &str) {
    eprintln!("{message}");
}

/// The label of a phase reproto shows with `progress` or `spinning`,
/// printed when the phase ends, unless `--quiet`.
pub fn phase_done(label: &str) {
    if !quiet() {
        eprintln!("{label}");
    }
}

/// A plain line on stderr, unless `--quiet` (the schema database's
/// summary, written with `sys.stderr.write`).
pub fn note(message: &str) {
    if !quiet() {
        eprintln!("{message}");
    }
}
