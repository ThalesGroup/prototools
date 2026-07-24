// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

use super::*;

use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};
use prost_reflect::Cardinality;

impl App {
    /// Recursively collect every current descendant of `idx` (any depth),
    /// via `first_child`/`next_sibling` pointer traversal — never array
    /// position (spec 0114 §5's splice design: post-order array
    /// contiguity does not survive a *second* override of the same node,
    /// since the first override's new nodes are appended at the array's
    /// end, breaking it). Used to find which array entries become orphans
    /// once `idx`'s subtree is replaced, so they can be scrubbed from
    /// `self.folded`.
    pub(super) fn collect_descendants(&self, idx: usize, out: &mut Vec<usize>) {
        let mut child = self.tree[idx].first_child;
        while let Some(c) = child {
            out.push(c);
            self.collect_descendants(c, out);
            child = self.tree[c].next_sibling;
        }
    }

    /// Looks up `idx`'s own field on its parent's schema (spec 0119
    /// §G1/§G2's shared lookup): requires both that `idx`'s parent has a
    /// resolved `type_fqdn` and that its schema declares `idx`'s
    /// `field_number`. Returns `None` when either fails (no parent,
    /// unresolved parent type, or the field isn't declared) — the same
    /// failure mode `natural_type`/`field_name_for` both fall back from.
    pub(super) fn parent_field(&self, idx: usize) -> Option<prost_reflect::FieldDescriptor> {
        let parent = self.tree[idx].parent?;
        let fqdn = self.tree[parent].span.type_fqdn.as_ref()?;
        let field_number = self.tree[idx].span.field_number;
        self.ctx
            .pool()
            .get_message_by_name(fqdn)?
            .get_field(field_number as u32)
    }

    /// The type `idx` would naturally have from its parent's schema, used
    /// as the fallback when no active override applies (spec 0119 §G1) —
    /// `None` only when genuinely no type information is available at all
    /// (no parent schema, or field not declared).
    ///
    /// Every primitive `Kind` (spec 0135 follow-up, 2026-07-17: found
    /// post-implementation) resolves to its own `:type-as` keyword, not
    /// `None`: since G3 widened `can_override` to plain scalar leaves, a
    /// primitive field with no active override is a real, reachable case
    /// here (e.g. pressing `t` then `Esc` on a plain `int32` field) —
    /// `None` would make `resettle_node` fall back to raw, wrongly
    /// discarding the field's own natural decoding, not just "no override
    /// applies." Message fields already relied on this same "natural
    /// type, not raw" fallback since spec 0119; primitives now get the
    /// same treatment via `primitive_type_for_keyword`'s reverse mapping.
    ///
    /// `Kind::Enum` resolves to its own FQDN (2026-07-18 fix): previously
    /// excluded here (`None`, spec 0135's original Non-goals, predating
    /// spec 0137's enum splicing support in `splice_override`), which made
    /// `resettle_node` demote every enum-typed field to a raw record dump
    /// the moment it was directly resettled (e.g. opening the override
    /// pane with `t` then cancelling with `Esc`) — permanently, since no
    /// other pass ever revisits a plain scalar leaf once `rendered_as` is
    /// set (see `render_overrides`'s recursion-gating comment below).
    pub(super) fn natural_type(&self, idx: usize) -> Option<String> {
        use prost_reflect::Kind;
        match self.parent_field(idx)?.kind() {
            Kind::Message(desc) => Some(desc.full_name().to_string()),
            Kind::Enum(desc) => Some(desc.full_name().to_string()),
            Kind::Double => Some("double".to_string()),
            Kind::Float => Some("float".to_string()),
            Kind::Int32 => Some("int32".to_string()),
            Kind::Int64 => Some("int64".to_string()),
            Kind::Uint32 => Some("uint32".to_string()),
            Kind::Uint64 => Some("uint64".to_string()),
            Kind::Sint32 => Some("sint32".to_string()),
            Kind::Sint64 => Some("sint64".to_string()),
            Kind::Fixed32 => Some("fixed32".to_string()),
            Kind::Fixed64 => Some("fixed64".to_string()),
            Kind::Sfixed32 => Some("sfixed32".to_string()),
            Kind::Sfixed64 => Some("sfixed64".to_string()),
            Kind::Bool => Some("bool".to_string()),
            Kind::String => Some("string".to_string()),
            Kind::Bytes => Some("bytes".to_string()),
        }
    }

