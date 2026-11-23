// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! reproto's warning collector (`lib/warnings.py`; spec 0433 S2, spec
//! 0438), and the record the graph dump keeps of phases 1 to 6 (spec 0428
//! S6).
//!
//! Every warning has a code (spec 0438 S1), is counted under it, for
//! `--fail-on`, and prints as `Warning [CODE]: message`. W1 (summarized
//! with W5, the same cause), W4, W6, W19 to W22 and the text reader's T1
//! to T4 are counted by message, then flushed at the end as a sorted
//! summary, as are the files `--force-proto2-for-editions` patches (W2);
//! with `--detailed-warnings`, each prints at once instead. The dump
//! records each call as made, and each message.

use crate::hash::HashSet;
use std::collections::{BTreeMap, BTreeSet};

use crate::console;

/// Spec 0438 S1, in its order: what each code means.
pub const CODES: &[(&str, &str)] = &[
    (
        "W1",
        "a file not found on the -I path; skipped (summarized with W5)",
    ),
    (
        "W2",
        "an editions file written as proto2 (--force-proto2-for-editions)",
    ),
    (
        "W3",
        "a duplicate pruned: a symbol, or an extension number on one extendee",
    ),
    ("W4", "an unresolvable type"),
    ("W5", "a missing dependency"),
    ("W6", "an option value or a file that cannot be rendered"),
    (
        "W7",
        "a file shadowed by one of the same name under another -I root",
    ),
    ("W8", "a descriptor the pool refuses while loading"),
    ("W9", "an unparseable file skipped while loading"),
    ("W10", "a circular dependency"),
    ("W11", "a prune pattern matching nothing"),
    ("W12", "a seed pattern matching nothing"),
    (
        "W13",
        "two files rewritten onto one path, the variant's descriptor.proto kept",
    ),
    ("W14", "--dump-resolved-features: a file not loaded"),
    ("W15", "an input that cannot be parsed"),
    ("W16", "a self-dependency stripped"),
    ("W17", "a variant key no tool reads"),
    ("W18", "a deprecated option or variable"),
    (
        "W19",
        "a file the descriptor pool refuses: its options not rendered",
    ),
    ("W20", "a schema-database entry that cannot be built"),
    ("W21", "a scoring-graph entry that cannot be built"),
    (
        "W22",
        "an option field the options type does not declare: an orphan",
    ),
    (
        "W23",
        "an import of a pruned file stripped from its importer",
    ),
    (
        "W24",
        "a definition lost to a move: no file its importer sees defines it",
    ),
    (
        "W25",
        "a move that is idle: nothing imports its file, no input holds it",
    ),
    (
        "W26",
        "a move that is wrong, not applied (or a target the input lacks)",
    ),
    (
        "W27",
        "the input's own google/protobuf/descriptor.proto dropped for the vendor descriptor",
    ),
    (
        "W28",
        "an option the target's reader cannot represent, redacted",
    ),
    (
        "W29",
        "a profile folder's file replacing an input's copy that defines something it lacks",
    ),
    (
        "W30",
        "an edition 2024 file written with enforce_naming_style = STYLE_LEGACY (target portable)",
    ),
    (
        "W31",
        "a symbol local to its file written export: another file uses it (--export-used)",
    ),
    (
        "W32",
        "a nested symbol another file uses, which STRICT visibility can't export (--export-used)",
    ),
    (
        "W33",
        "an import added: the one written file defining a type used (--repair-skew)",
    ),
    (
        "W34",
        "a field or method commented out: its type is defined in no file (--repair-skew)",
    ),
    (
        "W35",
        "an extension commented out: its number is in no extension range of its extendee (--repair-skew)",
    ),
    (
        "W36",
        "a type several written files define, not imported: not repaired (--repair-skew)",
    ),
    (
        "W37",
        "the Python protobuf imported is older than reproto's descriptor.proto",
    ),
    (
        "T1",
        "text: a field by number declared nowhere, read as length-delimited",
    ),
    ("T2", "text: a string protoc printed as a message, rebuilt"),
    ("T3", "text: an [extension] declared nowhere, skipped"),
    ("T4", "text: a field name declared nowhere, skipped"),
];

/// The `.proto` anomalies' codes (`anomalies.ANOMALIES`), which
/// `--fail-on` takes too.
pub const ANOMALIES: &[&str] = &[
    "A1", "A2", "A3", "A4", "A5", "B1", "B2", "B3", "C1", "C2", "C3", "C4", "C5", "D1", "D2",
];

/// Whether `code` is one `--fail-on` takes.
pub fn is_code(code: &str) -> bool {
    CODES.iter().any(|(c, _)| *c == code) || ANOMALIES.contains(&code)
}

