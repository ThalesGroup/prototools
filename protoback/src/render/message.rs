// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Messages, fields, enums and services (`re_descriptor.py`,
//! `re_field.py`, `re_enum.py`, `re_service.py`, `re_method.py`), stage 1:
//! no options, no features block, no source comments.

use prototext_upb::descriptor_access::{DescriptorProto, FeatureSet, FieldDescriptorProto};

use super::file::{anomaly, extend_blocks, Ctx};
use super::names::{short_ref, shorten_method_type, type_name};
use super::pyfmt::json_dumps;
use super::text::{Block, Line, LineType, MAX_LINE_LENGTH, TAB_SIZE};
use super::{text, Def};
use crate::features::{self, Feature, FIELD_PRESENCE_IMPLICIT, MESSAGE_ENCODING_DELIMITED};
use crate::graph::{Kind, TypeRef};

const LABEL_REQUIRED: i32 = 2;
const LABEL_REPEATED: i32 = 3;
const TYPE_GROUP: i32 = 10;
const TYPE_MESSAGE: i32 = 11;
const TYPE_ENUM: i32 = 14;
/// `globals.FIELD_NUM_MAX`.
const FIELD_NUM_MAX: i32 = 536_870_911;

/// A message whose body is empty collapses to `message X {}`: its first
/// line after the header is the body's `}`.
pub fn collapse_empty(lines: &mut Block) {
    if lines.lines.get(1).is_some_and(|l| l.text == "}") {
        lines.lines[0].text.push('}');
        lines.lines.remove(1);
    }
}

/// The messages enclosing a field of message node `msg`, outermost
/// first, `msg`'s own last (`_message_chain`).
fn message_chain<'a>(ctx: &Ctx<'_, 'a>, msg: Option<usize>) -> Vec<DescriptorProto<'a>> {
    let mut chain = Vec::new();
    let mut cur = msg;
    while let Some(i) = cur {
        let node = &ctx.shared.graph.nodes[i];
        if node.kind != Kind::Message {
            break;
        }
        if let Some(m) = ctx.shared.message_def(i) {
            chain.push(m);
        }
        cur = node.parent;
    }
    chain.reverse();
    chain
}

/// A resolved feature of a field of message node `msg` (none for a
/// top-level extension), for an editions file; `None` otherwise
/// (`_resolve_field_features`).
fn field_feature(
    ctx: &Ctx<'_, '_>,
    msg: Option<usize>,
    field: &FieldDescriptorProto<'_>,
    feature: Feature,
) -> Option<i32> {
    let edition = ctx.edition?;
    let file = ctx.shared.file_def(ctx.file)?;
    let mut sets: Vec<Option<FeatureSet<'_>>> = vec![file.options().and_then(|o| o.features())];
    sets.extend(
        message_chain(ctx, msg)
            .iter()
            .map(|m| m.options().and_then(|o| o.features())),
    );
    sets.push(field.options().and_then(|o| o.features()));
    Some(features::resolve(
        ctx.shared.edition_defaults,
        feature,
        edition,
        &sets,
    ))
}

/// `field_label`.
fn field_label(
    ctx: &Ctx<'_, '_>,
    f: &FieldDescriptorProto<'_>,
    is_oneof: bool,
    presence: Option<i32>,
) -> &'static str {
    if is_oneof {
        return "";
    }
    if f.label() == LABEL_REPEATED {
        return "repeated ";
    }
    if ctx.editions_target() {
        return "";
    }
    if presence.is_some() {
        // An editions file rendered as proto2: stage 4.
        return "optional ";
    }
    if ctx.proto3() {
        if f.label() == LABEL_REQUIRED {
            return "";
        }
        return if f.proto3_optional() { "optional " } else { "" };
    }
    if f.label() == LABEL_REQUIRED {
        "required "
    } else {
        "optional "
    }
}

