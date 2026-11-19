// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Type names as protoc resolves them (spec 0434, invariant 0001), as
//! reproto's `names.py` writes them.
//!
//! A type reference is written in googleapis' convention (S4): a type of
//! the file's own package by the shortest suffix of its name within the
//! package, any other package's type by its full name without the leading
//! dot; and, when protoc would resolve that name to something else, with
//! the leading dot. Whether a name resolves to its target is decided by
//! simulating protoc's lookup (`DescriptorBuilder::LookupSymbolNoPlaceholder`,
//! protobuf's `descriptor.cc`) over the symbols visible from the file.

use crate::hash::{HashMap, HashSet};
use std::collections::BTreeSet;

use prototext_upb::descriptor_access::{DescriptorProto, EnumDescriptorProto, FileDescriptorProto};

use super::Shared;

/// `globals.type_names`: a scalar type's name by its
/// `FieldDescriptorProto.Type` number.
pub fn type_name(t: i32) -> String {
    match t {
        1 => "double",
        2 => "float",
        3 => "int64",
        4 => "uint64",
        5 => "int32",
        6 => "fixed64",
        7 => "fixed32",
        8 => "bool",
        9 => "string",
        10 => "group",
        11 => "message",
        12 => "bytes",
        13 => "uint32",
        14 => "enum",
        15 => "sfixed32",
        16 => "sfixed64",
        17 => "sint32",
        18 => "sint64",
        _ => return format!("<unknown type {t}>"),
    }
    .to_string()
}

/// A symbol's kind, as far as protoc's lookup tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolKind {
    Package,
    Message,
    Enum,
    Service,
    /// A field, oneof, extension, enum value or method.
    Other,
}

impl SymbolKind {
    fn is_type(self) -> bool {
        matches!(self, SymbolKind::Message | SymbolKind::Enum)
    }

    fn is_aggregate(self) -> bool {
        !matches!(self, SymbolKind::Other)
    }
}

/// protoc's `ResolveMode`: a field's type is looked up among types only;
/// an extendee and a method's types among every symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Types,
    All,
}

/// What protoc sees from one file (`FindSymbol`): the files it may read,
/// and the packages they are in, with every parent package.
#[derive(Debug, Default)]
pub struct Visible {
    files: HashSet<String>,
    packages: HashSet<String>,
}

/// Every symbol of the loaded files, by full name (no leading dot): its
/// kind in each file defining it (S1).
#[derive(Debug, Default)]
pub struct SymbolTable {
    symbols: HashMap<String, Vec<(String, SymbolKind)>>,
    packages: HashMap<String, String>,
    /// A file's imports, and its public ones.
    imports: HashMap<String, (Vec<String>, Vec<String>)>,
    /// A file's `import option`s (edition 2024).
    option_imports: HashMap<String, Vec<String>>,
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn join(prefix: &str, name: &[u8]) -> String {
    if prefix.is_empty() {
        text(name)
    } else {
        format!("{prefix}.{}", text(name))
    }
}

/// The output names the table holds (spec 0440 S3): symbols and packages
/// renamed, files by their input names.
pub struct Renaming<'v> {
    pub variant: &'v crate::variant::Variant,
    pub keep_descriptor: bool,
}