/// The squashed codes, in the order their summary lines are printed: the
/// most actionable first (spec 0041), then by code.
const SQUASHED_ORDER: &[&str] = &[
    "W5", "W4", "W6", "W19", "W20", "W21", "W22", "W28", "W30", "W31", "W32", "W33", "W34", "W35",
    "W36", "T1", "T2", "T3", "T4",
];

const KNOCK_ON: &str = "the descriptor pool refused its import '";

/// The file a knock-on refusal leads to, followed through the imports
/// refused (spec 0474 S2): one refused for a reason of its own; `None`
/// when the chain ends nowhere or loops.
fn root_of<'a>(
    reasons: &BTreeMap<&'a str, BTreeSet<&'a str>>,
    mut file: &'a str,
) -> Option<String> {
    let mut seen = BTreeSet::from([file]);
    while let Some(r) = reasons.get(file).filter(|r| r.len() == 1) {
        let error = *r.iter().next()?;
        let Some(dep) = error
            .strip_prefix(KNOCK_ON)
            .and_then(|x| x.strip_suffix('\''))
        else {
            return Some(file.to_string());
        };
        if !seen.insert(dep) {
            return None;
        }
        file = dep;
    }
    None
}

/// The W19 and W20 summary messages printed, each to its line (spec
/// 0474): a W20 repeating its file's W19 left out (S1); a W19 refused
/// through its imports left out, counted on its root's line (S2).
fn said_once(
    w19: &BTreeMap<String, usize>,
    w20: &BTreeMap<String, usize>,
    refusals: &BTreeMap<String, (String, String, String)>,
) -> BTreeMap<String, String> {
    // A file's own W19 reasons (no field context).
    let mut reasons: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for message in w19.keys() {
        let (file, ctx, error) = &refusals[message];
        if ctx.is_empty() {
            reasons.entry(file).or_default().insert(error);
        }
    }
    let mut under: BTreeMap<String, usize> = BTreeMap::new();
    let mut lines = BTreeMap::new();
    for message in w19.keys() {
        let (file, ctx, error) = &refusals[message];
        let root = (ctx.is_empty() && error.starts_with(KNOCK_ON))
            .then(|| root_of(&reasons, file))
            .flatten();
        match root {
            Some(root) if root != *file => *under.entry(root).or_default() += 1,
            _ => {
                lines.insert(message.clone(), message.clone());
            }
        }
    }
    for (message, line) in lines.iter_mut() {
        let (file, ctx, error) = &refusals[message];
        let n = if ctx.is_empty() && !error.starts_with(KNOCK_ON) {
            under.get(file).copied().unwrap_or(0)
        } else {
            0
        };
        if n > 0 {
            let noun = if n == 1 { "file" } else { "files" };
            *line = format!("{message} (and {n} {noun} importing it)");
        }
    }
    for message in w20.keys() {
        let (file, _, error) = &refusals[message];
        if !reasons
            .get(file.as_str())
            .is_some_and(|r| r.contains(error.as_str()))
        {
            lines.insert(message.clone(), message.clone());
        }
    }
    lines
}

