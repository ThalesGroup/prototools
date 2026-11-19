// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0445: `move: OLD: TARGET` (or `OLD: [TARGET, …]`). Every import of
//! `OLD` becomes imports of its targets, and `OLD` is pruned. Nothing else
//! is rewritten: what a move leaves inconsistent is said (W24), a move
//! that does nothing (W25) or is wrong (W26) too.

use crate::hash::HashMap;
use std::collections::{BTreeMap, BTreeSet};

use prototext_upb::descriptor_access::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorProtoMut,
};
use prototext_upb::DynamicMessage;

use crate::db::{extension_keys, file_symbols, Database};
use crate::events::Events;
use crate::pathmatch::fqdn_match;

/// A move that did not apply as written (W25, W26): warned, and listed in
/// the report's `idle_directives` (spec 0446 S5).
fn not_applied(events: &mut Events, code: &str, m: &Move, message: &str) {
    crate::report::idle(
        code,
        &format!("move: {}: {}", m.old, m.targets.join(", ")),
        message,
    );
    events.warn(code, message);
}

/// One directive, as given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub old: String,
    pub targets: Vec<String>,
}

/// `OLD=TARGET[,TARGET…]`, `--move`'s value.
pub fn parse_cli(value: &str) -> Result<Move, String> {
    let bad = || format!("--move expects OLD=TARGET[,TARGET...], got '{value}'");
    let (old, targets) = value.split_once('=').ok_or_else(bad)?;
    let targets: Vec<String> = targets.split(',').map(str::to_string).collect();
    if old.is_empty() || targets.iter().any(String::is_empty) {
        return Err(bad());
    }
    Ok(Move {
        old: old.to_string(),
        targets,
    })
}

/// An importer a move re-pointed: its imports before and after, and the
/// moves that did it.
#[derive(Debug, Clone, Default)]
pub struct Repointed {
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub olds: BTreeSet<String>,
}

/// The moves applied, and what they did.
#[derive(Debug, Default)]
pub struct Moves {
    /// The directives applied, in order.
    pub applied: Vec<Move>,
    by_old: HashMap<String, usize>,
    /// Per directive: whether an import of its `OLD` was read (phase 1).
    imported: Vec<bool>,
    /// The importers re-pointed (phase 2), by name.
    pub repointed: BTreeMap<String, Repointed>,
}

impl Moves {
    /// The directives to apply, each wrong one left out with W26 (spec 0445
    /// S7): an `OLD` given twice, a target itself moved (a chain, or a
    /// cycle), a target a `file:` prune pattern matches.
    pub fn new(given: Vec<Move>, prunings: &[String], events: &mut Events) -> Moves {
        let mut olds: BTreeSet<&str> = BTreeSet::new();
        let mut firsts = Vec::new();
        for m in &given {
            if olds.insert(&m.old) {
                firsts.push(m);
            } else {
                not_applied(
                    events,
                    "W26",
                    m,
                    &format!("move '{}': given twice; the second not applied", m.old),
                );
            }
        }
        let mut moves = Moves::default();
        for m in firsts {
            let broken = m.targets.iter().find_map(|t| {
                if olds.contains(t.as_str()) {
                    Some(format!("its target '{t}' is itself moved"))
                } else {
                    prunings
                        .iter()
                        .find(|p| fqdn_match(p, &format!("file:{t}")))
                        .map(|p| format!("its target '{t}' is pruned ({p})"))
                }
            });
            if let Some(why) = broken {
                not_applied(
                    events,
                    "W26",
                    m,
                    &format!("move '{}': {why}; not applied", m.old),
                );
                continue;
            }
            moves.by_old.insert(m.old.clone(), moves.applied.len());
            moves.applied.push(m.clone());
            moves.imported.push(false);
        }
        moves
    }

    pub fn is_empty(&self) -> bool {
        self.applied.is_empty()
    }

    /// The targets of moved file `name`, none when it is not one.
    pub fn targets(&self, name: &str) -> &[String] {
        self.by_old
            .get(name)
            .map_or(&[], |&k| self.applied[k].targets.as_slice())
    }

    /// Whether `name` is a moved file, pruned (spec 0445 S5).
    pub fn is_old(&self, name: &str) -> bool {
        self.by_old.contains_key(name)
    }

