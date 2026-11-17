// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! An input's descriptors, binary and text, read in import order (spec
//! 0427).
//!
//! protoc requires a file using an option to import the file declaring
//! it. [`read_input`] reads the files imports first into one pool, so
//! that a text file, parsed once when its turn comes, finds the
//! declarations of its options in the files read before it. A file upb
//! refuses is added mended ([`crate::mend`]). A text file whose parse left
//! a value declared nowhere (W1, W3) is parsed once more at the end, with
//! every file in the pool: a file using options it declares itself, a
//! cycle, a set protoc did not produce.

use std::collections::{BTreeSet, HashMap};

use crate::set::name_and_imports;
use crate::text_format::{self, descriptor_type, Options, TextError, Warning};
use crate::{LoadReport, Pool, DESCRIPTOR_SET};

/// One file of an input.
#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    /// A serialized `FileDescriptorProto`.
    Binary(&'a [u8]),
    /// A `FileDescriptorProto` in the text format.
    Text(&'a str),
}

/// What became of a text file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// A binary file: nothing to parse.
    Binary,
    /// Parsed: the bytes, what the pool could not settle, and whether it
    /// took a second parse.
    Parsed {
        bytes: Vec<u8>,
        warnings: Vec<Warning>,
        parsed_again: bool,
    },
    /// The text does not parse.
    Error(TextError),
}

/// Names unknown in the text, skipped, but not values guessed: what a
/// second parse can settle.
fn settles_more(w: &Warning) -> bool {
    matches!(
        w,
        Warning::Undeclared { .. }
            | Warning::UnknownExtension { .. }
            | Warning::UnknownField { .. }
    )
}

const LENIENT: Options = Options {
    allow_unknown_field: true,
    allow_unknown_extension: true,
};

/// Read `inputs` into one pool, imports first (spec 0427 S2–S5): each
/// text file parsed when its turn comes, as the `FileDescriptorProto` of
/// `descriptor_file` (the vendored `descriptor.proto` when no input has
/// that name). The pool, what became of each input, in order, and which
/// files the pool could not hold, even mended.
pub fn read_input(inputs: &[Input<'_>], descriptor_file: &str) -> (Pool, Vec<Read>, LoadReport) {
    let n = inputs.len();
    let mut reads: Vec<Read> = vec![Read::Binary; n];
    // Each input's name and imports; `None` for a text that does not lex.
    let mut heads: Vec<Option<(String, Vec<String>)>> = Vec::with_capacity(n);
    for (i, input) in inputs.iter().enumerate() {
        heads.push(match input {
            Input::Binary(b) => name_and_imports(b).ok(),
            Input::Text(t) => match text_format::scan_imports(t) {
                Ok(head) => Some(head),
                Err(e) => {
                    reads[i] = Read::Error(e);
                    None
                }
            },
        });
    }
    let pool = Pool::new();
    if !heads
        .iter()
        .flatten()
        .any(|(name, _)| name == descriptor_file)
    {
        pool.load_set(DESCRIPTOR_SET);
    }
    // Kahn's order, by input index; a cycle broken at its first file.
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (i, head) in heads.iter().enumerate() {
        if let Some((name, _)) = head {
            index.entry(name.as_str()).or_insert(i);
        }
    }
    let mut waiting = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, head) in heads.iter().enumerate() {
        for import in head.iter().flat_map(|(_, imports)| imports) {
            if let Some(&j) = index.get(import.as_str()) {
                if j != i {
                    waiting[i] += 1;
                    dependents[j].push(i);
                }
            }
        }
    }
    let mut pending: BTreeSet<usize> = (0..n).filter(|&i| heads[i].is_some()).collect();
    let mut ready: BTreeSet<usize> = pending
        .iter()
        .copied()
        .filter(|&i| waiting[i] == 0)
        .collect();
    let mut report = LoadReport::default();
    while let Some(&first) = pending.first() {
        let i = ready.pop_first().unwrap_or(first);
        pending.remove(&i);
        ready.remove(&i);
        let bytes = match inputs[i] {
            Input::Binary(b) => Some(b.to_vec()),
            Input::Text(t) => match parse(&pool, descriptor_file, t) {
                Ok(parsed) => {
                    let bytes = parsed.bytes.clone();
                    reads[i] = Read::Parsed {
                        bytes: parsed.bytes,
                        warnings: parsed.warnings,
                        parsed_again: false,
                    };
                    Some(bytes)
                }
                Err(e) => {
                    reads[i] = Read::Error(e);
                    None
                }
            },
        };
        if let Some(bytes) = bytes {
            let name = &heads[i].as_ref().unwrap().0;
            // The schema is not mended: a descriptor.proto that loads only
            // without what upb refuses in it would be read as, and lose its
            // fields in, every text (spec 0436). Refused, the texts are read
            // with the vendored one, and the refusal is reported.
            add(&pool, name, &bytes, name != descriptor_file, &mut report);
        }
        for &d in &dependents[i] {
            waiting[d] -= 1;
            if waiting[d] == 0 && pending.contains(&d) {
                ready.insert(d);
            }
        }
    }
    // S5: parsed again where the whole pool may settle more.
    for (i, input) in inputs.iter().enumerate() {
        let (Input::Text(t), Read::Parsed { warnings, .. }) = (input, &reads[i]) else {
            continue;
        };
        if !warnings.iter().any(settles_more) {
            continue;
        }
        if let Ok(again) = parse(&pool, descriptor_file, t) {
            reads[i] = Read::Parsed {
                bytes: again.bytes,
                warnings: again.warnings,
                parsed_again: true,
            };
        }
    }
    (pool, reads, report)
}

fn parse(pool: &Pool, descriptor_file: &str, text: &str) -> Result<text_format::Parsed, TextError> {
    let def = descriptor_type(pool, descriptor_file, "FileDescriptorProto")
        .expect("the vendored descriptor.proto declares it");
    text_format::parse_reporting(&def, text, LENIENT)
}

/// Add one file to `pool`, mended if upb refuses it as it is.
fn add(pool: &Pool, name: &str, bytes: &[u8], may_mend: bool, report: &mut LoadReport) {
    let reason = match pool.add_file(bytes) {
        Ok(_) => {
            report.loaded.push(name.to_string());
            return;
        }
        Err(e) => e.0,
    };
    let mended =
        may_mend && crate::mend::mend(pool, bytes).is_some_and(|m| pool.add_file(&m).is_ok());
    if mended {
        report.loaded.push(name.to_string());
    } else {
        report.skipped.push((name.to_string(), reason));
    }
}
