// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The scoring graphs (spec 0430 S1, S2): each file's, as reproto's
//! `_collect_scoring_messages` collects it from `ctx.pool`.
//!
//! The definitions are read from the upb pool `pool_loads` filled in
//! reproto's order, so the kinds, labels, packing and closed enums are
//! upb's, as Python's upb resolves them. The pool holds every file reproto's
//! ever does; a message's extensions are those of the files loaded (spec
//! 0430 S1, 0429 S14): every summoned file and its imports, whatever their
//! order.

pub mod html;
pub mod pyyaml;

use crate::hash::{HashMap, HashSet};

use prototext_graph::build_scoring_graph::{
    FieldLabel, FieldSpec, FieldType, NodeKind, ScoringFile, ScoringMessage,
};
use prototext_upb::{Cardinality, Field, Kind, Message};

use crate::graph::Graph;
use crate::render::PoolLoads;

/// What one collection reads.
pub struct Collect<'a> {
    pub graph: &'a Graph,
    pub loads: &'a PoolLoads,
    /// `--emit-extension-ranges`.
    pub extension_ranges: bool,
    /// The names written (`rename`): the identity for the YAML, the
    /// variant's namespace rules for the schema database.
    pub rename: &'a (dyn Fn(&str) -> String + Sync),
}

/// A document's messages, as Python's `dict` keeps them: a key set again
/// keeps its place.
#[derive(Default)]
struct Messages {
    list: Vec<(String, ScoringMessage)>,
    index: HashMap<String, usize>,
}

impl Messages {
    fn insert(&mut self, name: String, m: ScoringMessage) {
        match self.index.get(&name) {
            Some(&i) => self.list[i].1 = m,
            None => {
                self.index.insert(name.clone(), self.list.len());
                self.list.push((name, m));
            }
        }
    }
}

/// File `name`'s scoring graph; `None` when the pool refused it (reproto's
/// W6, step 9).
pub fn file(c: &Collect<'_>, name: &str) -> Option<ScoringFile> {
    let fd = c.loads.pool.file(name)?;
    let mut groups = HashSet::default();
    for m in fd.messages() {
        group_fqdns(&m, &mut groups);
    }
    let mut messages = Messages::default();
    let mut entries = Vec::new();
    for m in fd.messages() {
        message(c, &m, &mut messages, &groups, &mut entries);
    }
    entries.sort();
    Some(ScoringFile {
        extension_ranges: c.extension_ranges,
        entries,
        messages: messages.list,
    })
}

/// `_collect_group_fqdns`: the types of the file's group fields
/// (`TYPE_GROUP`, an editions field's delimited encoding included).
fn group_fqdns(m: &Message, out: &mut HashSet<String>) {
    for f in m.fields() {
        if f.is_group() {
            if let Kind::Message(t) = f.kind() {
                out.insert(t.full_name().to_string());
            }
        }
    }
    for n in m.child_messages() {
        group_fqdns(&n, out);
    }
}

fn pruned(graph: &Graph, fqdn: &str) -> bool {
    graph.find(fqdn).is_some_and(|i| graph.nodes[i].is_pruned)
}