    /// `imports` with every moved file replaced by its targets, an entry
    /// this duplicates removed (spec 0445 S4): what import discovery reads.
    pub fn rewrite_names(&mut self, imports: Vec<String>) -> Vec<String> {
        if self.is_empty() || !imports.iter().any(|d| self.is_old(d)) {
            return imports;
        }
        let mut out: Vec<String> = Vec::new();
        for d in imports {
            match self.by_old.get(&d) {
                Some(&k) => {
                    self.imported[k] = true;
                    for t in &self.applied[k].targets {
                        if !out.contains(t) {
                            out.push(t.clone());
                        }
                    }
                }
                None if !out.contains(&d) => out.push(d),
                None => {}
            }
        }
        out
    }

    /// The file's imports rewritten as `rewrite_names` does, its public,
    /// weak and option imports with them: a public import of `OLD` gives
    /// public imports of the targets, a weak one weak ones (spec 0445 S4).
    /// Recorded as re-pointed when anything changed.
    pub fn rewrite_fdp(&mut self, fdp: &mut DynamicMessage) {
        if self.is_empty() {
            return;
        }
        let f = FileDescriptorProto(fdp.as_ref());
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let deps: Vec<String> = f.dependency().iter().map(|d| text(d)).collect();
        let options: Vec<String> = f.option_dependency().iter().map(|d| text(d)).collect();
        if !deps.iter().chain(&options).any(|d| self.is_old(d)) {
            return;
        }
        let name = text(f.name());
        let public: BTreeSet<i32> = f.public_dependency().into_iter().collect();
        let weak: BTreeSet<i32> = f.weak_dependency().into_iter().collect();
        let mut olds = BTreeSet::new();
        let mut after: Vec<String> = Vec::new();
        let (mut new_public, mut new_weak) = (BTreeSet::new(), BTreeSet::new());
        for (n, d) in deps.iter().enumerate() {
            let replaced: Vec<String> = match self.by_old.get(d) {
                Some(&k) => {
                    olds.insert(d.clone());
                    self.applied[k].targets.clone()
                }
                None => vec![d.clone()],
            };
            for r in replaced {
                let at = after.iter().position(|a| *a == r).unwrap_or_else(|| {
                    after.push(r);
                    after.len() - 1
                });
                let at = at as i32;
                if public.contains(&(n as i32)) {
                    new_public.insert(at);
                }
                if weak.contains(&(n as i32)) {
                    new_weak.insert(at);
                }
            }
        }
        let mut new_options: Vec<String> = Vec::new();
        for d in &options {
            let replaced = match self.by_old.get(d) {
                Some(&k) => {
                    olds.insert(d.clone());
                    self.applied[k].targets.clone()
                }
                None => vec![d.clone()],
            };
            for r in replaced {
                if !new_options.contains(&r) {
                    new_options.push(r);
                }
            }
        }
        {
            let mut w = FileDescriptorProtoMut(fdp.as_mut());
            w.clear_dependency();
            for d in &after {
                let _ = w.push_dependency(d.as_bytes());
            }
            w.clear_public_dependency();
            for p in &new_public {
                let _ = w.push_public_dependency(*p);
            }
            w.clear_weak_dependency();
            for p in &new_weak {
                let _ = w.push_weak_dependency(*p);
            }
            w.clear_option_dependency();
            for d in &new_options {
                let _ = w.push_option_dependency(d.as_bytes());
            }
        }
        let olds_list: Vec<String> = olds.iter().cloned().collect();
        crate::report::moved(&name, &olds_list, &deps, &after);
        let entry = self.repointed.entry(name).or_default();
        entry.before = deps;
        entry.after = after;
        entry.olds = olds;
    }

    /// After phase 1: W25 for a move nothing imported and no input holds,
    /// W26 for a target the input lacks (spec 0445 S7). `held` says whether
    /// the input holds a file of that name.
    pub fn check_loaded(&self, held: impl Fn(&str) -> bool, events: &mut Events) {
        for (k, m) in self.applied.iter().enumerate() {
            if !self.imported[k] {
                if !held(&m.old) {
                    not_applied(
                        events,
                        "W25",
                        m,
                        &format!(
                            "move '{}': nothing imports it, and no input holds it; \
                             the directive is idle",
                            m.old
                        ),
                    );
                }
                continue;
            }
            for t in m.targets.iter().filter(|t| !held(t)) {
                not_applied(
                    events,
                    "W26",
                    m,
                    &format!("move '{}': its target '{t}' is not in the input", m.old),
                );
            }
        }
    }

