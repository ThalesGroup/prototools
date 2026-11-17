// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Phase 2's pool: Python's `DescriptorDatabase`, which reproto fills
//! (`ctx.pool_db`). Not a descriptor pool: it keeps files and indexes their
//! symbols, and builds nothing. It refuses only a name added twice with
//! other contents, which phase 2 cannot do, its files' names being the
//! topology's.

use crate::hash::HashMap;

use prototext_upb::descriptor_access::{DescriptorProto, FileDescriptorProto};
use prototext_upb::DynamicMessage;

#[derive(Default)]
pub struct Database {
    /// The files, in the order added.
    pub files: Vec<DynamicMessage>,
    by_name: HashMap<String, usize>,
    /// Symbol → the file that last declared it.
    symbols: HashMap<String, usize>,
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn join(package: &str, name: &str) -> String {
    if package.is_empty() {
        name.to_string()
    } else {
        format!("{package}.{name}")
    }
}

/// `descriptor_database._ExtractSymbols`: a message, its nested messages,
/// and the enums of each.
fn extract_symbols(m: &DescriptorProto<'_>, package: &str, out: &mut Vec<String>) {
    let name = join(package, &text(m.name()));
    out.push(name.clone());
    for nested in m.nested_type() {
        extract_symbols(&nested, &name, out);
    }
    for e in m.enum_type() {
        out.push(format!("{name}.{}", text(e.name())));
    }
}

/// reproto's `_extract_fdp_symbols`: the messages, at any depth, and the
/// enums of each and of the file.
pub fn file_symbols(fdp: &FileDescriptorProto<'_>) -> Vec<String> {
    fn collect(
        messages: Vec<DescriptorProto<'_>>,
        enums: Vec<String>,
        parent: &str,
        out: &mut Vec<String>,
    ) {
        for m in messages {
            let fqn = join(parent, &text(m.name()));
            out.push(fqn.clone());
            let nested_enums = m.enum_type().iter().map(|e| text(e.name())).collect();
            collect(m.nested_type(), nested_enums, &fqn, out);
        }
        for e in enums {
            out.push(join(parent, &e));
        }
    }
    let mut out = Vec::new();
    let enums = fdp.enum_type().iter().map(|e| text(e.name())).collect();
    collect(fdp.message_type(), enums, &text(fdp.package()), &mut out);
    out
}

impl Database {
    /// `DescriptorDatabase.Add`: the file, and its symbols indexed, a later
    /// file's taking the place of an earlier's.
    pub fn add(&mut self, fdp: DynamicMessage) {
        let f = FileDescriptorProto(fdp.as_ref());
        let name = text(f.name());
        if self.by_name.contains_key(&name) {
            // The same name again: phase 2 never does it (module docs).
            return;
        }
        let i = self.files.len();
        let package = text(f.package());
        let mut symbols = Vec::new();
        for m in f.message_type() {
            extract_symbols(&m, &package, &mut symbols);
        }
        for e in f.enum_type() {
            symbols.push(join(&package, &text(e.name())));
            for v in e.value() {
                symbols.push(join(&package, &text(v.name())));
            }
        }
        for x in f.extension() {
            symbols.push(join(&package, &text(x.name())));
        }
        for s in f.service() {
            symbols.push(join(&package, &text(s.name())));
        }
        for s in symbols {
            self.symbols.insert(s, i);
        }
        self.by_name.insert(name, i);
        self.files.push(fdp);
    }

    pub fn file_by_name(&self, name: &str) -> Option<&DynamicMessage> {
        self.by_name.get(name).map(|&i| &self.files[i])
    }

    /// The position of the file named `name`, if added.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    /// `DescriptorDatabase.FindFileContainingSymbol`: the symbol, else the
    /// one it is nested in (a field, an enum value), one level up only. A
    /// symbol with a single `.`, its first character, loses it.
    pub fn file_containing_symbol(&self, symbol: &str) -> Option<usize> {
        let symbol = if symbol.matches('.').count() == 1 && symbol.starts_with('.') {
            symbol.trim_start_matches('.')
        } else {
            symbol
        };
        if let Some(&i) = self.symbols.get(symbol) {
            return Some(i);
        }
        let top_level = symbol.rsplit_once('.').map_or("", |(t, _)| t);
        self.symbols.get(top_level).copied()
    }

    /// The name of file `i`.
    pub fn name(&self, i: usize) -> String {
        text(FileDescriptorProto(self.files[i].as_ref()).name())
    }
}
