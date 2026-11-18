// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0446: `--report-out FILE`, a JSON report of the run, for a program
//! to read: every warning, unsquashed, with its fields; every prune and
//! move done; every directive that did not apply; the run itself.
//!
//! One recorder for the process, inactive without `--report-out`: its facts
//! come from the collector, the loader, phases 1 to 4 and the moves, and
//! the run can end at any of `main`'s exits, each of which `finish` serves.
//! Passing it through every phase instead would touch them all for the same
//! record.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{Map, Value};

/// The structured facts of one warning (spec 0446 S3).
pub type Fields = Map<String, Value>;

/// `json!({…})`'s object as fields.
pub fn fields(v: Value) -> Fields {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// A major version: a field added keeps it; one changed or removed bumps
/// it (spec 0446 S3).
const REPORT_VERSION: u32 = 1;

#[derive(Serialize, Default)]
struct Report {
    report_version: u32,
    complete: bool,
    tool: Tool,
    run: RunInfo,
    warnings: Vec<Warning>,
    rewrites: Rewrites,
    idle_directives: Vec<Idle>,
}

#[derive(Serialize, Default)]
struct Tool {
    name: &'static str,
    version: &'static str,
}

#[derive(Serialize, Default)]
struct RunInfo {
    argv: Vec<String>,
    cwd: String,
    variant: Option<String>,
    inputs: Vec<Input>,
    exit_status: i32,
    error: Option<String>,
}

/// A physical file read: its `-I` root, its path under it, and the FDPs
/// it gave.
#[derive(Serialize)]
struct Input {
    root: String,
    path: String,
    files: Vec<String>,
}

#[derive(Serialize)]
struct Warning {
    code: String,
    message: String,
    fields: Fields,
}

#[derive(Serialize, Default)]
struct Rewrites {
    prunes: Vec<Value>,
    moves: Vec<Value>,
}

#[derive(Serialize)]
struct Idle {
    /// None for what is not warned: a variants: section for another
    /// variant.
    code: Option<String>,
    directive: String,
    reason: String,
}

struct Recorder {
    path: PathBuf,
    report: Report,
}

static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);

fn with(f: impl FnOnce(&mut Report)) {
    let mut guard = RECORDER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = guard.as_mut() {
        f(&mut r.report);
    }
}

/// Whether a report is being made: callers that would compute facts for it
/// alone check first.
pub fn active() -> bool {
    RECORDER.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// `--report-out` read: a previous report removed at once, so that none
/// outlives this run (spec 0446 S1), and the recording started.
pub fn start(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("--report-out {}: {e}", path.display())),
    }
    let report = Report {
        report_version: REPORT_VERSION,
        tool: Tool {
            name: "protoback",
            version: env!("CARGO_PKG_VERSION"),
        },
        run: RunInfo {
            argv: std::env::args().collect(),
            cwd: std::env::current_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
            ..RunInfo::default()
        },
        ..Report::default()
    };
    *RECORDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Recorder {
        path: path.to_path_buf(),
        report,
    });
    Ok(())
}

pub fn variant(name: &str) {
    with(|r| r.run.variant = Some(name.to_string()));
}

pub fn input(root: &Path, path: &Path, files: Vec<String>) {
    with(|r| {
        r.run.inputs.push(Input {
            root: root.display().to_string(),
            path: path.display().to_string(),
            files,
        })
    });
}

/// One warning occurrence, as stderr shows its code.
pub fn warning(code: &str, message: &str, fields: Fields) {
    with(|r| {
        r.warnings.push(Warning {
            code: code.to_string(),
            message: message.to_string(),
            fields,
        })
    });
}

/// A file pruned, and what caused it (spec 0446 S4).
pub fn pruned_file(file: &str, cause: Value) {
    with(|r| {
        r.rewrites
            .prunes
            .push(serde_json::json!({ "file": file, "cause": cause }))
    });
}

/// A physical file a `path:` pattern kept from loading.
/// Once per path: import discovery retries a missing import every round.
pub fn pruned_path(path: &Path, pattern: &str) {
    with(|r| {
        let path = path.display().to_string();
        let seen = r.rewrites.prunes.iter().any(|p| p["path"] == path.as_str());
        if !seen {
            r.rewrites.prunes.push(serde_json::json!({
                "path": path,
                "cause": { "pattern": format!("path:{pattern}") },
            }));
        }
    });
}

/// A `-p` FQDN pattern and the definitions it pruned, one entry per
/// pattern.
pub fn pruned_definitions(pattern: &str, definitions: Vec<String>) {
    with(|r| {
        r.rewrites
            .prunes
            .push(serde_json::json!({ "pattern": pattern, "definitions": definitions }))
    });
}

/// An importer a move re-pointed.
pub fn moved(importer: &str, olds: &[String], before: &[String], after: &[String]) {
    with(|r| {
        r.rewrites.moves.push(serde_json::json!({
            "importer": importer,
            "moves": olds,
            "imports_before": before,
            "imports_after": after,
        }))
    });
}

/// A directive that did not apply as written (W11, W12, W25, W26; or no
/// code, a variants: section for another variant).
pub fn idle(code: Option<&str>, directive: &str, reason: &str) {
    with(|r| {
        r.idle_directives.push(Idle {
            code: code.map(str::to_string),
            directive: directive.to_string(),
            reason: reason.to_string(),
        })
    });
}

/// Why the run failed, as it said on stderr.
pub fn error(message: &str) {
    with(|r| r.run.error = Some(message.to_string()));
}

/// The run over with `status`: the report written beside its path, then
/// renamed over it, so that no reader sees it half-written (spec 0446 S1).
/// Complete when the run went to its end (0, or 3 for `--fail-on`).
pub fn finish(status: u8) {
    let Some(mut rec) = RECORDER.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    rec.report.run.exit_status = i32::from(status);
    rec.report.complete = status == 0 || status == 3;
    let Ok(mut text) = serde_json::to_string_pretty(&rec.report) else {
        return;
    };
    text.push('\n');
    let name = rec
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = rec
        .path
        .with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let written = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &rec.path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        crate::console::error(&format!("Error: --report-out {}: {e}", rec.path.display()));
    }
}