/// A type's name from `scope`: a scalar's own, a node's `short_ref`.
fn type_ref(
    ctx: &Ctx<'_, '_>,
    t: Option<TypeRef>,
    f: &FieldDescriptorProto<'_>,
    scope: usize,
) -> String {
    match t {
        Some(TypeRef::Node(n)) => short_ref(ctx.shared, &ctx.shared.graph.nodes[n].fqdn, scope),
        Some(TypeRef::Scalar(t)) => type_name(t),
        None => type_name(f.r#type()),
    }
}

/// `ReFieldDescriptorProto.render`, stage 1: the field's declaration,
/// with no options; a group's body inline.
pub fn field(ctx: &Ctx<'_, '_>, fd: usize, depth: usize, is_oneof: bool) -> Block {
    let shared = ctx.shared;
    let node = &shared.graph.nodes[fd];
    let mut out = Block::new();
    let Some(Def::Field(f)) = shared.def(fd) else {
        return out;
    };
    let name = text(f.name());
    let parent = node.parent.unwrap_or(ctx.file);
    let msg = (shared.graph.nodes[parent].kind == Kind::Message).then_some(parent);

    // A map field: its entry's key and value types.
    let mut map_string = None;
    if f.r#type() == TYPE_MESSAGE && f.label() == LABEL_REPEATED {
        if let Some(TypeRef::Node(entry)) = node.type_descriptor {
            let e = &shared.graph.nodes[entry];
            if let (true, Some(m)) = (e.present, shared.message_def(entry)) {
                if m.options().is_some_and(|o| o.map_entry()) {
                    let fields = m.field();
                    let key = fields.iter().find(|x| x.number() == 1);
                    let value = fields.iter().find(|x| x.number() == 2);
                    match (key, value) {
                        (Some(k), Some(v)) => {
                            map_string = Some(format!(
                                "map<{}, {}> {name} = {}",
                                map_type(ctx, k, parent),
                                map_type(ctx, v, parent),
                                f.number()
                            ));
                        }
                        _ => {
                            out.push(anomaly(
                                "WARNING",
                                "render",
                                &format!(
                                    "non-canonical map entry '{}'; rendered as repeated message \u{2014} wire semantics differ from original",
                                    text(m.name())
                                ),
                                depth,
                            ));
                            let r = type_ref(ctx, node.type_descriptor, &f, parent);
                            out.push(Line::code(
                                format!("repeated {r} {name} = {};", f.number()),
                                depth,
                            ));
                            return out;
                        }
                    }
                }
            }
        }
    }

    let presence = field_feature(ctx, msg, &f, Feature::FieldPresence);
    let encoding = field_feature(ctx, msg, &f, Feature::MessageEncoding);
    let allow_groups = match encoding {
        Some(e) => e == MESSAGE_ENCODING_DELIMITED,
        None => ctx.proto2_or_editions(),
    };
    let is_group_field = f.r#type() == TYPE_GROUP;

    let string = match &map_string {
        Some(s) => s.clone(),
        None => {
            if ctx.proto3() && !is_oneof && f.label() == LABEL_REQUIRED {
                out.push(anomaly(
                    "WARNING",
                    "proto3",
                    "'required' label is not valid in proto3; rendered as implicit singular",
                    depth,
                ));
            }
            let mut s = field_label(ctx, &f, is_oneof, presence).to_string();
            if !is_group_field || !allow_groups {
                if f.r#type() == TYPE_GROUP {
                    out.push(anomaly(
                        "WARNING",
                        "proto3",
                        "group field; rendered as plain message field \u{2014} wire semantics differ from original",
                        depth,
                    ));
                }
                let r = type_ref(ctx, node.type_descriptor, &f, parent);
                s.push_str(&format!("{r} {name}"));
            } else {
                let r = type_ref(ctx, node.type_descriptor, &f, fd);
                s.push_str(&format!("group {r}"));
            }
            s.push_str(&format!(" = {}", f.number()));
            s
        }
    };

    // The options block is empty in stage 1, but for proto3's dropped
    // default (C4), which makes it all orphan.
    let mut opt_block = Block::new();
    if ctx.proto3() && presence.is_none() && f.has_default_value() {
        opt_block.push(anomaly(
            "WARNING",
            "proto3",
            "explicit default value is not valid in proto3; omitted",
            depth + 1,
        ));
    }
    out.extend(format_composite(
        opt_block,
        Line::code(string, depth),
        depth,
    ));

    if map_string.is_some() || !is_group_field || !allow_groups {
        out.postpend(";");
    } else {
        out.postpend(" {");
        if let Some(group) = shared.graph.find(&format!("desc:{}", text(f.type_name()))) {
            out.extend(message(ctx, group, depth, true));
        }
    }

    if node
        .targets
        .iter()
        .any(|&t| !shared.graph.nodes[t].is_summoned)
    {
        out.abandon();
    }
    out
}

