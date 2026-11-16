// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Messages, fields, enums and services (`re_descriptor.py`,
//! `re_field.py`, `re_enum.py`, `re_service.py`, `re_method.py`), with
//! their built-in options (stage 2); no features block, no source comments
//! yet.

use prototext_upb::descriptor_access::{DescriptorProto, FeatureSet, FieldDescriptorProto};

use prototext_upb::MessageRef;

use super::file::{anomaly, extend_blocks, features_block, Ctx};
use super::names::{short_ref, shorten_method_type, type_name};
use super::options::Options;
use super::pyfmt::{bytes_default, float_repr, json_dumps, render_bytes};
use super::text::{Block, Line, LineType, MAX_LINE_LENGTH, TAB_SIZE};
use super::{text, Def};
use crate::features::{
    self, Feature, FIELD_PRESENCE_IMPLICIT, FIELD_PRESENCE_LEGACY_REQUIRED,
    MESSAGE_ENCODING_DELIMITED, REPEATED_FIELD_ENCODING_PACKED,
};
use crate::graph::{Kind, TypeRef};

const LABEL_REQUIRED: i32 = 2;
const LABEL_REPEATED: i32 = 3;
const TYPE_GROUP: i32 = 10;
const TYPE_MESSAGE: i32 = 11;
const TYPE_ENUM: i32 = 14;
const TYPE_STRING: i32 = 9;
const TYPE_BYTES: i32 = 12;
const TYPE_BOOL: i32 = 8;
const TYPE_FLOAT: i32 = 2;
const TYPE_DOUBLE: i32 = 1;

/// `globals.PACKABLE_TYPES`: the scalar types, but string, bytes,
/// message and group.
fn packable(t: i32) -> bool {
    !matches!(t, TYPE_STRING | TYPE_BYTES | TYPE_GROUP | TYPE_MESSAGE)
}

/// `render_options`: the options of node `node`, a message, enum, service
/// or method, which reproto finds through its pool: none when the pool
/// refused the element's file or does not have the element (under a
/// synthetic group), nor when one of them cannot be rendered (its
/// `except`).
fn element_options(
    ctx: &Ctx<'_, '_>,
    node: usize,
    kind: Options,
    options: Option<MessageRef<'_>>,
    depth: usize,
    exclude: &[&str],
) -> Vec<Block> {
    if ctx.refused || !ctx.in_pool(node) {
        return Vec::new();
    }
    own_options(ctx, kind, options, false, depth, exclude).unwrap_or_default()
}

/// Options reproto reads from the descriptor itself (a field's, a oneof's,
/// an extension range's, an enum value's), composite or not; `Err` when
/// one cannot be rendered.
fn own_options(
    ctx: &Ctx<'_, '_>,
    kind: Options,
    options: Option<MessageRef<'_>>,
    composite: bool,
    depth: usize,
    exclude: &[&str],
) -> Result<Vec<Block>, super::options::Unrenderable> {
    let (Some(pool), Some(o)) = (&ctx.shared.options, options) else {
        return Ok(Vec::new());
    };
    let Some(msg) = pool.parse_ref(kind, &o) else {
        return Ok(Vec::new());
    };
    super::options::options(&ctx.scope(), kind, &msg, composite, depth, exclude)
}

/// `_camel_case`: protoc's default JSON name.
fn camel_case(name: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for c in name.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `_render_default_value`: `default = v,` for a field with a default;
/// `Err` where Python's reading of it raises (a bytes default it cannot
/// unescape, a number it cannot parse).
fn default_value(f: &FieldDescriptorProto<'_>, depth: usize) -> Result<Block, ()> {
    let mut out = Block::new();
    if !f.has_default_value() {
        return Ok(out);
    }
    let raw = text(f.default_value());
    let value = match f.r#type() {
        TYPE_ENUM => raw,
        TYPE_STRING => json_dumps(&raw),
        TYPE_BYTES => render_bytes(&bytes_default(&raw).ok_or(())?),
        TYPE_BOOL => if raw.to_lowercase() == "true" {
            "true"
        } else {
            "false"
        }
        .to_string(),
        TYPE_FLOAT | TYPE_DOUBLE => float_repr(py_float(&raw).ok_or(())?),
        _ => raw.trim().parse::<i128>().map_err(|_| ())?.to_string(),
    };
    out.push(Line::code(format!("default = {value},"), depth));
    Ok(out)
}

