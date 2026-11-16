// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! reproto's warning collector (`lib/warnings.py`; spec 0433 S2), and the
//! record the graph dump keeps of phases 1 to 6 (spec 0428 S6).
//!
//! W3 and W7 print at once. W1 (and W5, the same cause), W4, W6 and the
//! files `--force-proto2-for-editions` patches are counted, then flushed
//! at the end as a sorted summary; with `--detailed-warnings`, each prints
//! at once instead. The dump records each call as made, and each message.

use std::collections::{BTreeMap, HashSet};

use crate::console;

#[derive(Debug, Default)]
pub struct Events {
    /// The graph dump's record (spec 0428 S6).
    pub lines: Vec<String>,
    /// `--detailed-warnings`.
    detailed: bool,
    w4: BTreeMap<String, usize>,
    w5: BTreeMap<String, usize>,
    w6: BTreeMap<String, usize>,
    prost: Vec<String>,
    /// Files reproto pruned itself: no W5 for them.
    pruned_files: HashSet<String>,
    /// Files an embedded fallback provides: no W1 for them.
    fallback_files: HashSet<String>,
}

fn occurrences(count: usize) -> String {
    let noun = if count == 1 {
        "occurrence"
    } else {
        "occurrences"
    };
    format!("({count} {noun})")
}

impl Events {
    pub fn new(detailed: bool) -> Events {
        Events {
            detailed,
            ..Events::default()
        }
    }

    /// The call itself: its code and arguments.
    fn call(&mut self, parts: &[&str]) {
        self.lines.push(parts.join(" | "));
    }

    pub fn register_pruned_file(&mut self, name: &str) {
        self.pruned_files.insert(name.to_string());
    }

    pub fn register_fallback_file(&mut self, name: &str) {
        self.fallback_files.insert(name.to_string());
    }

    /// A file not found on the `-I` path: W5's counter, unless pruned or a
    /// fallback's.
    pub fn w1(&mut self, missing_file: &str) {
        self.call(&["w1", missing_file]);
        if self.pruned_files.contains(missing_file) || self.fallback_files.contains(missing_file) {
            return;
        }
        if self.detailed {
            self.warning(&format!(
                "Warning: missing file '{missing_file}' (not found on -I path; skipped)"
            ));
        } else {
            *self.w5.entry(missing_file.to_string()).or_default() += 1;
        }
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
        if self.detailed {
            self.warning(&format!("Warning: unresolvable type {type_name}"));
        } else {
            *self.w4.entry(type_name.to_string()).or_default() += 1;
        }
    }

    /// A render that failed (A3, C5): the file, the field, the reason.
    pub fn w6(&mut self, proto_file: &str, field_ctx: &str, error: &str) {
        let key = format!("'{proto_file}' {field_ctx}: {error}");
        if self.detailed {
            console::warning(&format!("Warning: {key}"));
        } else {
            *self.w6.entry(key).or_default() += 1;
        }
    }

    /// Two files of one name, under two roots: the first kept.
    pub fn w7(&mut self, fdp_name: &str, shadowed_path: &str, kept_path: &str) {
        self.call(&["w7", fdp_name, shadowed_path, kept_path]);
        self.warning(&format!(
            "Warning: {shadowed_path}: definition of file:{fdp_name} shadowed by {kept_path}"
        ));
    }

    /// An editions file patched to proto2 (`--force-proto2-for-editions`).
    pub fn w_prost(&mut self, file_name: &str) {
        if self.detailed {
            console::warning(&format!(
                "'{file_name}' is an editions file; patching to proto2 for prost-reflect \
                 compatibility (--force-proto2-for-editions)."
            ));
        } else {
            self.prost.push(file_name.to_string());
        }
    }

    pub fn warning(&mut self, message: &str) {
        self.lines.push(format!("warning | {message}"));
        console::warning(message);
    }

    pub fn info(&mut self, message: &str) {
        self.lines.push(format!("info | {message}"));
        console::info(message);
    }

    pub fn attention(&mut self, message: &str) {
        self.lines.push(format!("attention | {message}"));
        console::attention(message);
    }

    /// The summary, at the end of the run, whatever ended it: W5 (W1's
    /// misses merged), W4, W6, each sorted, then the patched files' count
    /// and the hint to `--detailed-warnings`.
    pub fn flush(&mut self) {
        if self.detailed {
            return;
        }
        let mut repeated = false;
        for (dep, &count) in &self.w5 {
            console::warning(&format!(
                "Warning: missing dependency file:{dep} {}",
                occurrences(count)
            ));
            repeated |= count > 1;
        }
        for (type_name, &count) in &self.w4 {
            console::warning(&format!(
                "Warning: unresolvable type {type_name} {}",
                occurrences(count)
            ));
            repeated |= count > 1;
        }
        for (key, &count) in &self.w6 {
            console::warning(&format!("Warning: {key} {}", occurrences(count)));
            repeated |= count > 1;
        }
        if !self.prost.is_empty() {
            let n = self.prost.len();
            let noun = if n == 1 { "file" } else { "files" };
            console::warning(&format!(
                "{n} editions {noun} patched to proto2 for prost-reflect compatibility \
                 (--force-proto2-for-editions); run with --detailed-warnings to list them."
            ));
        }
        if repeated {
            console::info("Run with --detailed-warnings to see all warning occurrences.");
        }
    }
}