/// A map entry field's type name (`_get_field_type_name`).
fn map_type(ctx: &Ctx<'_, '_>, f: &FieldDescriptorProto<'_>, scope: usize) -> String {
    match f.r#type() {
        TYPE_MESSAGE => short_ref(ctx.shared, &format!("desc:{}", text(f.type_name())), scope),
        TYPE_ENUM => short_ref(ctx.shared, &format!("enum:{}", text(f.type_name())), scope),
        t => type_name(t),
    }
}

/// `format_composite_options`: the declaration alone without options;
/// with orphan options only, bracketed as orphans; one, inline; more,
/// one per line.
pub fn format_composite(mut options: Block, mut prolog: Line, depth: usize) -> Block {
    let mut is_orphan = true;
    for line in options.lines.iter_mut().rev() {
        if line.kind == LineType::Code {
            is_orphan = false;
            line.text.pop();
            break;
        }
    }
    if options.is_empty() {
        return Block {
            lines: vec![prolog],
        };
    }
    if is_orphan {
        options.insert(0, Line::orphan("[", depth));
        options.insert(0, prolog);
        options.push(Line::orphan("]", depth));
        options.push(Line::code("", depth));
        return options;
    }
    if options.len() == 1 {
        prolog
            .text
            .push_str(&format!(" [{}]", options.lines[0].text));
        return Block {
            lines: vec![prolog],
        };
    }
    prolog.text.push_str(" [");
    options.insert(0, prolog);
    options.push(Line::code("]", depth));
    options
}

