// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! How a type is named where it is used (`utils.py`): `short_ref`, the
//! shortest name protoc resolves to the type from a scope, and
//! `shorten_type_name`, a method's argument types without their package.
//! Both apply the variant's namespace rules first, a rewritten name coming
//! out whole (spec 0429 S17 records why that is to change after the port).

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

/// `parse_fqdn`: an FQDN's prefix and reference (`desc:.pkg.M` →
/// `desc`, `.pkg.M`).
fn parse_fqdn(fqdn: &str) -> (&str, &str) {
    fqdn.split_once(':').unwrap_or(("", fqdn))
}

/// The reference a scope node's children are named under, as `short_ref`
/// takes it: a file's `.package` (empty without one), any other node's
/// own reference.
fn scope_ref(shared: &Shared<'_>, scope: usize) -> String {
    let node = &shared.graph.nodes[scope];
    if node.kind == crate::graph::Kind::File {
        let package = shared.file_package(scope);
        return if package.is_empty() {
            String::new()
        } else {
            format!(".{package}")
        };
    }
    parse_fqdn(&node.fqdn).1.to_string()
}

/// `short_ref` for a type named by its FQDN, from `scope`, a node.
///
/// The collision test looks the candidate up under the reference's own
/// prefix: an extendee comes as `message:…`, a prefix no node has, so its
/// candidates never collide; a message never collides with an enum of the
/// same name. Both as reproto does.
pub fn short_ref(shared: &Shared<'_>, fqdn: &str, scope: usize) -> String {
    let (d_prefix, d_ref) = parse_fqdn(fqdn);
    if !shared.keep_variant_descriptor {
        let rewritten = shared.variant.namespace(d_ref);
        if rewritten != d_ref {
            return rewritten;
        }
    }
    let r_parts: Vec<&str> = d_ref.split('.').collect();
    let r_len = r_parts.len();
    let s_ref = scope_ref(shared, scope);
    let l_parts: Vec<&str> = s_ref.split('.').collect();
    let l_len = l_parts.len();

    let mut found = String::new();
    for r_ndx in 1..r_len {
        let right = r_parts[r_len - r_ndx..].join(".");
        let mut collision = false;
        for l_ndx in (1..=l_len).rev() {
            let left = l_parts[..l_ndx].join(".");
            let path = format!("{left}.{right}");
            if path == d_ref {
                found = right.clone();
                break;
            }
            if shared.graph.find(&format!("{d_prefix}:{path}")).is_some() {
                collision = true;
                break;
            }
            // protoc's "first component collision": the shortened name's
            // first component is the scope's next element.
            if l_ndx < l_len && r_parts[r_len - r_ndx] == l_parts[l_ndx] {
                collision = true;
                break;
            }
        }
        if !found.is_empty() && !collision {
            break;
        }
        found.clear();
    }
    if found.is_empty() {
        d_ref.to_string()
    } else {
        found
    }
}

/// `shorten_type_name` for a method's input or output type, `name`, from
/// its service, a child of `file`: the file's package stripped, a
/// `.google.protobuf.` type written without its leading dot.
pub fn shorten_method_type(shared: &Shared<'_>, file: usize, name: &str) -> String {
    if !shared.keep_variant_descriptor {
        let rewritten = shared.variant.namespace(name);
        if rewritten != name {
            return rewritten;
        }
    }
    if let Some(rest) = name.strip_prefix(".google.protobuf.") {
        return format!("google.protobuf.{rest}");
    }
    let package = shared.file_package(file);
    let short = if package.is_empty() {
        name.to_string()
    } else if let Some(rest) = name.strip_prefix(&format!(".{package}.")) {
        format!(".{rest}")
    } else {
        name.to_string()
    };
    if short == name {
        short
    } else {
        short[1..].to_string()
    }
}