    /// `idx`'s own active override target for field number `field`, tried
    /// in priority order (spec 0156 G6c tiers 1–2): an active `PathField`
    /// override whose `path` is exactly `idx`'s own positional path, else
    /// an active `FqdnField` override whose `fqdn` matches `idx`'s own
    /// resolved `type_fqdn` (only tried when that resolves). `Path`-kind
    /// overrides are never consulted here — see spec 0156 N4.
    pub(super) fn own_field_override(&self, idx: usize, field: u64) -> Option<Option<String>> {
        let path = self.positional_path(idx);
        let path_field = self
            .overrides
            .entries()
            .iter()
            .find_map(|e| match &e.origin {
                OverrideOrigin::PathField { path: p, field: f }
                    if e.active && *p == path && *f == field =>
                {
                    Some(e.r#type.clone())
                }
                _ => None,
            });
        path_field.or_else(|| {
            let fqdn = self.tree[idx].span.type_fqdn.as_deref()?;
            self.overrides
                .entries()
                .iter()
                .find_map(|e| match &e.origin {
                    OverrideOrigin::FqdnField {
                        fqdn: f,
                        field: fld,
                    } if e.active && f == fqdn && *fld == field => Some(e.r#type.clone()),
                    _ => None,
                })
        })
    }

    /// Builds the `ResolvedField` list for `export --descriptor-*` (spec
    /// 0156 G6c): one entry per distinct field_number among `idx`'s
    /// direct live-tree children, typed via `own_field_override` (tiers
    /// 1–2), the child's schema field (tier 3), or a wire-type-derived
    /// primitive guess (tier 4). `Err` only for an untyped
    /// `WT_START_GROUP` field, or an override target that doesn't
    /// resolve in `ctx.pool()`.
    pub(super) fn resolve_export_fields(
        &self,
        idx: usize,
    ) -> Result<Vec<export_descriptor::ResolvedField>, String> {
        use prototext_core::helpers::{WT_I32, WT_I64, WT_LEN, WT_START_GROUP, WT_VARINT};

        // Group direct children by field_number, keyed off the first
        // child seen for each number (G6c).
        struct Group {
            field_number: u64,
            first_child: usize,
            count: usize,
        }
        let mut groups: Vec<Group> = Vec::new();
        let mut child = self.tree[idx].first_child;
        while let Some(c) = child {
            let field_number = self.tree[c].span.field_number;
            match groups.iter_mut().find(|g| g.field_number == field_number) {
                Some(g) => g.count += 1,
                None => groups.push(Group {
                    field_number,
                    first_child: c,
                    count: 1,
                }),
            }
            child = self.tree[c].next_sibling;
        }

        let mut fields = Vec::with_capacity(groups.len());
        for g in groups {
            let child = g.first_child;
            let name = export_descriptor::synthetic_field_name(&self.field_name_for(child));
            let parent_field_desc = self.parent_field(child);
            let is_repeated = parent_field_desc
                .as_ref()
                .map(|f| f.cardinality() == Cardinality::Repeated)
                .unwrap_or(false)
                || g.count > 1;
            let label = if is_repeated {
                Label::Repeated
            } else {
                Label::Optional
            };

            let (r#type, type_name, referenced_file) =
                match self.own_field_override(idx, g.field_number) {
                    // Tiers 1-2: an active PathField/FqdnField override.
                    Some(None) => (Type::Bytes, None, None),
                    Some(Some(fqdn)) => {
                        if let Some(desc) = self.ctx.pool().get_message_by_name(&fqdn) {
                            (
                                Type::Message,
                                Some(format!(".{fqdn}")),
                                Some(desc.parent_file()),
                            )
                        } else if let Some(desc) = self.ctx.pool().get_enum_by_name(&fqdn) {
                            (
                                Type::Enum,
                                Some(format!(".{fqdn}")),
                                Some(desc.parent_file()),
                            )
                        } else {
                            return Err(format!(
                                "export --descriptor: override target '{fqdn}' not found"
                            ));
                        }
                    }
                    // Tier 3: the child's own schema field (`natural_type`'s
                    // resolution, kept as a `Kind` here so the message/enum
                    // descriptor's own file is available for G6d's closure).
                    None => match parent_field_desc.map(|f| f.kind()) {
                        Some(prost_reflect::Kind::Message(desc)) => (
                            Type::Message,
                            Some(format!(".{}", desc.full_name())),
                            Some(desc.parent_file()),
                        ),
                        Some(prost_reflect::Kind::Enum(desc)) => (
                            Type::Enum,
                            Some(format!(".{}", desc.full_name())),
                            Some(desc.parent_file()),
                        ),
                        Some(prost_reflect::Kind::Double) => (Type::Double, None, None),
                        Some(prost_reflect::Kind::Float) => (Type::Float, None, None),
                        Some(prost_reflect::Kind::Int32) => (Type::Int32, None, None),
                        Some(prost_reflect::Kind::Int64) => (Type::Int64, None, None),
                        Some(prost_reflect::Kind::Uint32) => (Type::Uint32, None, None),
                        Some(prost_reflect::Kind::Uint64) => (Type::Uint64, None, None),
                        Some(prost_reflect::Kind::Sint32) => (Type::Sint32, None, None),
                        Some(prost_reflect::Kind::Sint64) => (Type::Sint64, None, None),
                        Some(prost_reflect::Kind::Fixed32) => (Type::Fixed32, None, None),
                        Some(prost_reflect::Kind::Fixed64) => (Type::Fixed64, None, None),
                        Some(prost_reflect::Kind::Sfixed32) => (Type::Sfixed32, None, None),
                        Some(prost_reflect::Kind::Sfixed64) => (Type::Sfixed64, None, None),
                        Some(prost_reflect::Kind::Bool) => (Type::Bool, None, None),
                        Some(prost_reflect::Kind::String) => (Type::String, None, None),
                        Some(prost_reflect::Kind::Bytes) => (Type::Bytes, None, None),
                        // Tier 4: no override, no schema — guess from the
                        // (first) child's own effective wire type (a
                        // packed-record child's effective wire type is its
                        // reconstructed WT_LEN framing).
                        None => {
                            let span = &self.tree[child].span;
                            let wire_type = if span.packed_record_start.is_some() {
                                WT_LEN
                            } else {
                                span.wire_type
                            };
                            match wire_type {
                                WT_VARINT => (Type::Int64, None, None),
                                WT_I32 => (Type::Fixed32, None, None),
                                WT_I64 => (Type::Fixed64, None, None),
                                WT_LEN => (Type::Bytes, None, None),
                                WT_START_GROUP => {
                                    return Err(format!(
                                        "export --descriptor: field {} is an untyped \
                                         group — not supported",
                                        g.field_number
                                    ));
                                }
                                _ => (Type::Bytes, None, None),
                            }
                        }
                    },
                };

            fields.push(export_descriptor::ResolvedField {
                number: g.field_number,
                name,
                label,
                r#type,
                type_name,
                referenced_file,
            });
        }

        Ok(fields)
    }

    /// Status-line "type:" fragment for `idx` (spec 0136): the field's
    /// *currently effective* proto type — an active `:type-as` override
    /// if one applies, else the natural (schema-declared) type, enum
    /// included (`natural_type`). `None` when nothing is resolvable at
    /// all, or when the pinned `<raw / no type>` override entry is
    /// explicitly active — either case means the status line shows no
    /// fragment. The second tuple element is the `message`/`group`/
    /// `enum` tag (2026-07-19 feedback: shown only in full-width mode,
    /// via `render.rs`), `None` for a bare primitive keyword.
    pub(super) fn status_type_label(&self, idx: usize) -> Option<(String, Option<&'static str>)> {
        let span = &self.tree[idx].span;
        if span.is_message {
            // `resettle_node` keeps `span.type_fqdn`/`is_message` in sync
            // with the currently effective override on every render pass
            // — no separate override lookup needed for this branch.
            let fqdn = span.type_fqdn.as_ref()?;
            let tag = if span.wire_type == prototext_core::helpers::WT_START_GROUP {
                "group"
            } else {
                "message"
            };
            // The internal, globally-shared MessageSet Item FQDN
            // (`decode::MESSAGE_SET_ITEM_FQDN`) is never shown to the
            // user directly — show the friendly, MessageSet-specific
            // FQDN instead (2026-07-18 feedback item 4).
            if fqdn == decode::MESSAGE_SET_ITEM_FQDN {
                if let Some(display) = self.message_set_item_display_fqdn(idx) {
                    return Some((format_fqdn_label(&display), Some(tag)));
                }
            }
            return Some((format_fqdn_label(fqdn), Some(tag)));
        }
        let effective = match self.resolve_active_override(idx) {
            Some(inner) => inner?,
            None => self.natural_type(idx)?,
        };
        if decode::primitive_type_for_keyword(&effective).is_some() {
            return Some((effective, None));
        }
        // Not a primitive keyword: the only other possibility reaching
        // this branch is an enum FQDN from `natural_type` (an active
        // override can never resolve to a message FQDN here — see the
        // `is_message` branch above).
        Some((format_fqdn_label(&effective), Some("enum")))
    }

    /// `true` when `idx`'s resolved type is `google.protobuf.Any` — spec
    /// 0120 §G1's detection rule, a plain FQDN match (per review).
    pub(super) fn is_any_typed(&self, idx: usize) -> bool {
        self.tree[idx].span.type_fqdn.as_deref() == Some("google.protobuf.Any")
    }

    /// `true` when `idx`'s resolved type is a MessageSet — spec 0120 §G2's
    /// detection rule: `message_set_wire_format = true` in the resolved
    /// `MessageDescriptor`'s own options, and zero declared fields. Mirrors
    /// `prototext-core`'s own (private, unreachable from this crate)
    /// `is_message_set` heuristic — an independent replica, not a shared
    /// helper, since protolens already has direct `prost_reflect`/
    /// `ctx.pool()` access and needs no new plumbing (spec 0120's
    /// assessment).
    pub(super) fn is_message_set_typed(&self, idx: usize) -> bool {
        let Some(fqdn) = self.tree[idx].span.type_fqdn.as_ref() else {
            return false;
        };
        let Some(desc) = self.ctx.pool().get_message_by_name(fqdn) else {
            return false;
        };
        let msf = desc
            .descriptor_proto()
            .options
            .as_ref()
            .and_then(|o| o.message_set_wire_format)
            .unwrap_or(false);
        msf && desc.fields().count() == 0
    }

    /// The friendly, MessageSet-specific FQDN to show in place of the
    /// internal, globally-shared `decode::MESSAGE_SET_ITEM_FQDN`
    /// wherever `idx` (a tier-1 Item wrapper node) is displayed to the
    /// user — `None` if `idx`'s parent isn't actually a MessageSet
    /// container (shouldn't happen for a real Item node, but keeps this
    /// a safe fallback rather than a panic). Display-only: never stored
    /// on a tree node or an override entry (2026-07-18 feedback item 4).
    pub(super) fn message_set_item_display_fqdn(&self, idx: usize) -> Option<String> {
        let parent = self.tree[idx].parent?;
        let message_set_fqdn = self.tree[parent].span.type_fqdn.as_deref()?;
        Some(decode::message_set_item_display_fqdn(message_set_fqdn))
    }

    /// The sibling of `idx` (another child of `idx`'s own parent) whose
    /// `field_number` is `field_number`, if any — used by
    /// `auto_expand_type` to locate Any's `type_url` next to `value`, and
    /// MessageSet's `type_id` next to `message`.
    pub(super) fn find_sibling(&self, idx: usize, field_number: u64) -> Option<usize> {
        let parent = self.tree[idx].parent?;
        let mut c = self.tree[parent].first_child;
        while let Some(ci) = c {
            if self.tree[ci].span.field_number == field_number {
                return Some(ci);
            }
            c = self.tree[ci].next_sibling;
        }
        None
    }

    /// Reads `idx`'s own raw payload (tag/length stripped, per
    /// `extract::message_payload_range`) as a UTF-8 string — used to read
    /// Any's `type_url` value directly off the wire, independent of how
    /// (or whether) it's currently rendered.
    pub(super) fn read_string_field(&self, idx: usize) -> Option<String> {
        let span = &self.tree[idx].span;
        let payload =
            extract::message_payload_range(&self.blob, &span.raw_range, span.packed_record_start);
        String::from_utf8(self.blob[payload].to_vec()).ok()
    }

    /// Reads `idx`'s own raw payload as a varint — used to read
    /// MessageSet's `type_id` value directly off the wire.
    pub(super) fn read_varint_field(&self, idx: usize) -> Option<u64> {
        let span = &self.tree[idx].span;
        let payload =
            extract::message_payload_range(&self.blob, &span.raw_range, span.packed_record_start);
        prototext_core::helpers::parse_varint(&self.blob, payload.start).varint
    }

    /// `true` when `idx` is structurally *eligible* for Any/MessageSet
    /// auto-expansion (spec 0120) — regardless of whether the actual
    /// target type turns out to be resolvable. Used by `render_overrides`
    /// to widen its child-recursion gate (normally `span.is_message`
    /// only) just enough to give these two specific field shapes a
    /// chance to be visited and auto-overridden, without reopening the
    /// spec 0119 bug where every plain scalar LEN-wire field got
    /// incorrectly demoted to raw by being recursed into at all.
    pub(super) fn is_auto_expand_candidate(&self, idx: usize) -> bool {
        let Some(parent) = self.tree[idx].parent else {
            return false;
        };
        let field_number = self.tree[idx].span.field_number;
        if field_number == 2 && self.is_any_typed(parent) {
            return true;
        }
        // MessageSet tier 1 (the "Item" group wrapper itself) needs no
        // entry here: it's already `is_message == true` naturally (a
        // real decoded group), so `render_overrides`'s own `is_message`
        // half of its recursion gate already reaches it.
        if field_number == 3
            && self.tree[parent].span.type_fqdn.as_deref() == Some(decode::MESSAGE_SET_ITEM_FQDN)
        {
            if let Some(grandparent) = self.tree[parent].parent {
                return self.is_message_set_typed(grandparent);
            }
        }
        false
    }

    /// The Any/MessageSet auto-derived type for `idx`, if `idx` is one of
    /// the two eligible field shapes (spec 0120 §G1/§G2) and the type it
    /// points at is resolvable in `ctx.pool()` — `None` otherwise (either
    /// not an eligible shape, or an unresolvable `type_url`/`type_id`,
    /// both of which fall back to plain raw rendering like any other
    /// unresolvable type). Consulted as a fallback tier between an
    /// explicit active override and `natural_type` in `render_overrides`.
    pub(super) fn auto_expand_type(&mut self, idx: usize) -> Option<String> {
        let parent = self.tree[idx].parent?;
        let field_number = self.tree[idx].span.field_number;

        // Any's `value` (field 2): FQDN read from the sibling `type_url`
        // (field 1), stripped of any leading `.../` host/prefix segment
        // (mirrors `any_field.rs`'s own `rfind('/')` resolution).
        if field_number == 2 && self.is_any_typed(parent) {
            let type_url_idx = self.find_sibling(idx, 1)?;
            let type_url = self.read_string_field(type_url_idx)?;
            let fqdn = match type_url.rfind('/') {
                Some(slash) => &type_url[slash + 1..],
                None => type_url.as_str(),
            };
            return self
                .ctx
                .pool()
                .get_message_by_name(fqdn)
                .map(|d| d.full_name().to_string());
        }

        // MessageSet tier 1: the "Item" group wrapper (field 1,
        // `WT_START_GROUP`) auto-derives to the synthetic, globally
        // shared `protolens_internal.Item` shape (`type_id` +
        // `message`) — registered once per pool and reused across every
        // MessageSet occurrence in the document.
        if field_number == 1
            && self.tree[idx].span.wire_type == prototext_core::helpers::WT_START_GROUP
            && self.is_message_set_typed(parent)
        {
            return decode::register_message_set_item(self.ctx.pool_mut())
                .ok()
                .map(|d| d.full_name().to_string());
        }

        // MessageSet tier 2: "message" (field 3) of an Item already
        // retyped (tier 1) to `protolens_internal.Item` — extension type
        // resolved from the sibling `type_id` (field 2), keyed against
        // the MessageSet container's (idx's grandparent) own extensions.
        if field_number == 3
            && self.tree[parent].span.type_fqdn.as_deref() == Some(decode::MESSAGE_SET_ITEM_FQDN)
        {
            let grandparent = self.tree[parent].parent?;
            if self.is_message_set_typed(grandparent) {
                let type_id_idx = self.find_sibling(idx, 2)?;
                let type_id = self.read_varint_field(type_id_idx)?;
                let grandparent_fqdn = self.tree[grandparent].span.type_fqdn.clone()?;
                let extendee = self.ctx.pool().get_message_by_name(&grandparent_fqdn)?;
                let ext = extendee.get_extension(type_id as u32)?;
                if let prost_reflect::Kind::Message(inner) = ext.kind() {
                    return Some(inner.full_name().to_string());
                }
            }
        }

        None
    }

    /// The display name to use for `idx`'s synthetic wrapper field in
    /// `splice_override` (spec 0119 §G2, extended by §G4): the resolved
    /// active override entry's own `name` override when set (§G4 takes
    /// priority); otherwise `idx`'s real field name when resolvable from
    /// the parent's schema; otherwise `idx`'s field number as a string
    /// (protobuf field names can never be all-digits, so this can't
    /// collide with a real name) — the document root is not special-
    /// cased: it's always field number 1 of the virtual encompassing
    /// message, so it falls through to this same field-number case.
    pub(super) fn field_name_for(&self, idx: usize) -> String {
        if let Some(name) = self
            .resolve_active_override_entry(idx)
            .and_then(|e| e.name.clone())
        {
            return name;
        }
        if let Some(field) = self.parent_field(idx) {
            field.name().to_string()
        } else {
            self.tree[idx].span.field_number.to_string()
        }
    }

    /// Resolves `idx`'s applicable override entry, per the priority
    /// `Path > PathField > FqdnField` (spec 0117), or `None` when no
    /// active entry applies at all — spec 0118 §2. Only `active` entries
    /// are considered (at most one active entry per origin, per spec
    /// 0117's invariant). Shared by `resolve_active_override` (the
    /// entry's `r#type`) and `field_name_for` (spec 0119 §G4's `name`).
    pub(super) fn resolve_active_override_entry(
        &self,
        idx: usize,
    ) -> Option<&override_pane::OverrideEntry> {
        self.resolve_active_override_entry_index(idx)
            .map(|i| &self.overrides.entries()[i])
    }

    /// Same resolution as `resolve_active_override_entry` (priority
    /// order `Path` > `PathField` > `FqdnField`, active entries only),
    /// but returns the entry's index into `self.overrides.entries()`
    /// rather than a reference — needed wherever a caller must then
    /// place a cursor/highlight on the entry itself (e.g. the manage
    /// pane's `o`-key cursor placement), not just read its `r#type`.
    pub(super) fn resolve_active_override_entry_index(&self, idx: usize) -> Option<usize> {
        let path = self.positional_path(idx);
        if let Some(pos) = self.overrides.entries().iter().position(|e| {
            e.active && matches!(&e.origin, OverrideOrigin::Path { path: p } if *p == path)
        }) {
            return Some(pos);
        }
        let parent = self.tree[idx].parent?;
        let field = self.tree[idx].span.field_number;
        let parent_path = self.positional_path(parent);
        if let Some(pos) = self.overrides.entries().iter().position(|e| {
            e.active
                && matches!(&e.origin, OverrideOrigin::PathField { path: p, field: f }
                    if *p == parent_path && *f == field)
        }) {
            return Some(pos);
        }
        let fqdn = self.tree[parent].span.type_fqdn.as_ref()?;
        self.overrides.entries().iter().position(|e| {
            e.active
                && matches!(&e.origin, OverrideOrigin::FqdnField { fqdn: f, field: fld }
                    if f == fqdn && *fld == field)
        })
    }

    /// Resolves to the type (or `None` = raw) that should currently be
    /// used to render `idx`'s payload, or the outer `None` when no active
    /// override applies at all — spec 0118 §2.
    pub(super) fn resolve_active_override(&self, idx: usize) -> Option<Option<String>> {
        self.resolve_active_override_entry(idx)
            .map(|e| e.r#type.clone())
    }

    /// Spec 0132 §G3: settles `idx`'s main-pane rendering to its current
    /// "effective" override target (`resolve_active_override`'s
    /// explicit type if one is active, else `natural_type(idx)` when
    /// nothing is active at all) — splicing only if it doesn't already
    /// match `self.tree[idx].rendered_as` (the same no-op-when-already-
    /// current guard `render_overrides` always used, verbatim). Factored
    /// out of `render_overrides` itself (which calls this for `idx`
    /// before recursing into children) so the override-pane's live-
    /// preview revert (on close/cancel) can reuse the exact same
    /// "effective type" computation — including the natural-type
    /// fallback a plain `resolve_active_override_entry`-only revert
    /// would get wrong.
    ///
    /// 2026-07-17: no longer demotes a stale `auto` entry whose
    /// ancestor context has since changed (spec 0120's original
    /// design) — `auto`/`manual` is provenance only (how an entry was
    /// created, shown via `manage_entry_style`), and must have no
    /// effect on whether an *active* entry actually applies. An
    /// active entry, auto-derived or not, applies exactly as long as
    /// its path still resolves to a live node — the same fallback
    /// `splice_override` already relies on for a manual override that
    /// no longer cleanly matches its target (a `TYPE_MISMATCH`-style
    /// annotation, not a silent revert to raw).
    /// Returns `true` iff `idx` was actually re-spliced (i.e. `target`
    /// disagreed with `idx`'s currently-rendered provenance and
    /// `splice_override` succeeded) — spec 0160 G2: callers use this to
    /// distinguish "my own node was just re-spliced, so my children are
    /// freshly positioned and need no further correction" from "my own
    /// node was untouched, so my *existing* children still owe whatever
    /// correction I was just given". This can't be inferred from
    /// `pending_shift` alone, since a splice whose new content happens
    /// to have the same line count as the old (`delta == 0`) must still
    /// count as fresh.
    pub(super) fn resettle_node(&mut self, idx: usize) -> bool {
        let target = self.resolve_active_override(idx);
        let field_name = self.field_name_for(idx);
        let current = Some((target.clone(), field_name));
        if current != self.tree[idx].rendered_as {
            let effective = match &target {
                Some(explicit) => explicit.clone(),
                None => self.natural_type(idx),
            };
            match self.splice_override(idx, effective) {
                Ok(()) => {
                    self.tree[idx].rendered_as = current;
                    true
                }
                Err(e) => {
                    self.message = format!("cannot apply override: {e}");
                    false
                }
            }
        } else {
            false
        }
    }

    /// Applies the winning fqdn from the async root-type resolver (spec
    /// NNNN) to the document root — the deferred counterpart of
    /// `App::new`'s `overrides.seed_root` + real-wrapper-descriptor
    /// construction for the synchronous case. Seeding then delegating to
    /// `render_overrides` (rather than a bespoke splice) reuses the exact
    /// same mismatch-detection/splice/recurse path any other override
    /// activation already goes through — including seeding Any/
    /// MessageSet auto-expansion on the freshly-typed children
    /// `splice_override` builds, and `splice_override`'s own `line_to_
    /// node`/`rebuild_visible_rows` bookkeeping. A no-op if the user
    /// already made their own choice for the root (`t`, a `type-as`
    /// command, or explicitly confirming raw) by the time this lands —
    /// an existing entry for the root's own origin always wins over the
    /// worker's answer.
    pub(super) fn apply_resolved_root_type(&mut self, fqdn: String) {
        if self
            .resolve_active_override_entry(self.first_node)
            .is_some()
        {
            return;
        }
        self.overrides.seed_root(Some(fqdn));
        self.render_overrides(self.first_node);
    }

    /// Whether `entry` (assumed `auto == true`) would still be re-derived
    /// with the same `r#type` if `render_overrides` visited its node
    /// again right now — i.e. it is still "in scope" (spec 0125 §G2).
    /// Sole remaining use (2026-07-17, since `resettle_node` dropped its
    /// own demotion check): `handle_manage_key`'s `Delete`/`Backspace`
    /// handling, which still needs to distinguish "deleting this would
    /// just make the next `render_overrides` pass re-seed an identical
    /// entry" (still in scope) from "deleting this is final" (out of
    /// scope). Lives on `App` (not `OverrideCollection`) because it
    /// needs `auto_expand_type`, which resolves against the live tree/
    /// descriptor pool, not just the override collection itself.
    /// Auto-seeded entries only ever have a `Path` origin
    /// (`render_overrides` always calls `activate_auto` with
    /// `OverrideOrigin::Path`), so a single `resolve_path` lookup
    /// suffices.
    pub(super) fn auto_entry_in_scope(&mut self, entry: &override_pane::OverrideEntry) -> bool {
        let OverrideOrigin::Path { path } = &entry.origin else {
            return false;
        };
        let Some(idx) = self.resolve_path(path) else {
            return false;
        };
        self.auto_expand_type(idx) == entry.r#type
    }

    /// Recursive override-driven rendering pass (spec 0118 §3): resolves
    /// `idx`'s applicable override and splices a fresh render whenever the
    /// resolved target no longer matches what's currently displayed
    /// (`TreeNode::rendered_as`, spec 0118 §2.1) — comparing against
    /// provenance, not just "is there an override right now?", is what
    /// correctly detects a demotion (an override that used to apply no
    /// longer does), not just fresh promotions/retypes.
    ///
    /// Any/MessageSet auto-expansion (spec 0120) is seeded as a real,
    /// persisted `OverrideEntry` (`OverrideOrigin::Path`) the first time
    /// `idx` is visited with *no entry at all yet existing* for its path —
    /// checked via `self.overrides.entries()`, not via
    /// `resolve_active_override`: the latter can't distinguish "never
    /// seeded" from "user explicitly deactivated the seeded entry", and
    /// naively re-seeding (calling `activate` again) on every subsequent
    /// pass would both silently resurrect a deactivation the user just
    /// made in the manage pane, and — since `activate` unconditionally
    /// resorts the entries list — reshuffle `manage_highlight`'s raw index
    /// out from under the very keypress that triggered this pass. Once
    /// truly first-seeded, `auto_expand_type(idx)` computes the derived
    /// type, `self.overrides.activate` records it, and — because this
    /// happens *before* `target`/`current` are computed below — the very
    /// same pass's `resolve_active_override` already sees it, so no
    /// separate fallback tier is needed in the splice logic itself. This
    /// makes the derived type a real, visible, user-editable/removable
    /// entry in the override management pane (rather than a silent
    /// dynamic fallback), and means every subsequent pass resolves it via
    /// the ordinary entries scan instead of re-deriving it from the wire
    /// each time. When no active override applies at all after seeding
    /// (`target == None`, e.g. the type wasn't resolvable, or the user
    /// deactivated it), the effective splice target falls back to
    /// `natural_type(idx)` — `idx`'s inherited type from its parent's
    /// schema. That fallback never fires when an active entry explicitly
    /// says raw (`target == Some(None)`), which still renders raw, since
    /// that's an explicit user choice. The *outer* `Option` of `target` is
    /// still what gets stored into `rendered_as`, preserving the
    /// provenance distinction for the next pass — paired with
    /// `field_name_for(idx)` (spec 0119 §G4): either half changing (a
    /// retype, or a name-only rename of the governing entry) is enough to
    /// trigger a re-splice, since both feed directly into the rendered
    /// text.
    ///
    /// Named `render_overrides` (not `render`) to avoid colliding with the
    /// unrelated `render(&mut self, frame: &mut Frame)` ratatui draw
    /// method below.
    pub(super) fn render_overrides(&mut self, idx: usize) {
        self.override_batch_depth += 1;
        self.render_overrides_inner(idx, 0);
        self.override_batch_depth -= 1;
        if self.override_batch_depth == 0 {
            self.finalize_override_batch(idx);
        }
    }

    /// Spec 0160 G2: a small private helper applying `delta` to both
    /// endpoints of `span.text_range` — the `(x as isize + delta) as
    /// usize` pattern used at several call sites in this file, factored
    /// out.
    fn shift_span(span: &mut prototext_core::serialize::render_text::NodeSpan, delta: isize) {
        span.text_range.start = (span.text_range.start as isize + delta) as usize;
        span.text_range.end = (span.text_range.end as isize + delta) as usize;
    }

    /// The actual (self-recursive) body of `render_overrides` (spec 0160
    /// G2): `inherited` is the line-count correction owed to `idx`'s own
    /// `span.text_range` by splices that already happened earlier in
    /// this batch, applied here directly, once — then, unless `idx`
    /// itself was just re-spliced (in which case its freshly decoded
    /// children are already positioned correctly), the same correction
    /// is carried down to each of `idx`'s existing children, accumulating
    /// each already-processed child's own growth onto what's owed to its
    /// later siblings.
    fn render_overrides_inner(&mut self, idx: usize, inherited: isize) {
        // `idx`'s own node — and, if it's a member of a packed-repeated
        // run (spec 0135 G1), every other member of that run too, since
        // `packed_record_extent` (inside `splice_override`, called from
        // `resettle_node` below) reads the run's first and last members
        // directly and can't wait for their own turn later in this
        // function's child loop — needs `inherited` applied now, once,
        // directly. An override can be triggered on any member of a
        // packed run, not just the leading one (see `splice_override`'s
        // own doc comment), so `packed_record_siblings(idx)` may return
        // `idx` at any position — shift every *other* member explicitly,
        // never a fixed `[1..]` slice (which would wrongly assume `idx`
        // is the leader).
        if inherited != 0 {
            Self::shift_span(&mut self.tree[idx].span, inherited);
            if self.tree[idx].span.packed_record_start.is_some() {
                for s in self.packed_record_siblings(idx) {
                    if s != idx {
                        Self::shift_span(&mut self.tree[s].span, inherited);
                    }
                }
            }
        }

        let origin = OverrideOrigin::Path {
            path: self.positional_path(idx),
        };
        let already_seeded = self.overrides.entries().iter().any(|e| e.origin == origin);
        if !already_seeded {
            if let Some(t) = self.auto_expand_type(idx) {
                // MessageSet tier 1's synthetic wrapper field has no
                // schema-declared name to fall back on (`field_name_for`
                // would otherwise show the bare field number "1"), so
                // seed it with the display name `prototext-core`'s native
                // MessageSet rendering uses for it ("Item") — spec 0120
                // §G2's follow-up cosmetic fix.
                let is_message_set_item = self.tree[idx].span.field_number == 1
                    && self.tree[idx].span.wire_type == prototext_core::helpers::WT_START_GROUP
                    && self.tree[idx]
                        .parent
                        .is_some_and(|p| self.is_message_set_typed(p));
                self.overrides.activate_auto(origin.clone(), Some(t));
                if is_message_set_item {
                    if let Some(entry_idx) = self
                        .overrides
                        .entries()
                        .iter()
                        .position(|e| e.origin == origin)
                    {
                        self.overrides.rename(entry_idx, Some("Item".to_string()));
                    }
                }
            }
        }
        let spliced = self.resettle_node(idx);
        // Fresh children (just pushed by a splice) are already
        // positioned relative to `idx`'s now-correct start and owe
        // nothing further; pre-existing children (no splice happened)
        // still owe whatever `idx` itself was just given.
        let mut child_owed = if spliced { 0 } else { inherited };
        let initial_child_owed = child_owed;
        let mut child = self.tree[idx].first_child;
        while let Some(c) = child {
            // Recurse into every node actually rendered as message/group
            // (`NodeSpan::is_message`) — the set of nodes that can carry
            // nested overridable children at all (spec 0119) — plus the
            // two specific plain-scalar shapes eligible for Any/
            // MessageSet auto-expansion (spec 0120's
            // `is_auto_expand_candidate`): those aren't `is_message` yet
            // (they're still bytes/varint until auto-overridden), but
            // must still be visited once so `auto_expand_type` above gets
            // a chance to promote them. Recursing into every plain
            // scalar LEN-wire field unconditionally would reopen the
            // spec 0119 bug this same gate was introduced to fix
            // (`natural_type` demoting an ordinary string/bytes field to
            // a raw record dump) — `is_auto_expand_candidate` is
            // deliberately narrow, matching only these two shapes. Also
            // recurse into any child carrying its own active override
            // entry (spec 0135 §G3 gap, found post-implementation): a
            // primitive override on a plain scalar leaf is exactly such
            // a node — not `is_message`, not an auto-expand candidate —
            // and would otherwise never actually get spliced by
            // `:type-as` (which, unlike the override pane's live
            // preview, applies solely through this recursive walk). Also
            // recurse into any child already carrying a `rendered_as`
            // (spec 0135 follow-up, found post-implementation): once a
            // plain scalar leaf has been spliced under an override at
            // least once, it must keep being revisited on every future
            // pass, even after that override is deactivated — otherwise
            // `resolve_active_override_entry(c)` above goes back to
            // `None` (deactivated) the moment the gate condition it
            // relies on stops holding, permanently orphaning the node
            // before `resettle_node` gets a chance to fall it back to
            // its natural type.
            if self.tree[c].span.is_message
                || self.is_auto_expand_candidate(c)
                || self.resolve_active_override_entry(c).is_some()
                || self.tree[c].rendered_as.is_some()
            {
                let before = self.pending_shift;
                self.render_overrides_inner(c, child_owed);
                child_owed += self.pending_shift - before;
            } else if child_owed != 0 {
                Self::shift_span(&mut self.tree[c].span, child_owed);
            }
            child = self.tree[c].next_sibling;
        }
        // `idx`'s own closing/footer `end` must grow by however much its
        // children collectively grew during this loop (their own
        // splices, and/or further-nested descendants' splices) — its
        // `start` never moves due to its own descendants' growth.
        let growth = child_owed - initial_child_owed;
        if growth != 0 {
            self.tree[idx].span.text_range.end =
                (self.tree[idx].span.text_range.end as isize + growth) as usize;
        }
    }

    /// Spec 0160 G1: runs exactly once, when the outermost
    /// `render_overrides` call for a batch of splices returns (or a
    /// standalone `splice_override` call finishes) — not once per
    /// splice. Everything *inside* `idx`'s own subtree is already
    /// correct by this point (`render_overrides_inner`'s carried-down
    /// correction, or — for a standalone call — `splice_override`'s own
    /// direct writes); this handles exactly what's left: what a single
    /// eager splice on `idx` alone would still owe the *rest* of the
    /// document — every live node strictly after `idx`'s whole subtree,
    /// and every ancestor of `idx` (its `text_range.end` only) — applied
    /// once for the whole batch using its aggregate `pending_shift`,
    /// followed by the `line_to_node`/`footer_line_to_node` rebuild and
    /// `rebuild_visible_rows()` (formerly redone on every splice).
    fn finalize_override_batch(&mut self, idx: usize) {
        let delta = self.pending_shift;
        if delta != 0 {
            // Doc-order-last live descendant of `idx` — `last_child`,
            // walked to its own leaf, since `build_tree`/`splice_
            // override` always keep it as the doc-order-last direct
            // child (see `packed_run_is_last_child`'s existing use of
            // this same invariant).
            let mut last = idx;
            while let Some(lc) = self.tree[last].last_child {
                last = lc;
            }
            let mut after = self.tree[last].doc_next;
            while let Some(a) = after {
                Self::shift_span(&mut self.tree[a].span, delta);
                after = self.tree[a].doc_next;
            }
            let mut p = self.tree[idx].parent;
            while let Some(pi) = p {
                self.tree[pi].span.text_range.end =
                    (self.tree[pi].span.text_range.end as isize + delta) as usize;
                p = self.tree[pi].parent;
            }
        }
        self.line_to_node.clear();
        self.footer_line_to_node.clear();
        let mut cur = Some(self.first_node);
        while let Some(c) = cur {
            let node = &self.tree[c];
            self.line_to_node.insert(node.span.text_range.start, c);
            // See `App::new`'s matching build site (spec 0142 fix) for
            // why this checks the line span rather than `first_child`.
            if node.span.text_range.end - 1 > node.span.text_range.start {
                self.footer_line_to_node
                    .insert(node.span.text_range.end - 1, c);
            }
            cur = node.doc_next;
        }
        self.pending_shift = 0;
        self.rebuild_visible_rows();
    }

    /// Unified splice mechanic (spec 0118 §4, reworked spec 0135 G1):
    /// regenerates the *whole* rendering of `idx` — header, interior, and
    /// footer alike — under `target` (`None` = revert to raw, `Some(fqdn)`
    /// = retype/promote to a message FQDN, `Some(keyword)` = retype to a
    /// wire-compatible primitive type, G3/G4). No existing rendering of
    /// `idx` is ever reused verbatim: decodes `idx`'s own real tag+payload
    /// bytes (`old_span.raw_range`) directly against a synthetic one-field
    /// descriptor (`decode::register_wrapper`) whose sole field has `idx`'s
    /// own real field number and the target's declared type — the node's
    /// real wire framing (message/group/scalar) is reproduced by
    /// `TextSink` for free, no header patching needed (spec 0135
    /// Background). This is what fixes task #34 (a stale `#@` type
    /// annotation surviving a retype) as a byproduct, for every node.
    ///
    /// `idx` keeps its own tree-array identity (so `cursor`/`folded`/
    /// back-jump state referencing it stays valid) — only its `span`
    /// (`raw_range` excepted: the underlying bytes haven't moved) and its
    /// children (old ones orphaned via `collect_descendants`, new ones
    /// appended and stitched in) are replaced.
    ///
    /// For a packed-repeated element (`packed_record_start.is_some()`),
    /// `idx` is first reassigned to `siblings[0]` — the packed run's own
    /// receiving node (spec 0135 G1's "sibling merge"): every sibling
    /// element sharing the same packed record is collapsed into this one
    /// node, regardless of which specific element the caller invoked the
    /// override on.
    pub(super) fn splice_override(
        &mut self,
        mut idx: usize,
        target: Option<String>,
    ) -> Result<(), String> {
        // Spec 0160 G2: `self.tree[idx].span` is already authoritative
        // by the time this is called — either `render_overrides_inner`'s
        // prologue already applied `idx`'s own pending correction (and,
        // for a packed member, every other member of its run too), or
        // this is a standalone call (`override_batch_depth == 0`), where
        // `pending_shift == 0` because the tree is already fully
        // reconciled from the previous batch.
        let mut old_span = self.tree[idx].span.clone();
        let is_packed = old_span.packed_record_start.is_some();

        // Packed-record reconstruction + sibling merge (spec 0135 G1):
        // collapse every sibling element of the same packed record into
        // `siblings[0]` before proceeding through the ordinary path below.
        let mut packed_next_sibling_of_run = None;
        let mut packed_seam_after = None;
        let mut packed_run_is_last_child = false;
        let mut packed_orphans: Vec<usize> = Vec::new();
        if is_packed {
            let siblings = self.packed_record_siblings(idx);
            let last = *siblings
                .last()
                .expect("packed_record_siblings always returns at least idx itself");
            let (raw_range, text_range) = self.packed_record_extent(&siblings);
            idx = siblings[0];
            old_span = self.tree[idx].span.clone();
            old_span.raw_range = raw_range;
            old_span.text_range = text_range;

            packed_next_sibling_of_run = self.tree[last].next_sibling;
            packed_seam_after = self.tree[last].doc_next;
            if let Some(parent) = self.tree[idx].parent {
                packed_run_is_last_child = self.tree[parent].last_child == Some(last);
            }
            for &s in &siblings[1..] {
                packed_orphans.push(s);
                self.collect_descendants(s, &mut packed_orphans);
            }
        }

        let field_number = old_span.field_number;
        let field_name = self.field_name_for(idx);
        let renamed = self
            .resolve_active_override_entry(idx)
            .and_then(|e| e.name.clone())
            .is_some();

        // Resolve `target` into the synthetic field's declared `Type`
        // (spec 0135 G1's "second subtlety" + G3, spec 0137 §G3/§G4): a
        // message FQDN yields `Type::Group` only when the node's real
        // wire framing is `WT_START_GROUP`, else `Type::Message`; a
        // primitive keyword yields the matching primitive `Type`
        // directly; an enum FQDN yields `Type::Enum`; the reserved
        // `None` sentinel string and a plain `Option::None` (raw) both
        // yield no synthetic field at all.
        let (target_desc, field_type) = match &target {
            None => (None, None),
            Some(name) if name == "protolens_internal.None" => (None, None),
            Some(name) => {
                if let Some(desc) = self.ctx.pool().get_message_by_name(name) {
                    let ft = if old_span.wire_type == prototext_core::helpers::WT_START_GROUP {
                        Type::Group
                    } else {
                        Type::Message
                    };
                    (Some(decode::WrapperTarget::Message(desc)), Some(ft))
                } else if let Some(prim) = decode::primitive_type_for_keyword(name) {
                    (None, Some(prim))
                } else if let Some(enum_desc) = self.ctx.pool().get_enum_by_name(name) {
                    (
                        Some(decode::WrapperTarget::Enum(enum_desc)),
                        Some(Type::Enum),
                    )
                } else {
                    return Err(format!("type '{name}' not found in descriptor set"));
                }
            }
        };

        // Decode `idx`'s own real tag+payload bytes directly (spec 0135
        // G1) — no synthetic tag prepended.
        let field_bytes = self.blob[old_span.raw_range.clone()].to_vec();

        // Render-cache key: `(interior_range, target)` — no longer
        // `field_name` (G2 makes the cached render field-name-invariant).
        // `interior_range` is the same "interior" quantity the cache
        // already keyed on before this spec, just computed from the
        // resolved `raw_range`, with `packed_record_start` always `None`
        // (the packed case has already been normalized above).
        let interior_range = extract::message_payload_range(&self.blob, &old_span.raw_range, None);
        let cache_key = (interior_range, target.clone());
        let (mut new_lines, new_spans, new_style_hints) = match self.render_cache.get(&cache_key) {
            Some(cached) => cached,
            None => {
                let wrapper_desc = match field_type {
                    Some(ft) => Some(
                        decode::register_wrapper(
                            self.ctx.pool_mut(),
                            field_number,
                            ft,
                            target_desc,
                        )
                        .map_err(|e| e.to_string())?,
                    ),
                    None => None,
                };
                let opts = DecodeRenderOpts {
                    // Always on (spec 0133): annotations are a pure
                    // main-pane display concern, not a decode-time input.
                    annotations: true,
                    indent_size: self.indent_size,
                    initial_level: old_span.level,
                    emit_header: false,
                    // Any/MessageSet expansion is handled by protolens
                    // itself, as automatic overrides (spec 0120), not by
                    // prototext-core's own virtual-node expansion.
                    expand_any: false,
                    expand_message_set: false,
                    ..Default::default()
                };
                let (new_text, new_spans) =
                    decode_and_render_indexed(&field_bytes, wrapper_desc.as_ref(), opts);
                let new_text = String::from_utf8(new_text)
                    .map_err(|e| format!("rendered text is not valid UTF-8: {e}"))?;
                let new_lines: Vec<String> = new_text.lines().map(str::to_string).collect();
                let new_style_hints = colorize::colorize(&new_text);
                let value = (new_lines, new_spans, new_style_hints);
                self.render_cache.insert(cache_key, value.clone());
                value
            }
        };

        // G2: the only remaining header patch is a plain substring
        // replacement of the synthetic field's placeholder name (`"_"`,
        // `register_wrapper`'s fixed literal) with the real display name
        // — the header line itself is otherwise already correct (spec
        // 0135 G1). Nor for `Type::Group`: `TextSink::begin_nested` labels
        // a group header with the group's own message type name, never
        // the field's declared name — standard proto2 group text-format
        // convention — so the `"_"` placeholder is never actually
        // present there. Any group target that needs a display name
        // other than its own type name must instead be named that way
        // at the source (e.g. `register_message_set_item`'s synthetic
        // shape is itself named `Item`, matching `prototext-core`'s own
        // native MessageSet rendering convention) rather than patched
        // here after the fact. For the raw (`target: None`) case, there
        // is no synthetic field/placeholder to patch — the header
        // already shows the node's own numeric field number straight off
        // the wire — except when an active override entry gives this
        // node its own rename (spec 0119 §G4), which must still show up
        // in place of that bare field number even though the node stays
        // raw.
        let mut new_line_styles = colorize::hints_by_line(&new_lines, &new_style_hints);
        let patched_header = match field_type {
            Some(ft) if ft != Type::Group => {
                decode::patch_synthetic_field_name(&new_lines[0], &field_name)
            }
            None if renamed => {
                decode::patch_raw_field_name(&new_lines[0], field_number, &field_name)
            }
            _ => None,
        };
        if let Some(patched) = patched_header {
            new_lines[0] = patched;
            new_line_styles[0] =
                colorize::hints_by_line(&new_lines[..1], &colorize::colorize(&new_lines[0]))
                    .remove(0);
        }

        let delta = new_lines.len() as isize
            - (old_span.text_range.end - old_span.text_range.start) as isize;
        // Spec 0160 G2: bump the batch's running offset now, rather than
        // eagerly walking every downstream node's `text_range` —
        // everything outside `idx`'s own subtree is reconciled once,
        // lazily, in `finalize_override_batch`; everything inside it is
        // handled directly, by `render_overrides_inner`'s carried-down
        // correction.
        self.pending_shift += delta;

        // Collect old descendants (pointer-based, before any pointer is
        // overwritten below) and scrub them from `folded` — otherwise
        // `rebuild_visible_rows` could read their now-meaningless stale
        // `text_range` and hide unrelated post-splice content. `idx`
        // itself is deliberately left in `folded` untouched (spec 0118
        // §7 — fold state on `idx` survives its own retype). For a
        // packed sibling merge, `packed_orphans` (siblings[1..] and their
        // own descendants) are unioned in too (spec 0135 G1).
        let mut old_descendants = Vec::new();
        self.collect_descendants(idx, &mut old_descendants);
        old_descendants.extend(packed_orphans);
        for d in &old_descendants {
            self.folded.remove(d);
        }
        let old_descendants: HashSet<usize> = old_descendants.into_iter().collect();

        // The live node immediately following the *whole* old subtree in
        // document order — the seam the new subtree must be spliced back
        // into. For a packed sibling merge this is `siblings.last()`'s
        // own `doc_next`, not `idx`'s (spec 0135 G1) — `idx` is now
        // `siblings[0]`, but the whole run is being replaced.
        let mut after = if is_packed {
            packed_seam_after
        } else {
            self.tree[idx].doc_next
        };
        while let Some(a) = after {
            if old_descendants.contains(&a) {
                after = self.tree[a].doc_next;
            } else {
                break;
            }
        }

        // Replace `idx`'s *whole* line range (header, interior, and
        // footer alike) — not just its interior, unlike the old
        // `apply_override`.
        self.lines.splice(
            old_span.text_range.start..old_span.text_range.end,
            new_lines,
        );
        self.line_styles.splice(
            old_span.text_range.start..old_span.text_range.end,
            new_line_styles,
        );

        // Translate the freshly built local tree (raw_range-relative
        // coordinates) into this document's global coordinates and append
        // it at the array's end. `build_tree` always emits a container's
        // own span last (post-order) — the local tree's final entry is
        // always idx's *new* self (the decoded field, whatever shape it
        // turned out to be); everything else is its descendants.
        let base = self.tree.len();
        let byte_offset = old_span.raw_range.start as isize;
        let local_len = new_spans.len();
        let local_root_idx = local_len - 1;
        let local_tree = decode::build_tree(new_spans);
        for node in local_tree {
            let mut span = node.span;
            span.raw_range = (span.raw_range.start as isize + byte_offset) as usize
                ..(span.raw_range.end as isize + byte_offset) as usize;
            span.text_range = (span.text_range.start + old_span.text_range.start)
                ..(span.text_range.end + old_span.text_range.start);
            let translate = |o: Option<usize>| o.map(|i| i + base);
            // `idx`'s new self is *not* pushed as a separate live entry
            // (its own span/children are folded into `self.tree[idx]`
            // below) — root-level local nodes (parent `None`) and its
            // direct children (local parent == the local root) both
            // become `idx`'s children, so both map their parent to `idx`.
            let parent = if node.parent.is_none() || node.parent == Some(local_root_idx) {
                Some(idx)
            } else {
                node.parent.map(|p| p + base)
            };
            self.tree.push(TreeNode {
                span,
                parent,
                first_child: translate(node.first_child),
                last_child: translate(node.last_child),
                next_sibling: translate(node.next_sibling),
                prev_sibling: translate(node.prev_sibling),
                doc_next: translate(node.doc_next),
                doc_prev: translate(node.doc_prev),
                rendered_as: None,
            });
        }

        // Keep `heat_states` parallel to `tree` (spec 0152 G6) — every
        // freshly pushed node starts all-pending; `idx` itself was just
        // retyped, so any previously resolved cue for it is now stale
        // and must be reset too, not left dangling as settled.
        self.heat_states
            .resize(self.tree.len(), heat_cue::HeatState::default());
        self.heat_states[idx] = heat_cue::HeatState::default();

        // The pushed copy of the local root (at `new_self_idx`) is left
        // orphaned, never referenced again — its span/children are copied
        // into the live `idx` entry instead, same "abandon in place"
        // pattern already used for old descendants.
        let new_self_idx = base + local_root_idx;
        let mut new_self_span = self.tree[new_self_idx].span.clone();
        // Defensive restatement: byte-offset translation above already
        // reproduces `old_span.raw_range` exactly, since `field_bytes` is
        // `idx`'s complete original tag+payload span decoded as-is (spec
        // 0135 G1) — no synthetic tag ever separates the two.
        new_self_span.raw_range = old_span.raw_range.clone();
        self.tree[idx].span = new_self_span;
        self.tree[idx].first_child = self.tree[new_self_idx].first_child;
        self.tree[idx].last_child = self.tree[new_self_idx].last_child;

        // Packed sibling-merge pointer repair (spec 0135 G1): skip
        // `idx`'s sibling linkage past the absorbed run. `idx`'s own
        // `prev_sibling` and the parent's `first_child` need no change —
        // the run's leading edge is unaffected by absorbing what follows.
        if is_packed {
            self.tree[idx].next_sibling = packed_next_sibling_of_run;
            if let Some(next) = packed_next_sibling_of_run {
                self.tree[next].prev_sibling = Some(idx);
            }
            if packed_run_is_last_child {
                if let Some(parent) = self.tree[idx].parent {
                    self.tree[parent].last_child = Some(idx);
                }
            }
        }

        if local_len > 1 {
            let first_new = self.tree[new_self_idx].doc_next;
            let last_new = (base..base + local_len)
                .find(|&i| self.tree[i].doc_next.is_none())
                .expect("local tree with descendants has a document-order last node");
            self.tree[idx].doc_next = first_new;
            if let Some(fnw) = first_new {
                self.tree[fnw].doc_prev = Some(idx);
            }
            self.tree[last_new].doc_next = after;
            if let Some(a) = after {
                self.tree[a].doc_prev = Some(last_new);
            }
        } else {
            self.tree[idx].doc_next = after;
            if let Some(a) = after {
                self.tree[a].doc_prev = Some(idx);
            }
        }

        // Spec 0160 G2: no eager forward/ancestor `text_range` shift and
        // no `line_to_node`/`footer_line_to_node`/`rebuild_visible_rows`
        // rebuild here anymore — `self.pending_shift += delta` above
        // already recorded this splice's effect. When called from within
        // a `render_overrides` batch (`override_batch_depth > 0`, e.g.
        // via `resettle_node`), reconciliation is deferred to that outer
        // call's own `finalize_override_batch`. `splice_override` is
        // also called directly outside of any `render_overrides` batch
        // (e.g. `override_select.rs`'s live preview splice,
        // `override_batch_depth == 0` there) — such a call must finalize
        // immediately itself, exactly matching today's eager-splice
        // behavior for a single standalone splice.
        if self.override_batch_depth == 0 {
            self.finalize_override_batch(idx);
        }

        // Spec 0142 G6.1: `idx` keeps its own tree-array identity across
        // a retype (see this function's own doc comment), so if the
        // cursor was resting on `idx`'s footer and the retype turned it
        // into a childless (scalar) node, that footer line no longer
        // exists — fall back to `idx`'s header rather than leave a
        // stale footer-cursor referencing a line that's gone.
        if self.cursor_footer && !self.has_children(self.cursor) {
            self.cursor_footer = false;
        }

        Ok(())
    }

    /// Every current sibling of `idx` that shares the same
    /// `packed_record_start` (spec 0135 G1) — i.e. every element of the
    /// same packed-repeated record, in document order. Always returns at
    /// least `idx` itself, even when `idx` has no parent.
    pub(super) fn packed_record_siblings(&self, idx: usize) -> Vec<usize> {
        let target = self.tree[idx].span.packed_record_start;
        let Some(parent) = self.tree[idx].parent else {
            return vec![idx];
        };
        let mut out = Vec::new();
        let mut c = self.tree[parent].first_child;
        while let Some(ci) = c {
            if self.tree[ci].span.packed_record_start == target {
                out.push(ci);
            }
            c = self.tree[ci].next_sibling;
        }
        out
    }

    /// The raw-byte and text-line extent of a packed record's whole run
    /// (spec 0135 G1), re-parsing the record's real tag+length from
    /// `packed_record_start` (mirroring `extract::message_payload_range`'s
    /// own packed-record handling). `siblings` must be non-empty and in
    /// document order, as returned by `packed_record_siblings`.
    pub(super) fn packed_record_extent(
        &self,
        siblings: &[usize],
    ) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
        let start = self.tree[siblings[0]]
            .span
            .packed_record_start
            .expect("packed_record_extent called with non-packed siblings");
        let tag = prototext_core::helpers::parse_wiretag(&self.blob, start);
        let len = prototext_core::helpers::parse_varint(&self.blob, tag.next_pos);
        let raw_end = len.next_pos + len.varint.unwrap_or(0) as usize;
        let last = *siblings.last().expect("siblings never empty");
        // Spec 0160 G2: every member of the run was already corrected by
        // `render_overrides_inner`'s prologue (or, for a standalone
        // call, is already reconciled since `pending_shift == 0` then) —
        // read `text_range` directly, no further correction needed.
        let text_range =
            self.tree[siblings[0]].span.text_range.start..self.tree[last].span.text_range.end;
        (start..raw_end, text_range)
    }

    /// Origin for a brand-new override, targeting node `idx` — created
    /// as kind `PathField` by default (feedback, 2026-07-21), since
    /// path:field origins survive sibling reordering/insertion better
    /// than a plain positional `Path`. Falls back to `Path` when `idx`
    /// is the wrapper root (no parent), the one case `PathField` can
    /// never resolve. Delegates to `origin_for_kind`.
    pub(super) fn override_origin_for_kind(&self, idx: usize) -> Result<OverrideOrigin, String> {
        match self.origin_for_kind(idx, OverrideKind::PathField) {
            Ok(origin) => Ok(origin),
            Err(_) => self.origin_for_kind(idx, OverrideKind::Path),
        }
    }

    /// Origin for an arbitrary `kind`, targeting node `idx` (spec 0117
    /// §2's derivation rules, generalized in spec 0124 G2 so the
    /// manage-pane `z` key can rederive an origin under a rotated kind).
    /// `PathField`/`FqdnField` error out when `idx` is the wrapper root
    /// (no parent) or, for `FqdnField`, when the parent's `type_fqdn` is
    /// unresolved.
    pub(super) fn origin_for_kind(
        &self,
        idx: usize,
        kind: OverrideKind,
    ) -> Result<OverrideOrigin, String> {
        match kind {
            OverrideKind::Path => Ok(OverrideOrigin::Path {
                path: self.positional_path(idx),
            }),
            OverrideKind::PathField => {
                let parent = self.tree[idx]
                    .parent
                    .ok_or_else(|| "cursor is the wrapper root (no parent)".to_string())?;
                Ok(OverrideOrigin::PathField {
                    path: self.positional_path(parent),
                    field: self.tree[idx].span.field_number,
                })
            }
            OverrideKind::FqdnField => {
                let parent = self.tree[idx]
                    .parent
                    .ok_or_else(|| "cursor is the wrapper root (no parent)".to_string())?;
                let fqdn = self.tree[parent]
                    .span
                    .type_fqdn
                    .clone()
                    .ok_or_else(|| "parent's type is unresolved".to_string())?;
                Ok(OverrideOrigin::FqdnField {
                    fqdn,
                    field: self.tree[idx].span.field_number,
                })
            }
        }
    }

    /// Third `OverrideKind` — the one that is neither `a` nor `b` (spec
    /// 0134 G2 step 5's `other_kind`; there are only 3 kinds total).
    pub(super) fn third_kind(a: OverrideKind, b: OverrideKind) -> OverrideKind {
        [
            OverrideKind::Path,
            OverrideKind::PathField,
            OverrideKind::FqdnField,
        ]
        .into_iter()
        .find(|k| *k != a && *k != b)
        .expect("3 kinds total, 2 excluded, 1 remains")
    }
}

/// Formats a message/group/enum status-line label (spec 0136): `fqdn`
/// prepended with a leading `.` only if omitting it would make the bare
/// `fqdn` collide with a primitive keyword or the reserved `None`
/// keyword (a future fake primitive type, per review — not yet wired up
/// anywhere in this codebase).
fn format_fqdn_label(fqdn: &str) -> String {
    if fqdn_needs_dot_prefix(fqdn) {
        format!(".{fqdn}")
    } else {
        fqdn.to_string()
    }
}

/// Spec 0136/0137 §G6: whether a bare message/group/enum FQDN would
/// collide with a primitive keyword or the reserved `None` keyword if
/// displayed undecorated — shared between the status-line label
/// (`format_fqdn_label`) and the override pane's row rendering
/// (`render.rs`'s `render_override_pane`).
pub(super) fn fqdn_needs_dot_prefix(fqdn: &str) -> bool {
    decode::primitive_type_for_keyword(fqdn).is_some() || fqdn == "None"
}
