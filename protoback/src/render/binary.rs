// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `--emit-binary` (spec 0429 S10): each file's output descriptor, as
//! reproto's second pass builds it (the `if outer_slot is not None:` block
//! of each `render`): the definition copied, then names rewritten, syntax
//! and features made the target's, and the children the text keeps rebuilt
//! in. Built once, reporting nothing: the text's render reported it all,
//! as reproto's muted re-renders do (S11).

use prototext_upb::descriptor_access::{
    DescriptorProto, DescriptorProtoMut, EnumDescriptorProtoMut, FieldDescriptorProtoMut,
    FileDescriptorProtoMut, ServiceDescriptorProtoMut,
};
use prototext_upb::DynamicMessage;

use super::file::Ctx;
use super::message::{field_binary, FieldBinary};
use super::sci::Location;
use super::{text, Def};
use crate::graph::Kind;

/// A copy of node `i`'s definition, to change.
fn copy(ctx: &Ctx<'_, '_>, i: usize) -> Option<DynamicMessage> {
    let m = match ctx.shared.def(i)? {
        Def::File(f) => f.0,
        Def::Message(m) => m.0,
        Def::Field(f) => f.0,
        Def::Enum(e) => e.0,
        Def::Service(s) => s.0,
        Def::Method(m) => m.0,
    };
    DynamicMessage::decode(m.descriptor(), &m.encode().ok()?).ok()
}

/// A child's bytes appended to a repeated message field, as reproto
/// appends the child's own descriptor.
fn append(
    target: Result<prototext_upb::MessageMut<'_>, prototext_upb::MessageError>,
    bytes: &[u8],
) {
    if let Ok(mut m) = target {
        let _ = m.merge_from(bytes);
    }
}

/// A type name's output one (spec 0440 S4).
fn rename(ctx: &Ctx<'_, '_>, name: &[u8]) -> String {
    let shared = ctx.shared;
    shared
        .variant
        .output_symbol(&text(name), shared.keep_variant_descriptor)
}

/// The file's descriptor: `locations`, the synthesized SourceCodeInfo.
pub fn file(ctx: &Ctx<'_, '_>, locations: &[Location]) -> Option<Vec<u8>> {
    let shared = ctx.shared;
    let f = shared.file_def(ctx.file)?;
    let mut out = copy(ctx, ctx.file)?;
    {
        let mut w = FileDescriptorProtoMut(out.as_mut());
        let keep = shared.keep_variant_descriptor;
        w.set_name(
            shared
                .variant
                .canonize_dependency(&text(f.name()), keep)
                .as_bytes(),
        )
        .ok()?;
        let deps: Vec<String> = f
            .dependency()
            .iter()
            .map(|d| shared.variant.canonize_dependency(&text(d), keep))
            .collect();
        // `import option`s: canonized alike; ordinary imports in a file
        // written as proto2, as the text has them.
        let option_deps: Vec<String> = f
            .option_dependency()
            .iter()
            .map(|d| shared.variant.canonize_dependency(&text(d), keep))
            .collect();
        w.clear_dependency();
        for d in &deps {
            w.push_dependency(d.as_bytes()).ok()?;
        }
        w.clear_option_dependency();
        for d in &option_deps {
            if ctx.editions_target() {
                w.push_option_dependency(d.as_bytes()).ok()?;
            } else if !deps.contains(d) {
                w.push_dependency(d.as_bytes()).ok()?;
            }
        }
        if !keep {
            // reproto assigns it but under --keep-descriptor-path: an
            // absent package comes out present and empty.
            w.set_package(
                shared
                    .variant
                    .output_package(&text(f.package()), &text(f.name()), keep)
                    .as_bytes(),
            )
            .ok()?;
        }
        w.clear_source_code_info();
        if !locations.is_empty() {
            let mut sci = w.source_code_info_mut().ok()?;
            for loc in locations {
                let mut l = sci.push_location().ok()?;
                for &p in &loc.path {
                    l.push_path(p).ok()?;
                }
                for &s in &loc.span {
                    l.push_span(s).ok()?;
                }
            }
        }
        w.clear_syntax();
        w.clear_edition();
        match ctx.target.as_str() {
            "proto3" => w.set_syntax(b"proto3").ok()?,
            "editions" => {
                w.set_syntax(b"editions").ok()?;
                w.set_edition(f.edition()).ok()?;
            }
            _ => {}
        }
        if f.has_options() && !ctx.editions_target() {
            w.options_mut().ok()?.clear_features();
        }
        w.clear_service();
        w.clear_enum_type();
        w.clear_message_type();
        w.clear_extension();
    }
    let mut services = Vec::new();
    for s in f.service() {
        if let Some(svc) = shared.child(Kind::Service, ctx.file, s.name()) {
            if shared.graph.nodes[svc].is_reachable {
                services.extend(service(ctx, svc));
            }
        }
    }
    let mut enums = Vec::new();
    for e in f.enum_type() {
        if let Some(en) = shared.child(Kind::Enum, ctx.file, e.name()) {
            if shared.graph.nodes[en].is_reachable {
                enums.extend(enumeration(ctx, en));
            }
        }
    }
    let mut messages = Vec::new();
    for m in f.message_type() {
        if let Some(msg) = shared.child(Kind::Message, ctx.file, m.name()) {
            let n = &shared.graph.nodes[msg];
            if !n.is_group && n.is_summoned {
                messages.extend(message(ctx, msg));
            }
        }
    }
    let mut extensions = Vec::new();
    for e in f.extension() {
        if let Some(fd) = shared.child(Kind::Field, ctx.file, e.name()) {
            if shared.graph.nodes[fd].is_summoned {
                extensions.extend(field(ctx, fd));
            }
        }
    }
    {
        let mut w = FileDescriptorProtoMut(out.as_mut());
        for b in &services {
            append(w.push_service().map(|m| m.0), b);
        }
        for b in &enums {
            append(w.push_enum_type().map(|m| m.0), b);
        }
        for b in &messages {
            append(w.push_message_type().map(|m| m.0), b);
        }
        for b in &extensions {
            append(w.push_extension().map(|m| m.0), b);
        }
    }
    out.encode().ok()
}