/// `ReDescriptorProto.render`, stage 1: the body of message node `msg`,
/// its lines at `depth + 1`, its closing `}` at `depth` (the caller adds
/// the header). `force`: a group's body, its fields never orphaned.
pub fn message(ctx: &Ctx<'_, '_>, msg: usize, depth: usize, force: bool) -> Block {
    let shared = ctx.shared;
    let mut out = Block::new();
    let Some(m) = shared.message_def(msg) else {
        out.push(Line::code("}", depth));
        out.div_maybe(depth);
        return out;
    };

    let ext = extend_blocks(ctx, msg, &m.extension(), depth + 1, false);
    if !ext.is_empty() {
        out.extend(ext);
        out.div_maybe(depth);
    }

    if !ctx.proto2_or_editions()
        && m.options()
            .is_some_and(|o| o.has_message_set_wire_format() && o.message_set_wire_format())
    {
        out.push(anomaly(
            "WARNING",
            "proto3",
            "'message_set_wire_format = true' is not valid in proto3; omitted",
            depth + 1,
        ));
    }

    let node = &shared.graph.nodes[msg];
    for sf in &node.stripped_fields {
        // (number, name, type_name, label), in reproto's slot order.
        if let [number, name, type_name, label] = sf.as_slice() {
            out.push(Line::orphan(
                format!("{label} {type_name} {name} = {number};"),
                depth + 1,
            ));
        }
    }
    if !node.stripped_fields.is_empty() {
        out.div_maybe(depth);
    }

    let fields = m.field();
    let oneofs = m.oneof_decl();
    let mut synthetic = vec![false; oneofs.len()];
    for (idx, o) in oneofs.iter().enumerate() {
        let members: Vec<&FieldDescriptorProto<'_>> = fields
            .iter()
            .filter(|f| f.has_oneof_index() && f.oneof_index() == idx as i32)
            .collect();
        let presence = members
            .first()
            .and_then(|f| field_feature(ctx, Some(msg), f, Feature::FieldPresence));
        let name = text(o.name());
        synthetic[idx] = name.starts_with('_')
            && members.len() == 1
            && match presence {
                Some(p) => p == FIELD_PRESENCE_IMPLICIT,
                None => ctx.proto3() && members[0].proto3_optional(),
            };
    }

    let mut done = vec![false; oneofs.len()];
    for f in &fields {
        let Some(fd) = shared.child(Kind::Field, msg, f.name()) else {
            continue;
        };
        let oneof = f.has_oneof_index().then(|| f.oneof_index() as usize);
        let real_oneof = oneof.filter(|&o| !synthetic.get(o).copied().unwrap_or(false));
        let Some(o) = real_oneof else {
            let mut lines = field(ctx, fd, depth + 1, false);
            if !force && !shared.graph.nodes[fd].is_summoned {
                lines.abandon();
            }
            out.extend(lines);
            continue;
        };
        if done.get(o).copied().unwrap_or(true) {
            continue;
        }
        done[o] = true;
        let mut block = Block::new();
        let mut is_orphan = true;
        for f2 in &fields {
            if !(f2.has_oneof_index() && f2.oneof_index() as usize == o) {
                continue;
            }
            let Some(fd2) = shared.child(Kind::Field, msg, f2.name()) else {
                continue;
            };
            let mut lines = field(ctx, fd2, depth + 2, true);
            if shared.graph.nodes[fd2].is_summoned {
                is_orphan = false;
            } else {
                lines.abandon();
            }
            block.extend(lines);
        }
        let kind = if is_orphan {
            LineType::Orphan
        } else {
            LineType::Code
        };
        block.insert(
            0,
            Line {
                text: format!("oneof {} {{", text(oneofs[o].name())),
                level: depth + 1,
                kind,
            },
        );
        block.push(Line {
            text: "}".into(),
            level: depth + 1,
            kind,
        });
        out.extend(block);
    }
    out.div_maybe(depth);

    for n in m.nested_type() {
        let Some(nested) = shared.child(Kind::Message, msg, n.name()) else {
            continue;
        };
        let nn = &shared.graph.nodes[nested];
        let is_map_entry = shared
            .message_def(nested)
            .and_then(|d| d.options())
            .is_some_and(|o| o.map_entry());
        if nn.is_group || is_map_entry {
            continue;
        }
        let mut lines = message(ctx, nested, depth + 1, false);
        lines.insert(
            0,
            Line::code(format!("message {} {{", text(n.name())), depth + 1),
        );
        collapse_empty(&mut lines);
        if !nn.is_summoned {
            lines.abandon();
        }
        out.extend(lines);
    }
    out.div_maybe(depth);

    for e in m.enum_type() {
        if let Some(en) = shared.child(Kind::Enum, msg, e.name()) {
            out.extend(enumeration(ctx, en, depth + 1));
        }
    }
    out.div_maybe(depth);

    let reserved = reserved(ctx, &m, depth + 1);
    if !reserved.is_empty() {
        out.extend(reserved);
        out.div_maybe(depth);
    }

    out.trim_end(1);
    out.push(Line::code("}", depth));
    out.div_maybe(depth);
    out
}

/// `render_reserved`: extension ranges (in proto3, the anomaly B2),
/// reserved ranges, reserved names.
fn reserved(ctx: &Ctx<'_, '_>, m: &DescriptorProto<'_>, depth: usize) -> Block {
    let mut out = Block::new();
    for r in m.extension_range() {
        let (start, end) = (r.start(), r.end());
        if !ctx.proto2_or_editions() {
            out.push(anomaly(
                "OMITTED",
                "proto3",
                &format!("extensions {start} to {end}; \u{2014} not valid in proto3"),
                depth,
            ));
            continue;
        }
        let s = if start == end - 1 {
            format!("extensions {start};")
        } else if end > FIELD_NUM_MAX {
            // reproto's `end - 1 >= FIELD_NUM_MAX`.
            format!("extensions {start} to max;")
        } else {
            format!("extensions {start} to {};", end - 1)
        };
        out.push(Line::code(s, depth));
    }
    for r in m.reserved_range() {
        let (start, end) = (r.start(), r.end());
        let s = if start == end - 1 {
            format!("reserved {start};")
        } else {
            format!("reserved {start} to {};", end - 1)
        };
        out.push(Line::code(s, depth));
    }
    for name in m.reserved_name() {
        let name = text(name);
        let s = if ctx.editions_target() {
            format!("reserved {name};")
        } else {
            format!("reserved \"{name}\";")
        };
        out.push(Line::code(s, depth));
    }
    out
}