    /// W24 for a re-pointed importer (spec 0445 S6): each type it uses, and
    /// each option extension it sets, that no file it sees defines. What it
    /// sees: itself, its imports, and what they pass on through `import
    /// public`, followed; its imports already in `db`, which phase 2 fills
    /// in dependency order.
    ///
    /// An option is custom when its number is no field of its options type
    /// in `descriptor` (the variant's descriptor.proto, in `db`): the file
    /// that declared it is the moved one, pruned, so `db` cannot say.
    pub fn check_importer(
        &self,
        fdp: &DynamicMessage,
        db: &Database,
        descriptor: &str,
        events: &mut Events,
    ) {
        let f = FileDescriptorProto(fdp.as_ref());
        let name = String::from_utf8_lossy(f.name()).into_owned();
        let Some(r) = self.repointed.get(&name) else {
            return;
        };
        let visible = visible_files(&f, db);
        let mut symbols: BTreeSet<String> = file_symbols(&f).into_iter().collect();
        let mut extensions: BTreeSet<(String, i32)> = extension_keys(&f).into_iter().collect();
        for v in &visible {
            if let Some(d) = db.file_by_name(v) {
                let d = FileDescriptorProto(d.as_ref());
                symbols.extend(file_symbols(&d));
                extensions.extend(extension_keys(&d));
            }
        }
        let moved: Vec<&String> = r.olds.iter().collect();
        let olds: Vec<String> = r.olds.iter().map(|o| format!("'{o}'")).collect();
        let cause = if olds.len() == 1 {
            format!("lost to move {}", olds[0])
        } else {
            format!("lost to the moves of {}", olds.join(", "))
        };
        let mut said = BTreeSet::new();
        for t in type_references(&f) {
            if !symbols.contains(t.trim_start_matches('.')) && said.insert(t.clone()) {
                events.with_fields(serde_json::json!({
                    "importer": name, "definition": t, "move": moved,
                }));
                events.warn(
                    "W24",
                    &format!("'{name}': type '{t}' is defined by no file it sees; {cause}"),
                );
            }
        }
        // Without the variant's descriptor.proto, built-in fields cannot be
        // told from custom options: none is checked.
        let Some(descriptor) = db.file_by_name(descriptor) else {
            return;
        };
        let descriptor = FileDescriptorProto(descriptor.as_ref());
        let package = String::from_utf8_lossy(descriptor.package()).into_owned();
        let Ok(bytes) = fdp.encode() else { return };
        for (options, number) in option_numbers(&bytes) {
            let builtin = descriptor
                .message_type()
                .iter()
                .find(|m| m.name() == options.as_bytes())
                .is_some_and(|m| m.field().iter().any(|x| x.number() == number));
            let simple = |e: &str| e.rsplit('.').next() == Some(options);
            if builtin || extensions.iter().any(|(e, n)| *n == number && simple(e)) {
                continue;
            }
            let extendee = format!("{package}.{options}");
            if said.insert(format!("{extendee}/{number}")) {
                events.with_fields(serde_json::json!({
                    "importer": name,
                    "definition": { "options": extendee, "number": number },
                    "move": moved,
                }));
                events.warn(
                    "W24",
                    &format!(
                        "'{name}': option {number} of {extendee} is declared by no file it \
                         sees; {cause}"
                    ),
                );
            }
        }
    }
}

/// The files `f` sees: its imports, and what they pass on through
/// `import public`, followed.
fn visible_files(f: &FileDescriptorProto<'_>, db: &Database) -> BTreeSet<String> {
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut todo: Vec<String> = f.dependency().iter().map(|d| text(d)).collect();
    while let Some(d) = todo.pop() {
        if !seen.insert(d.clone()) {
            continue;
        }
        if let Some(dep) = db.file_by_name(&d) {
            let dep = FileDescriptorProto(dep.as_ref());
            let deps = dep.dependency();
            for p in dep.public_dependency() {
                if let Some(name) = usize::try_from(p).ok().and_then(|p| deps.get(p)) {
                    todo.push(text(name));
                }
            }
        }
    }
    seen
}