/// A message's descriptor: every field (but a malformed map's), every
/// nested message (groups, map entries and those not summoned too), enum
/// and extension; its and its oneofs' features gone but in editions.
fn message(ctx: &Ctx<'_, '_>, msg: usize) -> Option<Vec<u8>> {
    let shared = ctx.shared;
    let m = shared.message_def(msg)?;
    let mut out = copy(ctx, msg)?;
    {
        let mut w = DescriptorProtoMut(out.as_mut());
        if !ctx.editions_target() {
            w.clear_visibility();
            if m.has_options() {
                w.options_mut().ok()?.clear_features();
            }
            for (i, o) in m.oneof_decl().iter().enumerate() {
                if o.has_options() {
                    w.oneof_decl_mut(i)
                        .ok()?
                        .options_mut()
                        .ok()?
                        .clear_features();
                }
            }
        }
        w.clear_field();
        w.clear_nested_type();
        w.clear_enum_type();
        w.clear_extension();
    }
    // Written as anything but proto3, a proto3 message loses its synthetic
    // oneofs, its fields leaving them (spec 0429 S13).
    if ctx.target != "proto3" {
        let synthetic = super::message::synthetic_oneofs(ctx, msg, &m);
        if synthetic.iter().any(|&s| s) {
            let kept: Vec<Vec<u8>> = DescriptorProto(out.as_ref())
                .oneof_decl()
                .iter()
                .zip(&synthetic)
                .filter(|(_, &s)| !s)
                .filter_map(|(o, _)| o.0.encode().ok())
                .collect();
            let mut w = DescriptorProtoMut(out.as_mut());
            w.clear_oneof_decl();
            for o in &kept {
                append(w.push_oneof_decl().map(|o| o.0), o);
            }
        }
    }
    let fields: Vec<Vec<u8>> = m
        .field()
        .iter()
        .filter_map(|f| shared.child(Kind::Field, msg, f.name()))
        .filter_map(|fd| field(ctx, fd))
        .collect();
    let nested: Vec<Vec<u8>> = m
        .nested_type()
        .iter()
        .filter_map(|n| shared.child(Kind::Message, msg, n.name()))
        .filter_map(|n| message(ctx, n))
        .collect();
    let enums: Vec<Vec<u8>> = m
        .enum_type()
        .iter()
        .filter_map(|e| shared.child(Kind::Enum, msg, e.name()))
        .filter_map(|e| enumeration(ctx, e))
        .collect();
    let extensions: Vec<Vec<u8>> = m
        .extension()
        .iter()
        .filter_map(|f| shared.child(Kind::Field, msg, f.name()))
        .filter_map(|fd| field(ctx, fd))
        .collect();
    {
        let mut w = DescriptorProtoMut(out.as_mut());
        for b in &fields {
            append(w.push_field().map(|m| m.0), b);
        }
        for b in &nested {
            append(w.push_nested_type().map(|m| m.0), b);
        }
        for b in &enums {
            append(w.push_enum_type().map(|m| m.0), b);
        }
        for b in &extensions {
            append(w.push_extension().map(|m| m.0), b);
        }
    }
    out.encode().ok()
}

