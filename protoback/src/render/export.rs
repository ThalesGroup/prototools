// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `--export-used` (spec 0458): the messages and enums local to their file
//! that another file to render uses, which are written `export` so that
//! protoc compiles the output.

use std::collections::BTreeSet;

use prototext_upb::descriptor_access::{DescriptorProto, FileDescriptorProto};

use crate::hash::HashMap;
use crate::phase2::VisibilityDefaults;

use super::Shared;

/// descriptor.proto's `SymbolVisibility`.
const VISIBILITY_LOCAL: i32 = 1;
pub(super) const VISIBILITY_EXPORT: i32 = 2;
/// descriptor.proto's `DefaultSymbolVisibility`.
const EXPORT_TOP_LEVEL: i32 = 2;
const LOCAL_ALL: i32 = 3;
const STRICT: i32 = 4;
/// descriptor.proto's `EDITION_2024`: visibility's first edition.
const EDITION_2024: i32 = 1001;

/// A local symbol another file uses.
#[derive(Debug, Clone)]
pub struct Export {
    /// The files using it, sorted.
    pub users: BTreeSet<String>,
    /// Whether it can be exported: not a nested symbol under `STRICT`
    /// (S5).
    pub exportable: bool,
}

/// By full name, without the leading dot.
pub type Exports = HashMap<String, Export>;

/// A message or enum of a file to render: its file, and whether it is
/// local (S2), and can't be exported, nested under `STRICT` (S5).
struct Symbol {
    file: String,
    local: bool,
    unexportable: bool,
}

/// The local symbols another file to render uses (S2, S3).
pub fn exports(shared: &Shared<'_>, defaults: &VisibilityDefaults) -> Exports {
    let files: Vec<FileDescriptorProto<'_>> = super::summoned(shared.graph)
        .into_iter()
        .filter_map(|i| shared.file_def(i))
        .collect();
    let written_as_editions = !(shared.force_proto2_output || shared.force_proto2_for_editions);

    let mut symbols: HashMap<String, Symbol> = HashMap::default();
    for f in &files {
        let file = text(f.name());
        let edition = super::file::file_edition(f);
        let visibility =
            written_as_editions && f.syntax() == b"editions" && edition >= EDITION_2024;
        // The file's own default, else its edition's (its file-level
        // feature is source-retained: descriptors seldom carry it).
        let default = f
            .options()
            .and_then(|o| o.features())
            .filter(|fs| fs.has_default_symbol_visibility())
            .map(|fs| fs.default_symbol_visibility())
            .or_else(|| {
                defaults
                    .iter()
                    .rev()
                    .find(|(e, _)| *e <= edition)
                    .map(|(_, d)| *d)
            })
            .unwrap_or(0);
        let local = |set: bool, value: i32, nested: bool| {
            visibility
                && match (set, value) {
                    (true, VISIBILITY_LOCAL) => true,
                    (true, VISIBILITY_EXPORT) => false,
                    _ => match default {
                        EXPORT_TOP_LEVEL => nested,
                        LOCAL_ALL | STRICT => true,
                        _ => false,
                    },
                }
        };
        let mut add = |name: String, set: bool, value: i32, nested: bool| {
            symbols.insert(
                name,
                Symbol {
                    file: file.clone(),
                    local: local(set, value, nested),
                    unexportable: nested && default == STRICT,
                },
            );
        };
        let package = text(f.package());
        let scope = if package.is_empty() {
            String::new()
        } else {
            format!("{package}.")
        };
        for e in f.enum_type() {
            add(
                format!("{scope}{}", text(e.name())),
                e.has_visibility(),
                e.visibility(),
                false,
            );
        }
        let mut todo: Vec<(String, DescriptorProto<'_>, bool)> = f
            .message_type()
            .into_iter()
            .map(|m| (scope.clone(), m, false))
            .collect();
        while let Some((scope, m, nested)) = todo.pop() {
            let name = format!("{scope}{}", text(m.name()));
            add(name.clone(), m.has_visibility(), m.visibility(), nested);
            for e in m.enum_type() {
                add(
                    format!("{name}.{}", text(e.name())),
                    e.has_visibility(),
                    e.visibility(),
                    true,
                );
            }
            for n in m.nested_type() {
                todo.push((format!("{name}."), n, true));
            }
        }
    }

    let mut out = Exports::default();
    for f in &files {
        let user = text(f.name());
        for reference in references(f) {
            let Some(s) = symbols.get(reference.trim_start_matches('.')) else {
                continue;
            };
            if !s.local || s.file == user {
                continue;
            }
            out.entry(reference.trim_start_matches('.').to_string())
                .or_insert_with(|| Export {
                    users: BTreeSet::new(),
                    exportable: !s.unexportable,
                })
                .users
                .insert(user.clone());
        }
    }
    out
}

/// The type names `f` refers to: fields' types, extensions' extendees,
/// methods' input and output types, nested messages' included.
fn references(f: &FileDescriptorProto<'_>) -> Vec<String> {
    let mut out = Vec::new();
    for e in f.extension() {
        out.push(text(e.extendee()));
        out.push(text(e.type_name()));
    }
    for s in f.service() {
        for m in s.method() {
            out.push(text(m.input_type()));
            out.push(text(m.output_type()));
        }
    }
    let mut todo: Vec<DescriptorProto<'_>> = f.message_type();
    while let Some(m) = todo.pop() {
        for x in m.field().into_iter().chain(m.extension()) {
            out.push(text(x.type_name()));
            out.push(text(x.extendee()));
        }
        todo.extend(m.nested_type());
    }
    out.retain(|r| !r.is_empty());
    out
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}