/// Every type the file names, fully qualified as protoc writes them: a
/// field's `type_name`, an extension's `extendee`, a method's input and
/// output types.
fn type_references(f: &FileDescriptorProto<'_>) -> Vec<String> {
    fn field(x: &FieldDescriptorProto<'_>, out: &mut Vec<String>) {
        for t in [x.type_name(), x.extendee()] {
            if !t.is_empty() {
                out.push(String::from_utf8_lossy(t).into_owned());
            }
        }
    }
    fn message(m: &DescriptorProto<'_>, out: &mut Vec<String>) {
        for x in m.field().iter().chain(&m.extension()) {
            field(x, out);
        }
        for n in m.nested_type() {
            message(&n, out);
        }
    }
    let mut out = Vec::new();
    for x in f.extension() {
        field(&x, &mut out);
    }
    for m in f.message_type() {
        message(&m, &mut out);
    }
    for s in f.service() {
        for m in s.method() {
            for t in [m.input_type(), m.output_type()] {
                if !t.is_empty() {
                    out.push(String::from_utf8_lossy(t).into_owned());
                }
            }
        }
    }
    out
}

/// Every field number set in an options message of the encoded file, with
/// its options type's simple name: what a custom option is set by.
pub fn option_numbers(fdp: &[u8]) -> Vec<(&'static str, i32)> {
    // descriptor.proto's layout: (container's field number, what it holds).
    #[derive(Clone, Copy)]
    enum Kind {
        File,
        Message,
        Field,
        Oneof,
        Enum,
        Value,
        Service,
        Method,
        Range,
    }
    fn children(k: Kind) -> &'static [(u32, Kind)] {
        use Kind::*;
        match k {
            File => &[(4, Message), (5, Enum), (6, Service), (7, Field)],
            Message => &[
                (2, Field),
                (3, Message),
                (4, Enum),
                (5, Range),
                (6, Field),
                (8, Oneof),
            ],
            Enum => &[(2, Value)],
            Service => &[(2, Method)],
            Field | Oneof | Value | Method | Range => &[],
        }
    }
    fn options(k: Kind) -> (u32, &'static str) {
        use Kind::*;
        match k {
            File => (8, "FileOptions"),
            Message => (7, "MessageOptions"),
            Field => (8, "FieldOptions"),
            Oneof => (2, "OneofOptions"),
            Enum => (3, "EnumOptions"),
            Value => (3, "EnumValueOptions"),
            Service => (3, "ServiceOptions"),
            Method => (4, "MethodOptions"),
            Range => (3, "ExtensionRangeOptions"),
        }
    }
    fn walk(bytes: &[u8], k: Kind, out: &mut Vec<(&'static str, i32)>) {
        let (opt_field, opt_name) = options(k);
        for (number, payload) in fields(bytes) {
            let Some(payload) = payload else { continue };
            if number == opt_field {
                for (n, _) in fields(payload) {
                    out.push((opt_name, n as i32));
                }
            } else if let Some(&(_, child)) = children(k).iter().find(|(f, _)| *f == number) {
                walk(payload, child, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(fdp, Kind::File, &mut out);
    out
}

/// The top-level fields of an encoded message: each number, with its
/// payload when length-delimited. Stops at the first malformed byte.
fn fields(mut b: &[u8]) -> Vec<(u32, Option<&[u8]>)> {
    fn varint(b: &mut &[u8]) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = b.split_first()?;
            *b = rest;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    let mut out = Vec::new();
    while !b.is_empty() {
        let Some(key) = varint(&mut b) else { break };
        let number = (key >> 3) as u32;
        match key & 7 {
            0 => {
                if varint(&mut b).is_none() {
                    break;
                }
                out.push((number, None));
            }
            1 | 5 => {
                let len = if key & 7 == 1 { 8 } else { 4 };
                if b.len() < len {
                    break;
                }
                b = &b[len..];
                out.push((number, None));
            }
            2 => {
                let Some(len) = varint(&mut b).and_then(|l| usize::try_from(l).ok()) else {
                    break;
                };
                if b.len() < len {
                    break;
                }
                out.push((number, Some(&b[..len])));
                b = &b[len..];
            }
            // A group (3) is only in a custom option's value, which is not
            // walked; its start, at the top level of an options message,
            // still says the option is set.
            3 => {
                out.push((number, None));
                let mut depth = 1;
                while depth > 0 && !b.is_empty() {
                    let Some(k) = varint(&mut b) else { return out };
                    match k & 7 {
                        0 => {
                            varint(&mut b);
                        }
                        1 => b = b.get(8..).unwrap_or_default(),
                        5 => b = b.get(4..).unwrap_or_default(),
                        2 => {
                            let l = varint(&mut b).and_then(|l| usize::try_from(l).ok());
                            b = l.and_then(|l| b.get(l..)).unwrap_or_default();
                        }
                        3 => depth += 1,
                        _ => depth -= 1,
                    }
                }
            }
            _ => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fdp(text: &str) -> DynamicMessage {
        let bytes = prototext_upb::text_format::parse_descriptor(text, false).unwrap();
        let def = prototext_upb::Pool::vendored()
            .get_message_by_name("google.protobuf.FileDescriptorProto")
            .unwrap();
        DynamicMessage::decode(&def, &bytes).unwrap()
    }

    fn deps(m: &DynamicMessage) -> (Vec<String>, Vec<i32>, Vec<i32>) {
        let f = FileDescriptorProto(m.as_ref());
        (
            f.dependency()
                .iter()
                .map(|d| String::from_utf8_lossy(d).into_owned())
                .collect(),
            f.public_dependency(),
            f.weak_dependency(),
        )
    }

    fn moves(given: &[(&str, &[&str])]) -> (Moves, Events) {
        let mut events = Events::default();
        let given = given
            .iter()
            .map(|(o, t)| Move {
                old: o.to_string(),
                targets: t.iter().map(|t| t.to_string()).collect(),
            })
            .collect();
        (Moves::new(given, &[], &mut events), events)
    }

    #[test]
    fn imports_rewritten_public_and_weak_kept() {
        let (mut m, _) = moves(&[("old.proto", &["n1.proto", "n2.proto"])]);
        let mut f = fdp(
            r#"name: "b.proto" dependency: "x.proto" dependency: "old.proto"
               dependency: "n2.proto" public_dependency: 1 weak_dependency: 0"#,
        );
        m.rewrite_fdp(&mut f);
        // old (public) gives n1, n2 public; n2 given again, merged.
        assert_eq!(
            deps(&f),
            (
                vec!["x.proto".into(), "n1.proto".into(), "n2.proto".into()],
                vec![1, 2],
                vec![0]
            )
        );
        let r = &m.repointed["b.proto"];
        assert_eq!(r.before, ["x.proto", "old.proto", "n2.proto"]);
        assert!(r.olds.contains("old.proto"));
    }

    #[test]
    fn untouched_without_a_moved_import() {
        let (mut m, _) = moves(&[("old.proto", &["new.proto"])]);
        let mut f = fdp(r#"name: "b.proto" dependency: "x.proto""#);
        m.rewrite_fdp(&mut f);
        assert!(m.repointed.is_empty());
        assert_eq!(deps(&f).0, ["x.proto"]);
    }

    #[test]
    fn names_rewritten_and_counted() {
        let (mut m, _) = moves(&[("old.proto", &["new.proto"])]);
        let out = m.rewrite_names(vec!["new.proto".into(), "old.proto".into()]);
        assert_eq!(out, ["new.proto"]);
        assert!(m.imported[0]);
    }

    #[test]
    fn chains_twice_and_pruned_targets_are_w26() {
        let mut events = Events::default();
        let m = |o: &str, t: &str| Move {
            old: o.into(),
            targets: vec![t.into()],
        };
        let given = vec![
            m("a.proto", "b.proto"),
            m("b.proto", "c.proto"),
            m("a.proto", "d.proto"),
            m("e.proto", "gone.proto"),
        ];
        let moves = Moves::new(given, &["file:gone.proto".into()], &mut events);
        // a -> b is a chain; b -> c applies; a given twice; e's target pruned.
        assert_eq!(moves.applied.len(), 1);
        assert_eq!(moves.applied[0].old, "b.proto");
        assert_eq!(events.count_of("W26"), 3);
    }

    #[test]
    fn option_numbers_found_at_every_level() {
        let f = fdp(r#"name: "o.proto"
               options { java_package: "x" }
               message_type { name: "M" options { deprecated: true }
                 field { name: "f" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32
                         options { packed: true } } }
               service { name: "S" method { name: "m" input_type: ".M" output_type: ".M"
                         options { deprecated: true } } }"#);
        let got = option_numbers(&f.encode().unwrap());
        assert!(got.contains(&("FileOptions", 1)));
        assert!(got.contains(&("MessageOptions", 3)));
        assert!(got.contains(&("FieldOptions", 2)));
        assert!(got.contains(&("MethodOptions", 33)));
    }

    #[test]
    fn cli_value_parsed() {
        assert_eq!(
            parse_cli("a.proto=b.proto,c.proto").unwrap().targets,
            ["b.proto", "c.proto"]
        );
        assert!(parse_cli("a.proto").is_err());
        assert!(parse_cli("a.proto=b.proto,").is_err());
    }
}