/// A field's descriptor, as reproto's second pass renders it: with
/// `is_oneof` false, whatever its oneof (the second pass passes none);
/// none for a malformed map, whose render returns before its descriptor.
fn field(ctx: &Ctx<'_, '_>, fd: usize) -> Option<Vec<u8>> {
    let FieldBinary {
        is_map,
        label,
        packed,
        keep_default,
    } = field_binary(ctx, fd)?;
    let Some(Def::Field(f)) = ctx.shared.def(fd) else {
        return None;
    };
    let mut out = copy(ctx, fd)?;
    {
        let mut w = FieldDescriptorProtoMut(out.as_mut());
        if !f.type_name().is_empty() {
            w.set_type_name(rename(ctx, f.type_name()).as_bytes())
                .ok()?;
        }
        if !f.extendee().is_empty() {
            w.set_extendee(rename(ctx, f.extendee()).as_bytes()).ok()?;
        }
        w.set_label(if is_map { 3 } else { label }).ok()?;
        // A proto3 `optional` written as anything but proto3 is proto2's:
        // no proto3_optional, out of its synthetic oneof, which its message
        // drops, the real oneofs renumbered (spec 0429 S13).
        if ctx.target != "proto3" && f.has_oneof_index() {
            if f.proto3_optional() {
                w.clear_proto3_optional();
                w.clear_oneof_index();
            } else if let Some(msg) = ctx.shared.graph.nodes[fd].parent {
                if let Some(m) = ctx.shared.message_def(msg) {
                    let synthetic = super::message::synthetic_oneofs(ctx, msg, &m);
                    let at = f.oneof_index() as usize;
                    let before = synthetic.iter().take(at).filter(|&&s| s).count();
                    w.set_oneof_index((at - before) as i32).ok()?;
                }
            }
        }
        let node = &ctx.shared.graph.nodes[fd];
        if let (Some(g), false) = (node.editions_group, ctx.editions_target()) {
            // The group's full name, one leading dot (spec 0429 S12: the
            // prefix has its own, which used to make `..pkg.M.G`).
            let name = format!(".{}", ctx.shared.graph.nodes[g].prefix.trim_matches('.'));
            w.set_type(10).ok()?;
            w.set_type_name(name.as_bytes()).ok()?;
        }
        if let Some(p) = packed {
            w.options_mut().ok()?.set_packed(p).ok()?;
        }
        if is_map || !keep_default {
            w.clear_default_value();
        }
        if w.as_ref().has_options() && !ctx.editions_target() {
            w.options_mut().ok()?.clear_features();
        }
    }
    out.encode().ok()
}

/// An enum's descriptor: every value, features gone but in editions.
fn enumeration(ctx: &Ctx<'_, '_>, en: usize) -> Option<Vec<u8>> {
    let Some(Def::Enum(e)) = ctx.shared.def(en) else {
        return None;
    };
    let mut out = copy(ctx, en)?;
    let editions = ctx.editions_target();
    let mut w = EnumDescriptorProtoMut(out.as_mut());
    if !editions {
        w.clear_visibility();
    }
    if e.has_options() && !editions {
        w.options_mut().ok()?.clear_features();
    }
    if !editions {
        for (i, v) in e.value().iter().enumerate() {
            if v.has_options() {
                w.value_mut(i).ok()?.options_mut().ok()?.clear_features();
            }
        }
    }
    out.encode().ok()
}

/// A service's descriptor: its reachable methods, their types renamed,
/// features gone but in editions.
fn service(ctx: &Ctx<'_, '_>, svc: usize) -> Option<Vec<u8>> {
    let shared = ctx.shared;
    let Some(Def::Service(s)) = shared.def(svc) else {
        return None;
    };
    let mut out = copy(ctx, svc)?;
    let editions = ctx.editions_target();
    let mut methods = Vec::new();
    for m in s.method() {
        let Some(meth) = shared.child(Kind::Method, svc, m.name()) else {
            continue;
        };
        if !shared.graph.nodes[meth].is_reachable {
            continue;
        }
        let Some(Def::Method(md)) = shared.def(meth) else {
            continue;
        };
        let mut mo = copy(ctx, meth)?;
        {
            let mut w = prototext_upb::descriptor_access::MethodDescriptorProtoMut(mo.as_mut());
            if !md.input_type().is_empty() {
                w.set_input_type(rename(ctx, md.input_type()).as_bytes())
                    .ok()?;
            }
            if !md.output_type().is_empty() {
                w.set_output_type(rename(ctx, md.output_type()).as_bytes())
                    .ok()?;
            }
            if md.has_options() && !editions {
                w.options_mut().ok()?.clear_features();
            }
        }
        methods.push(mo.encode().ok()?);
    }
    {
        let mut w = ServiceDescriptorProtoMut(out.as_mut());
        if s.has_options() && !editions {
            w.options_mut().ok()?.clear_features();
        }
        w.clear_method();
        for b in &methods {
            append(w.push_method().map(|m| m.0), b);
        }
    }
    out.encode().ok()
}