/// Python's `float(s)` for the defaults protoc writes: a decimal, or
/// `inf`, `-inf`, `nan` in any case.
fn py_float(s: &str) -> Option<f64> {
    let t = s.trim();
    match t.to_ascii_lowercase().as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
        "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
        "nan" | "+nan" | "-nan" => Some(f64::NAN),
        _ => t.parse::<f64>().ok(),
    }
}

/// `packed_option`: `true`, `false`, or nothing.
fn packed_option(
    ctx: &Ctx<'_, '_>,
    has: bool,
    effective: bool,
    editions: bool,
) -> Option<&'static str> {
    let v = if effective { "true" } else { "false" };
    if editions {
        if has {
            return Some(v);
        }
        if ctx.target == "proto2" {
            return effective.then_some("true");
        }
        return None;
    }
    if has {
        return Some(v);
    }
    if ctx.syntax == ctx.target {
        return None;
    }
    effective.then_some("true")
}
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
    if let Some(p) = presence {
        // An editions file written as proto2: presence picks the label.
        return if p == FIELD_PRESENCE_LEGACY_REQUIRED {
            "required "
        } else {
            "optional "
        };
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
    // A group, or an editions DELIMITED field with its synthetic group
    // (under --force-proto2-output).
    let editions_group = node.editions_group;
    let is_group_field = f.r#type() == TYPE_GROUP || editions_group.is_some();

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
                let r = match &node.editions_group_name {
                    Some(name) => name.clone(),
                    None => type_ref(ctx, node.type_descriptor, &f, fd),
                };
                s.push_str(&format!("group {r}"));
            }
            s.push_str(&format!(" = {}", f.number()));
            s
        }
    };

    // The options block, in reproto's order: the default, json_name,
    // packed, then the options; reproto's `try` around it turns a failure
    // into C5 and the bare declaration.
    match field_options(ctx, &f, msg, presence, depth) {
        Ok(opt_block) => out.extend(format_composite(
            opt_block,
            Line::code(string, depth),
            depth,
        )),
        Err(()) => {
            out.push(anomaly(
                "WARNING",
                "render",
                "field options could not be rendered (internal error)",
                depth,
            ));
            out.push(Line::code(string, depth));
        }
    }

    if map_string.is_some() || !is_group_field || !allow_groups {
        out.postpend(";");
    } else {
        out.postpend(" {");
        let group =
            editions_group.or_else(|| shared.graph.find(&format!("desc:{}", text(f.type_name()))));
        if let Some(group) = group {
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

/// A field's options block (`ReFieldDescriptorProto.render`'s `try`).
fn field_options(
    ctx: &Ctx<'_, '_>,
    f: &FieldDescriptorProto<'_>,
    msg: Option<usize>,
    presence: Option<i32>,
    depth: usize,
) -> Result<Block, ()> {
    let mut opt_block = Block::new();
    let render_default = f.has_default_value()
        && match presence {
            Some(p) => p != FIELD_PRESENCE_IMPLICIT,
            None => !ctx.proto3(),
        };
    if render_default {
        opt_block.extend(default_value(f, depth + 1)?);
    } else if ctx.proto3() && f.has_default_value() {
        opt_block.push(anomaly(
            "WARNING",
            "proto3",
            "explicit default value is not valid in proto3; omitted",
            depth + 1,
        ));
    }
    let json_name = text(f.json_name());
    if !json_name.is_empty() && json_name != camel_case(&text(f.name())) {
        opt_block.push(Line::code(
            format!("json_name = \"{json_name}\","),
            depth + 1,
        ));
    }
    let options = f.options();
    let has_packed = options.as_ref().is_some_and(|o| o.has_packed());
    let is_packable = f.label() == LABEL_REPEATED && packable(f.r#type());
    let mut effective = if has_packed {
        options.as_ref().is_some_and(|o| o.packed())
    } else {
        ctx.syntax == "proto3"
    };
    let editions = presence.is_some();
    if editions && !has_packed && is_packable {
        effective = field_feature(ctx, msg, f, Feature::RepeatedFieldEncoding)
            == Some(REPEATED_FIELD_ENCODING_PACKED);
    }
    if is_packable || has_packed {
        if let Some(v) = packed_option(ctx, has_packed, effective, editions) {
            opt_block.push(Line::code(format!("packed = {v},"), depth + 1));
        }
    }
    opt_block.extend(features_block(
        ctx,
        options.as_ref().and_then(|o| o.features()),
        depth + 1,
        true,
    ));
    let blocks = own_options(
        ctx,
        Options::Field,
        options.map(|o| o.0),
        true,
        depth + 1,
        &["packed", "json_name"],
    )
    .map_err(|_| ())?;
    for b in blocks {
        opt_block.extend(b);
    }
    Ok(opt_block)
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

    let msf: &[&str] = if ctx.proto2_or_editions() {
        &[]
    } else {
        &["message_set_wire_format"]
    };
    let features = m.options().and_then(|o| o.features());
    let has_features = features.is_some();
    out.extend(features_block(ctx, features, depth + 1, false));
    let blocks = element_options(
        ctx,
        msg,
        Options::Message,
        m.options().map(|o| o.0),
        depth + 1,
        msf,
    );
    let any = !blocks.is_empty();
    blocks.into_iter().for_each(|b| out.extend(b));
    if any || (has_features && ctx.editions_target()) {
        out.div_maybe(depth);
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
        // The oneof's options, from its descriptor (no `try`: a failure
        // fails the file).
        match own_options(
            ctx,
            Options::Oneof,
            oneofs[o].options().map(|x| x.0),
            false,
            depth + 2,
            &[],
        ) {
            Ok(blocks) => {
                let features = features_block(
                    ctx,
                    oneofs[o].options().and_then(|x| x.features()),
                    depth + 2,
                    false,
                );
                let mut at = 1;
                for line in features.lines {
                    block.insert(at, line);
                    at += 1;
                }
                for b in blocks {
                    for line in b.lines {
                        block.insert(at, line);
                        at += 1;
                    }
                }
            }
            Err(_) => ctx.failed.set(true),
        }
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
        let mut s = if start == end - 1 {
            format!("extensions {start}")
        } else if end > FIELD_NUM_MAX {
            // reproto's `end - 1 >= FIELD_NUM_MAX`.
            format!("extensions {start} to max")
        } else {
            format!("extensions {start} to {}", end - 1)
        };
        let mut texts = match own_options(
            ctx,
            Options::ExtensionRange,
            r.options().map(|o| o.0),
            true,
            depth + 1,
            &["declaration"],
        ) {
            Ok(t) => t,
            Err(_) => {
                ctx.failed.set(true);
                Vec::new()
            }
        };
        // The last option not an orphan loses its comma.
        if let Some(t) = texts
            .iter_mut()
            .rev()
            .find(|t| t.lines.first().is_some_and(|l| l.kind != LineType::Orphan))
        {
            if let Some(last) = t.lines.last_mut() {
                if last.text.ends_with(',') {
                    last.text.pop();
                }
            }
        }
        let set = texts
            .iter()
            .filter(|t| t.lines.first().is_some_and(|l| l.kind != LineType::Orphan))
            .count();
        if set == 0 {
            s.push(';');
            out.push(Line::code(s, depth));
            if !texts.is_empty() {
                out.push(Line::orphan("[", depth));
                texts.into_iter().for_each(|t| out.extend(t));
                out.push(Line::orphan("]", depth));
            }
        } else if set == 1 && texts.len() == 1 && texts[0].len() == 1 {
            let short = format!("[{}];", texts[0].lines[0].text);
            let s2 = format!("{s} {short}");
            if s2.chars().count() <= MAX_LINE_LENGTH - depth * TAB_SIZE {
                out.push(Line::code(s2, depth));
            } else {
                out.push(Line::code(s, depth));
                out.push(Line::code(short, depth + 1));
            }
        } else {
            out.push(Line::code(format!("{s} ["), depth));
            texts.into_iter().for_each(|t| out.extend(t));
            out.push(Line::code("];", depth));
        }
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
    out.extend(features_block(
        ctx,
        e.options().and_then(|o| o.features()),
        depth + 1,
        false,
    ));
    let blocks = element_options(
        ctx,
        en,
        Options::Enum,
        e.options().map(|o| o.0),
        depth + 1,
        &[],
    );
    blocks.into_iter().for_each(|b| out.extend(b));
    out.div_maybe(depth);
    for v in e.value() {
        out.extend(enum_value(ctx, en, &v, depth + 1));
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

/// `ReEnumValueDescriptorProto.render`: `NAME = N`, its options inline.
/// reproto looks the enum up in its pool first, with no `try`: in a file
/// the pool refused, that fails the file; so does an option that cannot
/// be rendered.
fn enum_value(
    ctx: &Ctx<'_, '_>,
    en: usize,
    v: &prototext_upb::descriptor_access::EnumValueDescriptorProto<'_>,
    depth: usize,
) -> Block {
    let prolog = Line::code(format!("{} = {}", text(v.name()), v.number()), depth);
    if ctx.refused || !ctx.in_pool(en) {
        ctx.failed.set(true);
    }
    let options = match own_options(
        ctx,
        Options::EnumValue,
        v.options().map(|o| o.0),
        true,
        depth + 1,
        &[],
    ) {
        Ok(blocks) => {
            let mut b =
                features_block(ctx, v.options().and_then(|o| o.features()), depth + 1, true);
            blocks.into_iter().for_each(|x| b.extend(x));
            b
        }
        Err(_) => {
            ctx.failed.set(true);
            Block::new()
        }
    };
    let mut out = format_composite(options, prolog, depth);
    out.postpend(";");
    out
}

/// `ReServiceDescriptorProto.render`: its options, its reachable methods.
pub fn service(ctx: &Ctx<'_, '_>, svc: usize, depth: usize) -> Block {
    let shared = ctx.shared;
    let mut out = Block::new();
    let Some(Def::Service(s)) = shared.def(svc) else {
        return out;
    };
    let blocks = element_options(
        ctx,
        svc,
        Options::Service,
        s.options().map(|o| o.0),
        depth + 1,
        &[],
    );
    blocks.into_iter().for_each(|b| out.extend(b));
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
        let mut lines = method(ctx, meth, &m, depth + 1);
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
    meth: usize,
    m: &prototext_upb::descriptor_access::MethodDescriptorProto<'_>,
    depth: usize,
) -> Block {
    let shared = ctx.shared;
    let input = shorten_method_type(shared, ctx.file, &text(m.input_type()));
    let output = shorten_method_type(shared, ctx.file, &text(m.output_type()));
    let cs = if m.client_streaming() { "stream " } else { "" };
    let ss = if m.server_streaming() { "stream " } else { "" };
    let s1 = format!("rpc {}({cs}{input}) ", text(m.name()));
    let blocks = element_options(
        ctx,
        meth,
        Options::Method,
        m.options().map(|o| o.0),
        depth + 1,
        &[],
    );
    let mut out = Block::new();
    if blocks.is_empty() {
        let s2 = format!("returns ({ss}{output});");
        if depth * TAB_SIZE + s1.chars().count() + s2.chars().count() > MAX_LINE_LENGTH {
            out.push(Line::code(s1, depth));
            out.push(Line::code(s2, depth + 2));
        } else {
            out.push(Line::code(format!("{s1}{s2}"), depth));
        }
        return out;
    }
    blocks.into_iter().for_each(|b| out.extend(b));
    out.div_maybe(depth);
    out.trim_end(1);
    out.push(Line::code("}", depth));
    out.div_maybe(depth);
    let s2 = format!("returns ({ss}{output}) {{");
    if depth * TAB_SIZE + s1.chars().count() + 1 + s2.chars().count() > MAX_LINE_LENGTH {
        out.insert(0, Line::code(s2, depth + 2));
        out.insert(0, Line::code(s1, depth));
    } else {
        out.insert(0, Line::code(format!("{s1}{s2}"), depth));
    }
    out
}
