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

/// `fd_is_packed`: a field declared packed, repeated, of a packable type,
/// packed by its syntax or its resolved encoding (spec 0371 S1).
fn fd_is_packed(f: &Field) -> bool {
    let packable = !matches!(f.kind(), Kind::Message(_) | Kind::String | Kind::Bytes);
    f.cardinality() == Cardinality::Repeated && packable && f.is_packed()
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
    let packed = fd_is_packed(f);
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
/// inclusive, sorted, with overlapping and adjacent ones merged, and clamped
/// to the largest field number a tag can carry: a message set's `max` is
/// `i32::MAX` (its extensions travel as `type_id` values, not as tags), and
/// the walk never looks a number past `MAX_FIELD_NUMBER` up in a range.
/// 2²⁹−1, the largest field number a tag can carry.
const MAX_FIELD_NUMBER: i64 = 536_870_911;

fn canonical_extension_ranges(m: &Message) -> Vec<(u32, u32)> {
    canonical_ranges(m.extension_ranges().iter().map(|r| (r.start, r.end)))
}

/// The canonicalization itself, on half-open `(start, end)` ranges: apart
/// from the message they are read from, so that its tests can give it
/// ranges no descriptor pool would hold (spec 0488).
fn canonical_ranges(ranges: impl IntoIterator<Item = (i32, i32)>) -> Vec<(u32, u32)> {
    let mut spans: Vec<(i64, i64)> = ranges
        .into_iter()
        .filter(|&(start, end)| end > start)
        .map(|(start, end)| (i64::from(start), (i64::from(end) - 1).min(MAX_FIELD_NUMBER)))
        .filter(|&(start, end)| start <= end)
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

/// The unit tests of `test_emit_extension_ranges.py` (spec 0488 S2), on the
/// canonicalization that ports reproto's `_canonical_extension_ranges`,
/// given the half-open ranges its stand-in descriptor carried: inputs
/// protoc cannot produce, which no descriptor pool would hold.
#[cfg(test)]
mod test_emit_extension_ranges {
    use super::*;

    /// `extensions 10000` arrives as the half-open (10000, 10001).
    #[test]
    fn test_end_is_exclusive() {
        assert_eq!(canonical_ranges([(10000, 10001)]), [(10000, 10000)]);
    }

    #[test]
    fn test_overlapping_ranges_merge() {
        assert_eq!(
            canonical_ranges([(1000, 2001), (1500, 3001)]),
            [(1000, 3000)]
        );
    }

    /// The merge takes the max of the two ends, not the later one.
    #[test]
    fn test_a_contained_range_does_not_shrink_its_container() {
        assert_eq!(
            canonical_ranges([(1000, 5001), (2000, 3001)]),
            [(1000, 5000)]
        );
    }

    #[test]
    fn test_degenerate_ranges_are_dropped() {
        assert_eq!(canonical_ranges([(1000, 1000), (5000, 4000)]), []);
    }

    /// Merging stops at a genuine gap, or distinct sets would collide.
    #[test]
    fn test_disjoint_non_adjacent_ranges_survive_separately() {
        assert_eq!(
            canonical_ranges([(3000, 4001), (1000, 2001)]),
            [(1000, 2000), (3000, 4000)]
        );
    }

    #[test]
    fn test_no_ranges_is_the_empty_list() {
        assert_eq!(canonical_ranges([]), []);
    }

    /// A message set may declare extensions no tag can carry at all.
    #[test]
    fn test_a_range_past_the_largest_field_number_is_dropped() {
        assert_eq!(
            canonical_ranges([(4, 1000), (600_000_000, 2_147_483_647)]),
            [(4, 999)]
        );
    }
}

/// The unit tests of `test_fd_is_packed.py` (spec 0488 S2), on
/// `fd_is_packed` over the declaration matrix (spec 0371 S1, S2); its
/// files, built with Python's protobuf, written here as text.
#[cfg(test)]
mod test_fd_is_packed {
    use super::*;
    use std::collections::BTreeMap;

    /// A file of `syntax` (and `edition`), message `M` of `fields`, in a
    /// pool: each field's `fd_is_packed`.
    fn packed(syntax: &str, edition: &str, fields: &str) -> BTreeMap<String, bool> {
        let pkg = format!("m{syntax}");
        let text = format!(
            "name: \"m_{syntax}.proto\" package: \"{pkg}\" syntax: \"{syntax}\" {edition} message_type {{ name: \"M\" {fields} }}"
        );
        let bytes = prototext_upb::text_format::parse_descriptor(&text, false).unwrap();
        let pool = prototext_upb::Pool::with_descriptor_proto();
        pool.add_file(&bytes).unwrap();
        let m = pool.get_message_by_name(&format!("{pkg}.M")).unwrap();
        m.fields()
            .map(|f| (f.name().to_string(), fd_is_packed(&f)))
            .collect()
    }

    /// `_field`: repeated unless said, with `packed`, another option, or a
    /// repeated_field_encoding.
    fn field(name: &str, number: u32, ty: &str, extra: &str) -> String {
        let label = if extra.contains("OPTIONAL") {
            ""
        } else {
            "label: LABEL_REPEATED "
        };
        let extra = extra.replace("OPTIONAL", "label: LABEL_OPTIONAL");
        format!("field {{ name: \"{name}\" number: {number} type: {ty} {label}{extra} }} ")
    }

    fn want(pairs: &[(&str, bool)]) -> BTreeMap<String, bool> {
        pairs.iter().map(|(n, b)| (n.to_string(), *b)).collect()
    }

    #[test]
    fn test_proto2() {
        let fields = [
            field("default", 1, "TYPE_INT32", ""),
            field("packed", 2, "TYPE_INT32", "options { packed: true }"),
            field("strings", 3, "TYPE_STRING", ""),
            field("single", 4, "TYPE_INT32", "OPTIONAL"),
        ]
        .concat();
        assert_eq!(
            packed("proto2", "", &fields),
            want(&[
                ("default", false),
                ("packed", true),
                ("strings", false),
                ("single", false)
            ])
        );
    }

    /// The proto3 field carrying another option, present but silent
    /// (prost-reflect's section 1 case), is packed.
    #[test]
    fn test_proto3() {
        let fields = [
            field("default", 1, "TYPE_INT32", ""),
            field("unpacked", 2, "TYPE_INT32", "options { packed: false }"),
            field(
                "with_option",
                3,
                "TYPE_INT32",
                "options { deprecated: false }",
            ),
            field("strings", 4, "TYPE_STRING", ""),
            field("single", 5, "TYPE_INT32", "OPTIONAL"),
        ]
        .concat();
        assert_eq!(
            packed("proto3", "", &fields),
            want(&[
                ("default", true),
                ("unpacked", false),
                ("with_option", true),
                ("strings", false),
                ("single", false)
            ])
        );
    }

    #[test]
    fn test_editions() {
        let fields = [
            field("default", 1, "TYPE_INT32", ""),
            field(
                "expanded",
                2,
                "TYPE_INT32",
                "options { features { repeated_field_encoding: EXPANDED } }",
            ),
            field(
                "packed",
                3,
                "TYPE_INT32",
                "options { features { repeated_field_encoding: PACKED } }",
            ),
        ]
        .concat();
        assert_eq!(
            packed("editions", "edition: EDITION_2023", &fields),
            want(&[("default", true), ("expanded", false), ("packed", true)])
        );
    }
}
