// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! What phases 1 to 6 warn about, in order (spec 0428 S6): the warning
//! collector's calls and the CLI's messages, as reproto's dump records
//! them. Printing them as reproto does is step 9.
//!
//! A collector call is recorded as made, then what reproto's collector
//! does with it at once: W3 and W7 print their warning; W1 and W4, counted
//! for the summary after the run (without `--detailed-warnings`, step 9),
//! print nothing yet.

#[derive(Debug, Default)]
pub struct Events {
    pub lines: Vec<String>,
}

impl Events {
    /// The call itself: its code and arguments.
    fn call(&mut self, parts: &[&str]) {
        self.lines.push(parts.join(" | "));
    }

    /// A file not found on the `-I` path.
    pub fn w1(&mut self, missing_file: &str) {
        self.call(&["w1", missing_file]);
    }

    /// A file pruned for its duplicate symbols, or a pruned import
    /// stripped: the message, printed as given.
    pub fn w3(&mut self, message: &str) {
        self.call(&["w3", message]);
        self.warning(message);
    }

    /// A type no file defines.
    pub fn w4(&mut self, type_name: &str) {
        self.call(&["w4", type_name]);
    }

    /// Two files of one name, under two roots: the first kept.
    pub fn w7(&mut self, fdp_name: &str, shadowed_path: &str, kept_path: &str) {
        self.call(&["w7", fdp_name, shadowed_path, kept_path]);
        self.warning(&format!(
            "Warning: {shadowed_path}: definition of file:{fdp_name} shadowed by {kept_path}"
        ));
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
