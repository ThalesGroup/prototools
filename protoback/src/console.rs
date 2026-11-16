// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What protoback prints on stderr, as reproto's console and logging
//! handlers print it (`lib/console.py`, `cli.py`; spec 0433 S3, S5):
//! warnings, silenced by `--quiet`; information, attention and errors,
//! always; the phases, their labels, and on a terminal their bars.
//!
//! A message is one line, whatever its length (spec 0433 S1). On a
//! terminal, warnings are bold yellow, errors bold red, attention bold
//! blue, and each phase shows a progress bar while it runs, removed when
//! it ends; a message printed meanwhile goes above the bar. The bars are
//! protoback's own, not a copy of reproto's (the user's decision).

use std::io::IsTerminal;
use std::sync::{Mutex, OnceLock};

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

struct Console {
    quiet: bool,
    terminal: bool,
    /// The bar of the phase running, if any.
    bar: Mutex<Option<ProgressBar>>,
}

static CONSOLE: OnceLock<Console> = OnceLock::new();

/// Set once, when the command line is read: `--quiet`, and whether stderr
/// is a terminal.
pub fn init(quiet: bool) {
    let _ = CONSOLE.set(Console {
        quiet,
        terminal: std::io::stderr().is_terminal(),
        bar: Mutex::new(None),
    });
}

fn quiet() -> bool {
    CONSOLE.get().is_some_and(|c| c.quiet)
}

fn terminal() -> bool {
    CONSOLE.get().is_some_and(|c| c.terminal)
}

/// How a message is shown on a terminal.
#[derive(Clone, Copy)]
enum Style {
    Plain,
    Warning,
    Error,
    Attention,
}

/// A line on stderr, above the running bar if there is one; styled on a
/// terminal.
fn print(line: &str, style: Style) {
    // The SGR sequences Rich writes for reproto's styles.
    let sgr = match style {
        Style::Plain => None,
        Style::Warning => Some("1;33"),
        Style::Error => Some("1;31"),
        Style::Attention => Some("1;34"),
    };
    let text = match (terminal(), sgr) {
        (true, Some(code)) => format!("\x1b[{code}m{line}\x1b[0m"),
        _ => line.to_string(),
    };
    let bar = CONSOLE
        .get()
        .and_then(|c| c.bar.lock().ok().and_then(|b| b.clone()));
    match bar {
        Some(b) => b.suspend(|| eprintln!("{text}")),
        None => eprintln!("{text}"),
    }
}

/// `cli_warning`: silenced by `--quiet`.
pub fn warning(message: &str) {
    if !quiet() {
        print(message, Style::Warning);
    }
}

/// `cli_info`.
pub fn info(message: &str) {
    print(message, Style::Plain);
}

/// `cli_attention`.
pub fn attention(message: &str) {
    print(message, Style::Attention);
}

/// `cli_error`.
pub fn error(message: &str) {
    print(message, Style::Error);
}

/// A plain line on stderr, unless `--quiet` (the schema database's
/// summary, written with `sys.stderr.write`).
pub fn note(message: &str) {
    if !quiet() {
        print(message, Style::Plain);
    }
}

/// When a phase's label is printed, as reproto prints it: piped output is
/// compared byte for byte (spec 0433 S3). On a terminal, protoback shows
/// its own bars (spec 0433 S5, N2).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// reproto's `progress` and `spinning`: when the phase ends, unless
    /// `--quiet`.
    Always,
    /// reproto's `progress_lazy`: only on a terminal.
    OnTerminal,
}

/// A phase running: its bar on a terminal, its label when it ends.
pub struct Phase {
    label: &'static str,
    rule: Label,
    bar: Option<ProgressBar>,
}

/// A phase starts, a bar of `total` steps on a terminal (none under
/// `--quiet`); its total can be set later.
pub fn phase(label: &'static str, rule: Label, total: u64) -> Phase {
    let bar = (terminal() && !quiet()).then(|| {
        let b = ProgressBar::with_draw_target(Some(total), ProgressDrawTarget::stderr());
        if let Ok(style) =
            ProgressStyle::with_template("{msg:28} {wide_bar} {pos}/{len} {elapsed_precise}")
        {
            b.set_style(style);
        }
        b.set_message(label);
        b
    });
    if let (Some(c), Some(b)) = (CONSOLE.get(), &bar) {
        if let Ok(mut slot) = c.bar.lock() {
            *slot = Some(b.clone());
        }
    }
    Phase { label, rule, bar }
}

impl Phase {
    /// The bar moves on by `n`.
    pub fn advance(&self, n: u64) {
        if let Some(b) = &self.bar {
            b.inc(n);
        }
    }

    /// The bar's total, known now, and where it stands.
    pub fn set(&self, position: u64, total: u64) {
        if let Some(b) = &self.bar {
            b.set_length(total);
            b.set_position(position);
        }
    }

    /// The phase ends: its bar removed, its label printed by reproto's
    /// rules.
    pub fn done(self) {
        if let Some(b) = &self.bar {
            b.finish_and_clear();
            if let Some(c) = CONSOLE.get() {
                if let Ok(mut slot) = c.bar.lock() {
                    *slot = None;
                }
            }
        }
        let printed = match self.rule {
            Label::Always => !quiet(),
            Label::OnTerminal => !quiet() && terminal(),
        };
        if printed {
            print(self.label, Style::Plain);
        }
    }
}
