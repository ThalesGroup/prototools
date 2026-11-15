// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Phase 3: the node graph (reproto's `_phase3_build_graph` and the
//! `_initialize_from_message` of each `Re*` class).
//!
//! One arena of nodes, in the order reproto registers them: a node takes
//! its place when it is first named, a stub for an unresolved reference
//! included, and keeps it when its definition comes later. Each node is
//! initialized once: a definition met again (two files, under
//! `--keep-duplicates`) keeps the first.

use std::collections::{BTreeSet, HashMap};

use prototext_upb::descriptor_access::{
    DescriptorProto, FeatureSet, FieldDescriptorProto, FileDescriptorProto,
};

use crate::db::Database;
use crate::phase2::{EditionDefault, EditionDefaults};
use crate::topology::{Stripped, Topology};

/// A node's class, by reproto's name for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Message,
    Field,
    Enum,
    Service,
    Method,
}

impl Kind {
    pub fn class_name(self) -> &'static str {
        match self {
            Kind::File => "ReFileDescriptorProto",
            Kind::Message => "ReDescriptorProto",
            Kind::Field => "ReFieldDescriptorProto",
            Kind::Enum => "ReEnumDescriptorProto",
            Kind::Service => "ReServiceDescriptorProto",
            Kind::Method => "ReMethodDescriptorProto",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Message => "desc",
            Kind::Field => "fdsc",
            Kind::Enum => "enum",
            Kind::Service => "serv",
            Kind::Method => "meth",
        }
    }
}

/// Where a node is defined: a file of the database, and the path to the
/// definition in its `FileDescriptorProto`, as (field number, index) steps
/// (`message_type` 4, `enum_type` 5, `service` 6, `extension` 7; in a
/// message, `field` 2, `nested_type` 3, `enum_type` 4, `extension` 6; in a
/// service, `method` 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefRef {
    pub file: usize,
    pub path: Vec<(u32, usize)>,
}

impl DefRef {
    fn then(&self, number: u32, index: usize) -> DefRef {
        let mut path = self.path.clone();
        path.push((number, index));
        DefRef {
            file: self.file,
            path,
        }
    }
}

/// A message's definition, and the messages enclosing it, outermost first:
/// what `_message_chain` gives, and the file it is in.
struct MessageDef<'a> {
    file: FileDescriptorProto<'a>,
    message: DescriptorProto<'a>,
    enclosing: Vec<DescriptorProto<'a>>,
}

/// The message at `def`, a path of `message_type` and `nested_type` steps.
fn message_def<'a>(db: &'a Database, def: &DefRef) -> Option<MessageDef<'a>> {
    let file = FileDescriptorProto(db.files.get(def.file)?.as_ref());
    let mut steps = def.path.iter();
    let &(4, first) = steps.next()? else {
        return None;
    };
    let mut message = file.message_type().into_iter().nth(first)?;
    let mut enclosing = Vec::new();
    for &(number, index) in steps {
        if number != 3 {
            return None;
        }
        let next = message.nested_type().into_iter().nth(index)?;
        enclosing.push(message);
        message = next;
    }
    Some(MessageDef {
        file,
        message,
        enclosing,
    })
}

/// A field's resolved type: a node, or a scalar's `FieldDescriptorProto.Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeRef {
    Node(usize),
    Scalar(i32),
}

#[derive(Debug)]
pub struct Node {
    pub kind: Kind,
    pub fqdn: String,
    /// Initialized from a definition; a stub otherwise.
    pub present: bool,
    pub is_pruned: bool,
    pub is_reachable: bool,
    pub is_summoned: bool,
    pub parent: Option<usize>,
    pub contains: BTreeSet<usize>,
    pub targets: BTreeSet<usize>,
    /// The scope its children's names are under (`.pkg.Msg`).
    pub prefix: String,
    /// Its definition, once initialized from one.
    pub def: Option<DefRef>,
    pub is_group: bool,
    pub type_descriptor: Option<TypeRef>,
    pub editions_group_name: Option<String>,
    pub stripped_fields: Vec<Stripped>,
    pub stripped_methods: Vec<Stripped>,
}

#[derive(Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    by_fqdn: HashMap<String, usize>,
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `MESSAGE_ENCODING_DELIMITED`.
const DELIMITED: i32 = 2;
const TYPE_GROUP: i32 = 10;
const TYPE_MESSAGE: i32 = 11;
const TYPE_ENUM: i32 = 14;