/// `ReEnumDescriptorProto.render`, stage 1.
pub fn enumeration(ctx: &Ctx<'_, '_>, en: usize, depth: usize) -> Block {
    let mut out = Block::new();
    let Some(Def::Enum(e)) = ctx.shared.def(en) else {
        return out;
    };
    out.div_maybe(depth);
    for v in e.value() {
        out.push(Line::code(
            format!("{} = {};", text(v.name()), v.number()),
            depth + 1,
        ));
    }
    let ranges = e.reserved_range();
    let names = e.reserved_name();
    if !ranges.is_empty() || !names.is_empty() {
        out.div_maybe(depth);
    }
    for r in ranges {
        let (start, end) = (r.start(), r.end());
        let s = if start == end {
            format!("reserved {start};")
        } else {
            format!("reserved {start} to {end};")
        };
        out.push(Line::code(s, depth + 1));
    }
    for n in names {
        let n = text(n);
        let s = if ctx.editions_target() {
            format!("reserved {n};")
        } else {
            format!("reserved {};", json_dumps(&n))
        };
        out.push(Line::code(s, depth + 1));
    }
    out.push(Line::code("}", depth));
    out.div_maybe(depth);
    out.insert(0, Line::code(format!("enum {} {{", text(e.name())), depth));
    out
}

/// `ReServiceDescriptorProto.render`, stage 1: its reachable methods.
pub fn service(ctx: &Ctx<'_, '_>, svc: usize, depth: usize) -> Block {
    let shared = ctx.shared;
    let mut out = Block::new();
    let Some(Def::Service(s)) = shared.def(svc) else {
        return out;
    };
    out.div_maybe(depth);
    let node = &shared.graph.nodes[svc];
    for sm in &node.stripped_methods {
        // (name, input_type, output_type), in reproto's slot order.
        if let [name, input, output] = sm.as_slice() {
            out.push(Line::orphan(
                format!("rpc {name} ({input}) returns ({output});"),
                depth + 1,
            ));
        }
    }
    if !node.stripped_methods.is_empty() {
        out.div_maybe(depth);
    }
    for m in s.method() {
        let Some(meth) = shared.child(Kind::Method, svc, m.name()) else {
            continue;
        };
        if !shared.graph.nodes[meth].is_reachable {
            continue;
        }
        // The node's own definition, as for every node (the first one,
        // under --keep-duplicates).
        let Some(Def::Method(m)) = shared.def(meth) else {
            continue;
        };
        let mut lines = method(ctx, &m, depth + 1);
        lines.div_maybe(depth);
        out.extend(lines);
    }
    out.div_maybe(depth);
    out.trim_end(1);
    out.push(Line::code("}", depth));
    out.div_maybe(depth);
    out.insert(
        0,
        Line::code(format!("service {} {{", text(s.name())), depth),
    );
    out
}

/// `ReMethodDescriptorProto.render`, stage 1 (no options): one line, or
/// two past column 79.
fn method(
    ctx: &Ctx<'_, '_>,
    m: &prototext_upb::descriptor_access::MethodDescriptorProto<'_>,
    depth: usize,
) -> Block {
    let shared = ctx.shared;
    let input = shorten_method_type(shared, ctx.file, &text(m.input_type()));
    let output = shorten_method_type(shared, ctx.file, &text(m.output_type()));
    let cs = if m.client_streaming() { "stream " } else { "" };
    let ss = if m.server_streaming() { "stream " } else { "" };
    let s1 = format!("rpc {}({cs}{input}) ", text(m.name()));
    let s2 = format!("returns ({ss}{output});");
    let mut out = Block::new();
    if depth * TAB_SIZE + s1.chars().count() + s2.chars().count() > MAX_LINE_LENGTH {
        out.push(Line::code(s1, depth));
        out.push(Line::code(s2, depth + 2));
    } else {
        out.push(Line::code(format!("{s1}{s2}"), depth));
    }
    out
}