impl SymbolTable {
    pub fn build<'f>(
        files: impl IntoIterator<Item = FileDescriptorProto<'f>>,
        renaming: &Renaming<'_>,
    ) -> SymbolTable {
        let mut table = SymbolTable::default();
        for f in files {
            table.add_file(&f, renaming);
        }
        table
    }

    /// Symbol `name` (its input full name) of `file`, by its output name,
    /// renamed as it is added (spec 0442 S4): the definitions of input
    /// names the rules merge kept together, in file order.
    fn add(&mut self, name: &str, kind: SymbolKind, file: &str, r: &Renaming<'_>) {
        let out = r.variant.output_symbol(name, r.keep_descriptor);
        let defs = self.symbols.entry(out).or_default();
        if !defs.iter().any(|(f, _)| f == file) {
            defs.push((file.to_string(), kind));
        }
    }

    fn add_file(&mut self, f: &FileDescriptorProto<'_>, renaming: &Renaming<'_>) {
        let file = text(f.name());
        if self.packages.contains_key(&file) {
            return;
        }
        let package = text(f.package());
        self.packages.insert(
            file.clone(),
            renaming
                .variant
                .output_package(&package, &file, renaming.keep_descriptor),
        );
        let deps: Vec<String> = f.dependency().iter().map(|d| text(d)).collect();
        let public = f
            .public_dependency()
            .iter()
            .filter_map(|&i| usize::try_from(i).ok().and_then(|i| deps.get(i)).cloned())
            .collect();
        self.imports.insert(file.clone(), (deps, public));
        let options: Vec<String> = f.option_dependency().iter().map(|d| text(d)).collect();
        self.option_imports.insert(file.clone(), options);
        for m in f.message_type() {
            self.add_message(&package, &m, &file, renaming);
        }
        for e in f.enum_type() {
            self.add_enum(&package, &e, &file, renaming);
        }
        for x in f.extension() {
            self.add(
                &join(&package, x.name()),
                SymbolKind::Other,
                &file,
                renaming,
            );
        }
        for s in f.service() {
            let name = join(&package, s.name());
            self.add(&name, SymbolKind::Service, &file, renaming);
            for meth in s.method() {
                self.add(
                    &join(&name, meth.name()),
                    SymbolKind::Other,
                    &file,
                    renaming,
                );
            }
        }
    }

    fn add_message(&mut self, prefix: &str, m: &DescriptorProto<'_>, file: &str, r: &Renaming<'_>) {
        let name = join(prefix, m.name());
        self.add(&name, SymbolKind::Message, file, r);
        for f in m.field().iter().chain(m.extension().iter()) {
            self.add(&join(&name, f.name()), SymbolKind::Other, file, r);
        }
        for o in m.oneof_decl() {
            self.add(&join(&name, o.name()), SymbolKind::Other, file, r);
        }
        for n in m.nested_type() {
            self.add_message(&name, &n, file, r);
        }
        for e in m.enum_type() {
            self.add_enum(&name, &e, file, r);
        }
    }

    fn add_enum(
        &mut self,
        prefix: &str,
        e: &EnumDescriptorProto<'_>,
        file: &str,
        r: &Renaming<'_>,
    ) {
        self.add(&join(prefix, e.name()), SymbolKind::Enum, file, r);
        // An enum value is a symbol of the enum's own scope's parent.
        for v in e.value() {
            self.add(&join(prefix, v.name()), SymbolKind::Other, file, r);
        }
    }

    /// S2: the file, its imports and their `import public` chains, and the
    /// packages these files are in, every parent included.
    /// File `file`, its imports, theirs (those the input has): whose
    /// extensions its options see (spec 0429 S14).
    pub fn closure(&self, file: &str) -> HashSet<String> {
        let mut files = HashSet::default();
        let mut todo = vec![file.to_string()];
        // The file's own `import option`s supply its options too; theirs
        // do not reach it (protoc's rule; reproto's `_closure`).
        todo.extend(self.option_imports.get(file).into_iter().flatten().cloned());
        while let Some(f) = todo.pop() {
            if let Some((deps, _)) = self.imports.get(&f) {
                if files.insert(f) {
                    todo.extend(deps.iter().cloned());
                }
            }
        }
        files
    }

    pub fn visible(&self, file: &str) -> Visible {
        let mut files = BTreeSet::from([file.to_string()]);
        let mut todo: Vec<String> = self
            .imports
            .get(file)
            .map(|(deps, _)| deps.clone())
            .unwrap_or_default();
        while let Some(dep) = todo.pop() {
            if files.insert(dep.clone()) {
                if let Some((_, public)) = self.imports.get(&dep) {
                    todo.extend(public.iter().cloned());
                }
            }
        }
        let mut packages = HashSet::default();
        for f in &files {
            if let Some(package) = self.packages.get(f).filter(|p| !p.is_empty()) {
                let mut prefix = String::new();
                for part in package.split('.') {
                    if !prefix.is_empty() {
                        prefix.push('.');
                    }
                    prefix.push_str(part);
                    packages.insert(prefix.clone());
                }
            }
        }
        Visible {
            files: files.into_iter().collect(),
            packages,
        }
    }

    /// `FindSymbol`: the symbol `name`, if `visible` sees it.
    fn find(&self, name: &str, visible: &Visible) -> Option<SymbolKind> {
        if visible.packages.contains(name) {
            return Some(SymbolKind::Package);
        }
        self.symbols
            .get(name)?
            .iter()
            .find(|(f, _)| visible.files.contains(f))
            .map(|&(_, kind)| kind)
    }

    /// `LookupSymbolNoPlaceholder`: the full name protoc resolves `name` to
    /// from `scope`, the innermost scope enclosing the reference.
    fn resolve(&self, name: &str, scope: &str, mode: Mode, visible: &Visible) -> Option<String> {
        if let Some(full) = name.strip_prefix('.') {
            return self.find(full, visible).map(|_| full.to_string());
        }
        let first = name.split('.').next().unwrap_or(name);
        let compound = first.len() < name.len();
        let mut s = scope;
        while !s.is_empty() {
            let candidate = format!("{s}.{first}");
            if let Some(kind) = self.find(&candidate, visible) {
                if compound {
                    if kind.is_aggregate() {
                        let full = format!("{s}.{name}");
                        return self.find(&full, visible).map(|_| full);
                    }
                } else if mode == Mode::All || kind.is_type() {
                    return Some(candidate);
                }
            }
            s = s.rsplit_once('.').map_or("", |(parent, _)| parent);
        }
        self.find(name, visible).map(|_| name.to_string())
    }

    /// S4: the name to write for `target` (a full name, with its leading
    /// dot) in `file`, from `scope`.
    pub fn type_name(
        &self,
        target: &str,
        file: &str,
        visible: &Visible,
        scope: &str,
        mode: Mode,
    ) -> String {
        let full = target.strip_prefix('.').unwrap_or(target);
        let package = self.packages.get(file).map_or("", String::as_str);
        let defined_in = self.symbols.get(full);
        // The package of a file defining the target, not a prefix of its
        // name: a sub-package's type is another package's.
        let own = match defined_in {
            Some(defs) => defs
                .iter()
                .any(|(f, _)| self.packages.get(f).map(String::as_str) == Some(package)),
            None => package.is_empty() || full.starts_with(&format!("{package}.")),
        };
        let rel = if own && !package.is_empty() {
            &full[package.len() + 1..]
        } else {
            full
        };
        if defined_in.is_none() {
            // A target not loaded (a missing import, or a group the output
            // synthesizes): nothing to check it against. Relative to the
            // innermost scope enclosing it, as its declaration nests; else
            // by the convention alone.
            if own {
                let mut s = scope;
                while !s.is_empty() && s != package {
                    if let Some(rest) = full.strip_prefix(&format!("{s}.")) {
                        return rest.to_string();
                    }
                    s = s.rsplit_once('.').map_or("", |(parent, _)| parent);
                }
            }
            return rel.to_string();
        }
        if own {
            let parts: Vec<&str> = rel.split('.').collect();
            for i in (0..parts.len()).rev() {
                let candidate = parts[i..].join(".");
                if self.resolve(&candidate, scope, mode, visible).as_deref() == Some(full) {
                    return candidate;
                }
            }
        } else if self.resolve(full, scope, mode, visible).as_deref() == Some(full) {
            return full.to_string();
        }
        format!(".{full}")
    }
}