/// What a field's message encoding resolves to (`resolve_features`, the
/// one feature phase 3 reads): the default of the latest edition at or
/// before the file's, then each feature set, coarsest to finest, that sets
/// it.
fn message_encoding(table: &EditionDefaults, edition: i32, sets: &[Option<FeatureSet<'_>>]) -> i32 {
    let defaults: &[(i32, String)] = match table.get("message_encoding") {
        Some(EditionDefault::Defaults(d)) => d,
        _ => &[],
    };
    let numbers = match table.get("_enum_message_encoding") {
        Some(EditionDefault::Numbers(n)) => Some(n),
        _ => None,
    };
    let mut result = defaults
        .iter()
        .rev()
        .find(|(e, _)| *e <= edition)
        .and_then(|(_, name)| numbers.and_then(|n| n.get(name)).copied())
        .unwrap_or(0);
    for fs in sets.iter().flatten() {
        if fs.has_message_encoding() {
            result = fs.message_encoding();
        }
    }
    result
}

/// What phase 3 reads of the run.
pub struct Build<'a> {
    pub graph: &'a mut Graph,
    pub db: &'a Database,
    pub force_proto2_output: bool,
    pub edition_defaults: &'a EditionDefaults,
}

impl Graph {
    pub fn find(&self, fqdn: &str) -> Option<usize> {
        self.by_fqdn.get(fqdn).copied()
    }

    /// `ReX.from_ref` and `ReX.__new__`: the node of that FQDN, created as
    /// a stub when there is none.
    fn node(&mut self, kind: Kind, fqdn: String) -> usize {
        if let Some(&i) = self.by_fqdn.get(&fqdn) {
            return i;
        }
        let i = self.nodes.len();
        self.by_fqdn.insert(fqdn.clone(), i);
        self.nodes.push(Node {
            kind,
            fqdn,
            present: false,
            is_pruned: false,
            is_reachable: false,
            is_summoned: false,
            parent: None,
            contains: BTreeSet::new(),
            targets: BTreeSet::new(),
            prefix: String::new(),
            def: None,
            is_group: false,
            type_descriptor: None,
            editions_group_name: None,
            stripped_fields: Vec::new(),
            stripped_methods: Vec::new(),
        });
        i
    }

    fn reference(&mut self, kind: Kind, r: &str) -> usize {
        self.node(kind, format!("{}:{r}", kind.prefix()))
    }

    fn link(&mut self, from: usize, to: usize, contained: bool) {
        self.nodes[from].targets.insert(to);
        if contained {
            self.nodes[from].contains.insert(to);
        }
    }
}

