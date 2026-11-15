// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What phases 1 to 6 warn about, in order (spec 0428 S6): the warning
//! collector's calls and the CLI's messages, as reproto's dump records
//! them. Printing them as reproto does is step 9.

#[derive(Debug, Default)]
pub struct Events {
    pub lines: Vec<String>,
}

impl Events {
    /// A collector call: its code (`w1` to `w7`) and arguments.
    pub fn w(&mut self, parts: &[&str]) {
        self.lines.push(parts.join(" | "));
    }

    pub fn warning(&mut self, message: &str) {
        self.lines.push(format!("warning | {message}"));
    }

    pub fn info(&mut self, message: &str) {
        self.lines.push(format!("info | {message}"));
    }

    pub fn attention(&mut self, message: &str) {
        self.lines.push(format!("attention | {message}"));
    }
}