impl SymbolTable {
    /// `names.option_name` (spec 0439 S1): an option's extension, written
    /// in parentheses from `scope`, the first scope protoc's option lookup
    /// searches: in full without the dot when protoc resolves it, among
    /// every symbol, to the extension; else with it. An extension no loaded
    /// file declares is written in full: nothing can check it.
    pub fn option_name(&self, extension: &str, visible: &Visible, scope: &str) -> String {
        let full = extension.trim_start_matches('.');
        if !self.symbols.contains_key(full) {
            return full.to_string();
        }
        if self.resolve(full, scope, Mode::All, visible).as_deref() == Some(full) {
            full.to_string()
        } else {
            format!(".{full}")
        }
    }
}

/// The scope a node opens, by full name (no leading dot): a file's
/// package, a message's or service's own name.
pub fn scope_name(shared: &Shared<'_>, node: usize) -> String {
    shared.graph.nodes[node]
        .prefix
        .trim_start_matches('.')
        .to_string()
}

/// `utils.type_ref`: the name to write for the type `target` (its input
/// full name, leading dot included) in the file `ctx` renders, from
/// `scope` (an input name): the name protoc resolves back to it (spec 0434
/// S4), all in output names (spec 0440 S3).
pub fn type_ref(ctx: &super::file::Ctx<'_, '_>, target: &str, scope: &str, mode: Mode) -> String {
    let shared = ctx.shared;
    let keep = shared.keep_variant_descriptor;
    shared.names.type_name(
        &shared.variant.output_symbol(target, keep),
        &ctx.file_name(),
        &ctx.visible,
        &shared.variant.output_package(scope, &ctx.file_name(), keep),
        mode,
    )
}