impl Build<'_> {
    /// A child's node, under its parent's prefix, `None` when it was
    /// initialized already (its definition met before).
    fn child(&mut self, kind: Kind, parent: usize, name: &str, def: DefRef) -> (usize, bool) {
        let fqdn = format!(
            "{}:{}.{name}",
            kind.prefix(),
            self.graph.nodes[parent].prefix
        );
        let i = self.graph.node(kind, fqdn);
        let fresh = !self.graph.nodes[i].present;
        if fresh {
            let prefix = format!("{}.{name}", self.graph.nodes[parent].prefix);
            let n = &mut self.graph.nodes[i];
            n.present = true;
            n.parent = Some(parent);
            n.prefix = prefix;
            n.def = Some(def);
        }
        (i, fresh)
    }

    /// A file's node and everything it defines (`ReFileDescriptorProto`),
    /// file `index` of the database.
    pub fn file(&mut self, index: usize) {
        let db = self.db;
        let fdp = FileDescriptorProto(db.files[index].as_ref());
        let i = self.graph.reference(Kind::File, &text(fdp.name()));
        if self.graph.nodes[i].present {
            return;
        }
        let package = text(fdp.package());
        let here = DefRef {
            file: index,
            path: Vec::new(),
        };
        {
            let n = &mut self.graph.nodes[i];
            n.present = true;
            n.prefix = if package.is_empty() {
                String::new()
            } else {
                format!(".{package}")
            };
            n.def = Some(here.clone());
        }
        for dep in fdp.dependency() {
            let d = self.graph.reference(Kind::File, &text(dep));
            self.graph.link(i, d, false);
        }
        for (si, s) in fdp.service().iter().enumerate() {
            let sdef = here.then(6, si);
            let (svc, fresh) = self.child(Kind::Service, i, &text(s.name()), sdef.clone());
            if fresh {
                for (mi, m) in s.method().iter().enumerate() {
                    let (meth, _) =
                        self.child(Kind::Method, svc, &text(m.name()), sdef.then(2, mi));
                    self.graph.link(svc, meth, false);
                }
            }
            self.graph.link(i, svc, true);
        }
        for (ei, e) in fdp.enum_type().iter().enumerate() {
            let (en, _) = self.child(Kind::Enum, i, &text(e.name()), here.then(5, ei));
            self.graph.link(i, en, true);
        }
        for (mi, m) in fdp.message_type().iter().enumerate() {
            let msg = self.message(m, i, &fdp, &[], here.then(4, mi));
            self.graph.link(i, msg, true);
        }
        for (xi, x) in fdp.extension().iter().enumerate() {
            self.extension(x, i, here.then(7, xi));
        }
    }

    /// An extension: its field node, contained by `owner`, and its
    /// extendee, a target of the field.
    fn extension(&mut self, x: &FieldDescriptorProto<'_>, owner: usize, def: DefRef) {
        let ext = self.field(x, owner, def);
        self.graph.link(owner, ext, true);
        let extendee = self.graph.reference(Kind::Message, &text(x.extendee()));
        self.graph.link(ext, extendee, false);
    }

    /// A field's node (`ReFieldDescriptorProto`): its type a target, a stub
    /// when unresolved.
    fn field(&mut self, f: &FieldDescriptorProto<'_>, parent: usize, def: DefRef) -> usize {
        let (i, fresh) = self.child(Kind::Field, parent, &text(f.name()), def);
        if !fresh {
            return i;
        }
        let type_name = text(f.type_name());
        let ty = f.r#type();
        let resolved = match ty {
            TYPE_GROUP | TYPE_MESSAGE => {
                let t = self.graph.reference(Kind::Message, &type_name);
                if ty == TYPE_GROUP {
                    // `allow_groups`: phase 3 runs before any file sets the
                    // target syntax, still proto2's.
                    self.graph.nodes[t].is_group = true;
                }
                self.graph.link(i, t, false);
                TypeRef::Node(t)
            }
            TYPE_ENUM => {
                let t = self.graph.reference(Kind::Enum, &type_name);
                self.graph.link(i, t, false);
                TypeRef::Node(t)
            }
            other => TypeRef::Scalar(other),
        };
        self.graph.nodes[i].type_descriptor = Some(resolved);
        i
    }

    /// A message's node and its children (`ReDescriptorProto`), defined
    /// in `file` inside `enclosing`, at `def`.
    fn message(
        &mut self,
        m: &DescriptorProto<'_>,
        parent: usize,
        file: &FileDescriptorProto<'_>,
        enclosing: &[DescriptorProto<'_>],
        def: DefRef,
    ) -> usize {
        let (i, fresh) = self.child(Kind::Message, parent, &text(m.name()), def.clone());
        if fresh {
            self.message_body(m, i, file, enclosing, &def);
        }
        i
    }

    /// What `ReDescriptorProto._initialize_from_message` makes of `m` under
    /// node `i`: extensions, enums, nested messages, synthetic groups,
    /// fields. Children's definitions are under `def`.
    fn message_body(
        &mut self,
        m: &DescriptorProto<'_>,
        i: usize,
        file: &FileDescriptorProto<'_>,
        enclosing: &[DescriptorProto<'_>],
        def: &DefRef,
    ) {
        for (xi, x) in m.extension().iter().enumerate() {
            self.extension(x, i, def.then(6, xi));
        }
        for (ei, e) in m.enum_type().iter().enumerate() {
            let (en, _) = self.child(Kind::Enum, i, &text(e.name()), def.then(4, ei));
            self.graph.link(i, en, false);
        }
        // `_message_chain`: the enclosing messages, outermost first, `m` last.
        let mut chain: Vec<DescriptorProto<'_>> = enclosing.to_vec();
        chain.push(m.clone());
        for (ni, n) in m.nested_type().iter().enumerate() {
            let nested = self.message(n, i, file, &chain, def.then(3, ni));
            self.graph.link(i, nested, false);
        }
        let groups = if file.has_edition() && self.force_proto2_output {
            self.delimited_groups(m, i, file, &chain)
        } else {
            Vec::new()
        };
        for (fi, f) in m.field().iter().enumerate() {
            let fd = self.field(f, i, def.then(2, fi));
            if let Some((_, grp, name)) = groups.iter().find(|(n, _, _)| *n == f.number()) {
                self.graph.nodes[fd].editions_group_name = Some(name.clone());
                self.graph.link(fd, *grp, false);
            }
            self.graph.link(i, fd, true);
        }
    }

    /// Editions DELIMITED fields under `--force-proto2-output`: a synthetic
    /// group for each, named after the field in CamelCase, disambiguated
    /// against the message's nested names, and built, as reproto builds it,
    /// from the definition of the field's message type, wherever that is.
    /// (field number, group node, group name).
    fn delimited_groups(
        &mut self,
        m: &DescriptorProto<'_>,
        i: usize,
        file: &FileDescriptorProto<'_>,
        chain: &[DescriptorProto<'_>],
    ) -> Vec<(i32, usize, String)> {
        let mut occupied: BTreeSet<String> =
            m.nested_type().iter().map(|n| text(n.name())).collect();
        occupied.extend(m.enum_type().iter().map(|e| text(e.name())));
        let file_fs = file.options().and_then(|o| o.features());
        let mut out = Vec::new();
        for f in m.field() {
            if f.r#type() != TYPE_MESSAGE {
                continue;
            }
            let mut sets: Vec<Option<FeatureSet<'_>>> = vec![file_fs.clone()];
            sets.extend(chain.iter().map(|c| c.options().and_then(|o| o.features())));
            sets.push(f.options().and_then(|o| o.features()));
            if message_encoding(self.edition_defaults, file.edition(), &sets) != DELIMITED {
                continue;
            }
            let raw: String = text(f.name())
                .split('_')
                .filter(|p| !p.is_empty())
                .map(capitalize)
                .collect();
            let mut candidate = raw.clone();
            let mut suffix = 2;
            while occupied.contains(&candidate) {
                candidate = format!("{raw}{suffix}");
                suffix += 1;
            }
            occupied.insert(candidate.clone());
            let synthetic = format!("{}.{candidate}", self.graph.nodes[i].prefix);
            let inner = self.graph.reference(Kind::Message, &text(f.type_name()));
            let Some(inner_def) = self.graph.nodes[inner].def.clone() else {
                // reproto reads `inner.this` and fails there: the type is
                // not defined yet. Nothing to build the group from.
                continue;
            };
            let grp = self.graph.reference(Kind::Message, &synthetic);
            let inner_parent = self.graph.nodes[inner].parent;
            {
                let g = &mut self.graph.nodes[grp];
                g.prefix = synthetic.clone();
                g.present = true;
                g.parent = inner_parent;
                g.def = Some(inner_def.clone());
            }
            let db = self.db;
            if let Some(d) = message_def(db, &inner_def) {
                self.message_body(&d.message, grp, &d.file, &d.enclosing, &inner_def);
            }
            self.graph.nodes[grp].is_group = true;
            out.push((f.number(), grp, candidate));
        }
        out
    }
}

/// Python's `str.capitalize`: the first character upper case, the rest
/// lower case.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// Phase 3: a node for every file loaded and not pruned, in topology
/// order, from the database's mended file; then the topology's pruned
/// files and phase 2's stripped records onto their nodes.
pub fn run(
    topo: &Topology,
    db: &Database,
    force_proto2_output: bool,
    edition_defaults: &EditionDefaults,
) -> Graph {
    let mut graph = Graph::default();
    {
        let mut b = Build {
            graph: &mut graph,
            db,
            force_proto2_output,
            edition_defaults,
        };
        for f in &topo.files {
            if f.is_ref() || f.is_pruned {
                continue;
            }
            if let Some(index) = db.index_of(&f.name) {
                b.file(index);
            }
        }
    }
    for f in &topo.files {
        let Some(node) = graph.find(&format!("file:{}", f.name)) else {
            continue;
        };
        if f.is_pruned {
            graph.nodes[node].is_pruned = true;
        }
        for (owner, records) in &f.stripped_field_types {
            if let Some(n) = graph.find(&format!("desc:{owner}")) {
                graph.nodes[n]
                    .stripped_fields
                    .extend(records.iter().cloned());
            }
        }
        for (owner, records) in &f.stripped_method_types {
            if let Some(n) = graph.find(&format!("serv:{owner}")) {
                graph.nodes[n]
                    .stripped_methods
                    .extend(records.iter().cloned());
            }
        }
    }
    graph
}
