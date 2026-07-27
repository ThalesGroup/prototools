// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Optional file trace, for investigating interactive latency.
//!
//! The TUI owns the terminal, so diagnostics cannot go to stdout or
//! stderr — they would be painted over by the next frame, or corrupt it.
//! Setting `PROTOLENS_TRACE=<path>` opens that file once, in append
//! mode, and every `trace!` appends one timestamped line to it. With the
//! variable unset — the normal case — a `trace!` costs one `OnceLock`
//! read and nothing else: the arguments are never formatted.
//!
//! Timestamps are seconds since the first `trace!` of the process, which
//! is what makes the log readable as a timeline: the interesting
//! quantity is almost always the gap between two lines, not the wall
//! clock.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static SINK: OnceLock<Option<Mutex<File>>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

fn sink() -> Option<&'static Mutex<File>> {
    SINK.get_or_init(|| {
        let path = std::env::var_os("PROTOLENS_TRACE")?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        Some(Mutex::new(file))
    })
    .as_ref()
}

pub(super) fn enabled() -> bool {
    sink().is_some()
}

/// Appends one line. Errors are swallowed: a trace that cannot be
/// written must never take down an interactive session.
pub(super) fn write(args: fmt::Arguments<'_>) {
    let Some(sink) = sink() else { return };
    let start = *START.get_or_init(Instant::now);
    let mut file = sink.lock().unwrap_or_else(|e| e.into_inner());
    let _ = writeln!(file, "{:10.4} {args}", start.elapsed().as_secs_f64());
}

macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::tui::trace::enabled() {
            $crate::tui::trace::write(format_args!($($arg)*));
        }
    };
}
pub(super) use trace;