#[derive(Debug, Default)]
pub struct Events {
    /// The graph dump's record (spec 0428 S6).
    pub lines: Vec<String>,
    /// Whether the record is kept: from the start of the run on, as
    /// reproto's recorder, created there; the command line's own warnings
    /// (deprecated options, the variant's keys) are printed and counted,
    /// not recorded.
    recording: bool,
    /// `--detailed-warnings`.
    detailed: bool,
    /// Every warning, by code.
    counts: BTreeMap<String, usize>,
    /// Squashed mode: summary code → message → occurrences.
    squashed: BTreeMap<&'static str, BTreeMap<String, usize>>,
    /// W19 and W20 summary messages → (file, context, reason), to say a
    /// refusal once (spec 0474).
    refusals: BTreeMap<String, (String, String, String)>,
    prost: Vec<String>,
    /// Files reproto pruned itself: no W5 for them.
    pruned_files: HashSet<String>,
    /// Files an embedded fallback provides: no W1 for them.
    fallback_files: HashSet<String>,
    /// The report's fields for the next warning (spec 0446 S3).
    pending: Option<crate::report::Fields>,
    /// The code and message the report gives the next warning, when not
    /// the ones printed (W1, shown as W5 unless detailed).
    report_as: Option<(&'static str, String)>,
    /// The next warning stays out of the report (`w1_unreported`).
    unreported: bool,
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

    /// The record starts: the run begins (`dump_graph.Recorder()`).
    pub fn record(&mut self) {
        self.recording = true;
    }

    fn keep(&mut self, line: String) {
        if self.recording {
            self.lines.push(line);
        }
    }

    /// The call itself: its code and arguments.
    fn call(&mut self, parts: &[&str]) {
        self.keep(parts.join(" | "));
    }

    pub fn register_pruned_file(&mut self, name: &str) {
        self.pruned_files.insert(name.to_string());
    }

    pub fn register_fallback_file(&mut self, name: &str) {
        self.fallback_files.insert(name.to_string());
    }

    fn count(&mut self, code: &str) {
        debug_assert!(is_code(code), "{code}");
        *self.counts.entry(code.to_string()).or_default() += 1;
    }

    /// The fields the report gives the next warning (spec 0446 S3).
    pub fn with_fields(&mut self, fields: serde_json::Value) -> &mut Self {
        self.pending = Some(crate::report::fields(fields));
        self
    }

    /// One occurrence, into the report: every one, whatever is printed.
    fn occur(&mut self, code: &str, message: &str) {
        if std::mem::take(&mut self.unreported) {
            self.pending = None;
            self.report_as = None;
            return;
        }
        let fields = self.pending.take().unwrap_or_default();
        match self.report_as.take() {
            Some((code, message)) => crate::report::warning(code, &message, fields),
            None => crate::report::warning(code, message, fields),
        }
    }

    /// One warning line, recorded and printed (`_line`).
    fn line(&mut self, code: &str, message: &str) {
        let text = format!("Warning [{code}]: {message}");
        self.keep(format!("warning | {text}"));
        console::warning(&text);
    }

    /// A warning (`warn`): counted under `code`, printed now.
    pub fn warn(&mut self, code: &str, message: &str) {
        self.count(code);
        self.occur(code, message);
        self.line(code, message);
    }

    /// A warning of a squashed code (`squash`): counted under `code`;
    /// printed now in detailed mode (as `detailed`, else `message`), else
    /// summarized by `message` under `summary` (`code` when `None`).
    pub fn squash(
        &mut self,
        code: &str,
        message: &str,
        summary: Option<&str>,
        detailed: Option<&str>,
    ) {
        self.count(code);
        self.occur(summary.unwrap_or(code), message);
        if self.detailed {
            self.line(code, detailed.unwrap_or(message));
            return;
        }
        let summary = summary.unwrap_or(code);
        let Some(&key) = SQUASHED_ORDER.iter().find(|c| **c == summary) else {
            unreachable!("{summary} is not squashed")
        };
        *self
            .squashed
            .entry(key)
            .or_default()
            .entry(message.to_string())
            .or_default() += 1;
    }

    /// A file not found on the `-I` path: summarized with W5, unless pruned
    /// or a fallback's.
    pub fn w1(&mut self, missing_file: &str) {
        self.call(&["w1", missing_file]);
        if self.pruned_files.contains(missing_file) || self.fallback_files.contains(missing_file) {
            self.pending = None;
            self.unreported = false;
            return;
        }
        self.report_as = Some(("W5", format!("missing dependency file:{missing_file}")));
        if self.pending.is_none() {
            self.pending = Some(crate::report::fields(
                serde_json::json!({ "import": missing_file }),
            ));
        }
        if self.detailed {
            self.warn(
                "W1",
                &format!("missing file '{missing_file}' (not found on -I path; skipped)"),
            );
        } else {
            self.squash(
                "W1",
                &format!("missing dependency file:{missing_file}"),
                Some("W5"),
                None,
            );
        }
    }

    /// `w1` for an import not found during import discovery: printed and
    /// counted as ever, but not in the report, where phase 2 gives it once,
    /// with its importer (spec 0446 S3).
    pub fn w1_unreported(&mut self, missing_file: &str) {
        self.unreported = true;
        self.w1(missing_file);
    }

    /// A file pruned for its duplicate symbols or extension numbers (W3).
    pub fn w3(&mut self, message: &str) {
        self.call(&["w3", message]);
        self.warn("W3", message);
    }

    /// An import of a pruned file stripped from its importer (W23, spec
    /// 0438 S1): apart from W3, so that `--fail-on W3` means a duplicate
    /// pruned and nothing else.
    pub fn w23(&mut self, message: &str) {
        self.call(&["w23", message]);
        self.warn("W23", message);
    }

    /// A type no file defines (W4).
    pub fn w4(&mut self, type_name: &str) {
        self.call(&["w4", type_name]);
        self.squash("W4", &format!("unresolvable type {type_name}"), None, None);
    }

    /// Something not rendered or built: W6, W19, W20 or W21 (`w6`).
    pub fn w6(&mut self, code: &str, proto_file: &str, field_ctx: &str, error: &str) {
        if crate::report::active() {
            let mut f = serde_json::json!({ "file": proto_file });
            if let Some(field) = field_ctx
                .strip_prefix("field '")
                .and_then(|x| x.strip_suffix('\''))
            {
                f["field"] = field.into();
            }
            // Spec 0438 S8's forms: a knock-on refusal names its import,
            // a root one upb's reason; else, a W6, its reason as said.
            if let Some(dep) = error
                .strip_prefix("the descriptor pool refused its import '")
                .and_then(|x| x.strip_suffix('\''))
            {
                f["import"] = dep.into();
            } else if let Some(upb) = error.strip_prefix("the descriptor pool refused it: ") {
                f["reason"] = upb.into();
            } else if code == "W6" && error != "internal error" {
                f["reason"] = error.into();
            }
            self.with_fields(f);
        }
        let message = format!("'{proto_file}' {field_ctx}: {error}");
        if code == "W19" || code == "W20" {
            self.refusals.insert(
                message.clone(),
                (proto_file.into(), field_ctx.into(), error.into()),
            );
        }
        self.squash(code, &message, None, None);
    }

    /// Two files of one name, under two roots: the first kept (W7).
    pub fn w7(&mut self, fdp_name: &str, shadowed_path: &str, kept_path: &str) {
        self.call(&["w7", fdp_name, shadowed_path, kept_path]);
        self.warn(
            "W7",
            &format!("{shadowed_path}: definition of file:{fdp_name} shadowed by {kept_path}"),
        );
    }

    /// An editions file written as proto2 (`--force-proto2-for-editions`,
    /// W2).
    pub fn w_prost(&mut self, file_name: &str) {
        self.count("W2");
        self.with_fields(serde_json::json!({ "file": file_name }));
        self.occur(
            "W2",
            &format!(
                "'{file_name}' is an editions file; written as proto2 \
                 (--force-proto2-for-editions)"
            ),
        );
        if self.detailed {
            self.line(
                "W2",
                &format!(
                    "'{file_name}' is an editions file; written as proto2 \
                     (--force-proto2-for-editions)"
                ),
            );
        } else {
            self.prost.push(file_name.to_string());
        }
    }

    pub fn info(&mut self, message: &str) {
        self.keep(format!("info | {message}"));
        console::info(message);
    }

    pub fn attention(&mut self, message: &str) {
        self.keep(format!("attention | {message}"));
        console::attention(message);
    }

    /// The summary, at the end of the run, whatever ended it: the squashed
    /// codes, each sorted, then the patched files' count and the hint to
    /// `--detailed-warnings`.
    pub fn flush(&mut self) {
        if self.detailed {
            return;
        }
        let mut repeated = false;
        let squashed = std::mem::take(&mut self.squashed);
        let none = BTreeMap::new();
        let lines = said_once(
            squashed.get("W19").unwrap_or(&none),
            squashed.get("W20").unwrap_or(&none),
            &self.refusals,
        );
        for code in SQUASHED_ORDER {
            for (message, &count) in squashed.get(code).into_iter().flatten() {
                let line = if *code == "W19" || *code == "W20" {
                    match lines.get(message) {
                        Some(line) => line,
                        None => {
                            repeated = true;
                            continue;
                        }
                    }
                } else {
                    message
                };
                self.line(code, &format!("{line} {}", occurrences(count)));
                repeated |= count > 1;
            }
        }
        if !self.prost.is_empty() {
            let n = self.prost.len();
            let noun = if n == 1 { "file" } else { "files" };
            self.line(
                "W2",
                &format!(
                    "{n} editions {noun} written as proto2 (--force-proto2-for-editions); \
                     run with --detailed-warnings to list them."
                ),
            );
        }
        if repeated {
            console::info("Run with --detailed-warnings to see all warning occurrences.");
        }
    }

    /// How many warnings of `code` were given.
    #[cfg(test)]
    pub fn count_of(&self, code: &str) -> usize {
        self.counts.get(code).copied().unwrap_or(0)
    }

    /// The `--fail-on` codes that occurred, `T4 (3 occurrences), W5 (1
    /// occurrence)` in the codes' order (spec 0438 S4), or `None`.
    pub fn failed_on(&self, codes: &[String]) -> Option<String> {
        let order = CODES
            .iter()
            .map(|(c, _)| *c)
            .chain(ANOMALIES.iter().copied());
        let hit: Vec<String> = order
            .filter(|c| codes.iter().any(|x| x == c))
            .filter_map(|c| {
                let n = *self.counts.get(c)?;
                Some(format!("{c} {}", occurrences(n)))
            })
            .collect();
        (!hit.is_empty()).then(|| hit.join(", "))
    }
}