/// `_collect_scoring_messages`: `m` and its nested types, unless pruned.
fn message(
    c: &Collect<'_>,
    m: &Message,
    messages: &mut Messages,
    groups: &HashSet<String>,
    entries: &mut Vec<String>,
) {
    let full = m.full_name();
    if pruned(c.graph, &format!("desc:.{full}")) {
        return;
    }
    // A MessageSet's extensions are not its fields on the wire: they come
    // wrapped in its `Item` group.
    let message_set = m.is_message_set_wire_format();
    let mut known: Vec<Field> = m.fields().collect();
    if !message_set {
        known.extend(
            m.all_extensions()
                .into_iter()
                .filter(|e| c.loads.loaded.contains(e.parent_file().name())),
        );
    }
    known.sort_by_key(Field::number);
    let mut fields = Vec::new();
    for f in &known {
        if pruned(c.graph, &format!("fdsc:.{}", f.full_name())) {
            continue;
        }
        fields.push(field(c, f));
    }
    let name = (c.rename)(full);
    // Before the message itself: the document's order is the dict's.
    if message_set && fields.is_empty() {
        let item = format!("{name}.Item");
        let leaf = |number, ty| FieldSpec {
            number,
            ty,
            child: None,
            range: None,
            label: FieldLabel::Optional,
            packed: false,
        };
        messages.insert(
            item.clone(),
            ScoringMessage {
                kind: NodeKind::Group,
                fields: vec![leaf(2, FieldType::Int32), leaf(3, FieldType::Bytes)],
                ext_ranges: Vec::new(),
            },
        );
        fields.push(FieldSpec {
            number: 1,
            ty: FieldType::Message,
            child: Some(item),
            range: None,
            label: FieldLabel::Repeated,
            packed: false,
        });
    }
    let ext_ranges = if c.extension_ranges {
        canonical_extension_ranges(m)
    } else {
        Vec::new()
    };
    messages.insert(
        name.clone(),
        ScoringMessage {
            kind: if groups.contains(full) {
                NodeKind::Group
            } else {
                NodeKind::LenDel
            },
            fields,
            ext_ranges,
        },
    );
    entries.push(name);
    for n in m.child_messages() {
        message(c, &n, messages, groups, entries);
    }
}

/// `_scoring_kind`, `_field_label` and `fd_is_packed`: one field's entry.
fn field(c: &Collect<'_>, f: &Field) -> FieldSpec {
    let mut child = None;
    let mut range = None;
    let ty = match f.kind() {
        Kind::Message(t) => {
            child = Some((c.rename)(t.full_name()));
            if f.is_group() {
                FieldType::Group
            } else {
                FieldType::Message
            }
        }
        Kind::String => FieldType::String,
        Kind::Bytes => FieldType::Bytes,
        Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => FieldType::Double,
        Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => FieldType::Float,
        Kind::Bool => {
            range = Some((0, 1));
            FieldType::Bool
        }
        // An open enum takes any 32-bit value (spec 0176).
        Kind::Enum(e) if !e.is_closed() => FieldType::Int32,
        Kind::Enum(e) => {
            let numbers: Vec<i32> = e.values().map(|v| v.number()).collect();
            let min = numbers.iter().copied().min().unwrap_or(0);
            let max = numbers.iter().copied().max().unwrap_or(0);
            range = Some((min, max));
            FieldType::Enum
        }
        Kind::Int32 => FieldType::Int32,
        Kind::Uint32 | Kind::Sint32 => FieldType::Uint32,
        Kind::Int64 | Kind::Uint64 | Kind::Sint64 => FieldType::Uint64,
    };
    let label = match f.cardinality() {
        Cardinality::Required => FieldLabel::Required,
        Cardinality::Repeated => FieldLabel::Repeated,
        Cardinality::Optional => FieldLabel::Optional,
    };
    // Declared packed: repeated, of a packable type, packed by its syntax
    // or its resolved encoding (spec 0371 S1).
    let packable = !matches!(f.kind(), Kind::Message(_) | Kind::String | Kind::Bytes);
    let packed = label == FieldLabel::Repeated && packable && f.is_packed();
    FieldSpec {
        number: f.number(),
        ty,
        child,
        range,
        label,
        packed,
    }
}

/// `_canonical_extension_ranges` (spec 0238 S3): the half-open ranges as
/// inclusive, sorted, with overlapping and adjacent ones merged.
fn canonical_extension_ranges(m: &Message) -> Vec<(u32, u32)> {
    let mut spans: Vec<(i64, i64)> = m
        .extension_ranges()
        .iter()
        .filter(|r| r.end > r.start)
        .map(|r| (i64::from(r.start), i64::from(r.end) - 1))
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
        .into_iter()
        .map(|(s, e)| (s as u32, e as u32))
        .collect()
}
