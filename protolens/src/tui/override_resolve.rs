// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Which type a node has, and which override entry said so.
//!
//! Everything here reads `self.tree`, `self.fqdns`, `self.ctx`,
//! `self.overrides` and `self.blob`, and nothing else — no line, no
//! span, no fold, no node ever created or moved. That is what separates
//! it from `override_apply.rs`, which does the writing. `auto_expand_type`
//! is the one `&mut self` in the file, and what it mutates is the
//! descriptor pool: the schema grows, the document does not.

use super::*;

use prost_reflect::Cardinality;

/// Unifies `FieldDescriptor` (regular field) and `ExtensionDescriptor`
/// (extension field) for `parent_field`'s callers — mirrors
/// `prototext_core`'s own (crate-private) `FieldOrExt` adapter, which
/// can't be reused directly since it's `pub(super)` to that crate's
/// `render_text` module. Only the accessors `parent_field`'s three
/// call sites actually need are exposed.
pub(super) enum ParentFieldOrExt {
    Field(prost_reflect::FieldDescriptor),
    Ext(prost_reflect::ExtensionDescriptor),
}

impl ParentFieldOrExt {
    pub(super) fn kind(&self) -> prost_reflect::Kind {
        match self {
            ParentFieldOrExt::Field(f) => f.kind(),
            ParentFieldOrExt::Ext(e) => e.kind(),
        }
    }

    pub(super) fn cardinality(&self) -> Cardinality {
        match self {
            ParentFieldOrExt::Field(f) => f.cardinality(),
            ParentFieldOrExt::Ext(e) => e.cardinality(),
        }
    }

    /// Regular field: bare field name. Extension field: its full
    /// (dotted) name — there is no bracket-wrapping here, unlike
    /// `prototext_core::FieldOrExt::display_name` (that one is for
    /// prototext syntax output; this one feeds `field_name_for`'s plain
    /// status-line/override-pane name display).
    pub(super) fn name(&self) -> String {
        match self {
            ParentFieldOrExt::Field(f) => f.name().to_string(),
            ParentFieldOrExt::Ext(e) => e.full_name().to_string(),
        }
    }
}

impl App {
    /// Looks up `idx`'s own field on its parent's schema (spec 0119
    /// §G1/§G2's shared lookup): requires both that `idx`'s parent has a
    /// resolved `type_fqdn` and that its schema declares `idx`'s
    /// `field_number`, either as a regular field or as an extension
    /// (mirroring `render_text::render_message`'s own `get_field`/
    /// `get_extension` fallback — omitting the extension fallback here
    /// silently demoted every message-typed FieldOptions-style extension
    /// to raw/untyped on the auto-resettle walk). Returns `None` when
    /// neither lookup succeeds (no parent, unresolved parent type, or the
    /// field isn't declared at all) — the same failure mode
    /// `natural_type`/`field_name_for` both fall back from.
    pub(super) fn parent_field(&self, idx: usize) -> Option<ParentFieldOrExt> {
        let parent = self.parent(idx)?;
        let fqdn = self.fqdns.get(self.tree[parent].span.type_fqdn)?;
        let field_number = self.tree[idx].span.field_number;
        let message = self.ctx.pool().get_message_by_name(fqdn)?;
        message
            .get_field(field_number)
            .map(ParentFieldOrExt::Field)
            .or_else(|| {
                message
                    .get_extension(field_number)
                    .map(ParentFieldOrExt::Ext)
            })
    }

    /// The type `idx` would naturally have from its parent's schema, used
    /// as the fallback when no active override applies (spec 0119 §G1) —
    /// `None` only when genuinely no type information is available at all
    /// (no parent schema, or field not declared).
    ///
    /// Every primitive `Kind` resolves to its own `:type-as` keyword,
    /// not `None`: since spec 0135 G3 widened `can_override` to plain
    /// scalar leaves, a primitive field with no active override is a
    /// real, reachable case here (e.g. pressing `t` then `Esc` on a
    /// plain `int32` field) — `None` would make `resettle_node` fall
    /// back to raw, wrongly discarding the field's own natural decoding
    /// rather than merely reporting "no override applies."
    /// `primitive_type_for_keyword`'s reverse mapping is what gives
    /// primitives the same "natural type, not raw" fallback message
    /// fields have had since spec 0119.
    ///
    /// `Kind::Enum` resolves to its own FQDN for the same reason.
    /// Excluding it would make `resettle_node` demote every enum-typed
    /// field to a raw record dump the moment it is directly resettled
    /// (e.g. opening the override pane with `t` then cancelling with
    /// `Esc`) — permanently, since no other pass ever revisits a plain
    /// scalar leaf once `rendered_as` is set (see `render_overrides`'s
    /// recursion-gating comment below).
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
            let fqdn = self.fqdns.get(self.tree[idx].span.type_fqdn)?;
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

    /// `true` when `idx`'s resolved type is `google.protobuf.Any` — spec
    /// 0120 §G1's detection rule, a plain FQDN match (per review).
    pub(super) fn is_any_typed(&self, idx: usize) -> bool {
        // Spec 0212 S6: `id_of`'s miss is `UNINTERNED`, not `NO_FQDN`, so a
        // document containing no `Any` at all answers `false` here rather
        // than `true` for every typeless node.
        self.tree[idx].span.type_fqdn == self.fqdns.id_of("google.protobuf.Any")
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
        let Some(fqdn) = self.fqdns.get(self.tree[idx].span.type_fqdn) else {
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
    /// on a tree node or an override entry.
    pub(super) fn message_set_item_display_fqdn(&self, idx: usize) -> Option<String> {
        let parent = self.parent(idx)?;
        let message_set_fqdn = self.fqdns.get(self.tree[parent].span.type_fqdn)?;
        Some(decode::message_set_item_display_fqdn(message_set_fqdn))
    }

    /// The sibling of `idx` (another child of `idx`'s own parent) whose
    /// `field_number` is `field_number`, if any — used by
    /// `auto_expand_type` to locate Any's `type_url` next to `value`, and
    /// MessageSet's `type_id` next to `message`.
    pub(super) fn find_sibling(&self, idx: usize, field_number: u64) -> Option<usize> {
        let parent = self.parent(idx)?;
        let mut c = self.first_child(parent);
        while let Some(ci) = c {
            if u64::from(self.tree[ci].span.field_number) == field_number {
                return Some(ci);
            }
            c = self.next_sibling(ci);
        }
        None
    }

    /// Reads `idx`'s own raw payload (tag/length stripped, per
    /// `extract::message_payload_range`) as a UTF-8 string — used to read
    /// Any's `type_url` value directly off the wire, independent of how
    /// (or whether) it's currently rendered.
    pub(super) fn read_string_field(&self, idx: usize) -> Option<String> {
        let span = &self.tree[idx].span;
        let payload = extract::message_payload_range(&self.blob, &span.raw_range);
        String::from_utf8(self.blob[payload].to_vec()).ok()
    }

    /// Reads `idx`'s own raw payload as a varint — used to read
    /// MessageSet's `type_id` value directly off the wire.
    pub(super) fn read_varint_field(&self, idx: usize) -> Option<u64> {
        let span = &self.tree[idx].span;
        let payload = extract::message_payload_range(&self.blob, &span.raw_range);
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
        let Some(parent) = self.parent(idx) else {
            return false;
        };
        let field_number = self.tree[idx].span.field_number;
        if field_number == 2 && self.is_any_typed(parent) {
            return true;
        }
        // MessageSet tier 1 (the "Item" group wrapper itself). It is
        // tempting to omit, since a real decoded group is
        // `is_message == true` anyway — but spec 0183 makes this
        // predicate the seed source for the descent marks, with no
        // `is_message` recursion disjunct to reach it for free, so
        // omitting it would silently stop MessageSet auto-expansion:
        // no panic, just an unexpanded document.
        //
        // The two O(1) discriminators come first so that the
        // `pool()` lookup inside `is_message_set_typed` is reached
        // only by field-1 group nodes, not by every node in the
        // document (spec 0183 S1's cheap-first requirement).
        if field_number == 1
            && u32::from(self.tree[idx].span.wire_type) == prototext_core::helpers::WT_START_GROUP
            && self.is_message_set_typed(parent)
        {
            return true;
        }
        if field_number == 3
            && self.tree[parent].span.type_fqdn == self.fqdns.id_of(decode::MESSAGE_SET_ITEM_FQDN)
        {
            if let Some(grandparent) = self.parent(parent) {
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
        let parent = self.parent(idx)?;
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
            // A `type_url` names any type in the schema, not necessarily
            // one in the root's file closure — so it JIT-loads (spec 0197
            // §S5), exactly as `prototext`'s own `Any` loader does.
            return self.ctx.message(fqdn).map(|d| d.full_name().to_string());
        }

        // MessageSet tier 1: the "Item" group wrapper (field 1,
        // `WT_START_GROUP`) auto-derives to the synthetic, globally
        // shared `protolens_internal.Item` shape (`type_id` +
        // `message`) — registered once per pool and reused across every
        // MessageSet occurrence in the document.
        if field_number == 1
            && u32::from(self.tree[idx].span.wire_type) == prototext_core::helpers::WT_START_GROUP
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
            && self.tree[parent].span.type_fqdn == self.fqdns.id_of(decode::MESSAGE_SET_ITEM_FQDN)
        {
            let grandparent = self.parent(parent)?;
            if self.is_message_set_typed(grandparent) {
                let type_id_idx = self.find_sibling(idx, 2)?;
                // Checked, not `as`: the varint comes off the wire, and a
                // truncating narrowing would name a *valid but different*
                // extension, which renders as the wrong type with nothing
                // to show for it. Refusing leaves the node untyped.
                let type_id = u32::try_from(self.read_varint_field(type_id_idx)?).ok()?;
                let grandparent_fqdn = self
                    .fqdns
                    .get(self.tree[grandparent].span.type_fqdn)?
                    .to_owned();
                // The extension is declared in whatever file extends the
                // MessageSet, which need not be in the root closure; the
                // index's `ext_to_file` names it (spec 0100 §5.1, 0197 §S5).
                self.ctx.load_extension(&grandparent_fqdn, type_id);
                let extendee = self.ctx.pool().get_message_by_name(&grandparent_fqdn)?;
                let ext = extendee.get_extension(type_id)?;
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
        let path = self.positional_path(idx);
        self.field_name_for_by_path(idx, &path)
    }

    /// Same as `field_name_for`, but takes `idx`'s own already-known
    /// path rather than recomputing it — see `resolve_active_override_
    /// entry_index_by_path`'s doc comment for why this matters.
    pub(super) fn field_name_for_by_path(&self, idx: usize, path: &str) -> String {
        if let Some(name) = self
            .resolve_active_override_entry_index_by_path(idx, path)
            .and_then(|i| self.overrides.entries()[i].name.clone())
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
        let path = self.positional_path(idx);
        self.resolve_active_override_entry_index_by_path(idx, &path)
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
        self.resolve_active_override_entry_index_by_path(idx, &path)
    }

    /// Same resolution as `resolve_active_override_entry_index`, but
    /// takes `idx`'s own positional path as an already-known `path`
    /// rather than recomputing it via `positional_path` — `path` is
    /// O(depth) to walk up from `idx`, and its own leading segment
    /// (`sibling_position`) is O(k) in `idx`'s ordinal position among
    /// its siblings; both are cheap for a single ad hoc call but
    /// prohibitively expensive when called once per node across an
    /// entire large document (spec 0163 follow-up: `render_overrides_
    /// inner`'s full-document walk already knows every node's path for
    /// free, incrementally, while descending top-down in sibling
    /// order — see `render_overrides_inner`'s own use of this). The
    /// `PathField` tier's `parent_path` is derived from `path` by
    /// trimming its last segment (`parent_path_of`), rather than a
    /// second `positional_path` call on the parent, for the same
    /// reason.
    pub(super) fn resolve_active_override_entry_index_by_path(
        &self,
        idx: usize,
        path: &str,
    ) -> Option<usize> {
        if let Some(pos) = self.active_entry_with_label(path, OverrideKind::Path) {
            return Some(pos);
        }
        let parent = self.parent(idx)?;
        let field = self.tree[idx].span.field_number;
        let parent_path = Self::parent_path_of(path);
        if let Some(pos) =
            self.active_entry_with_label(&format!("{parent_path}:{field}"), OverrideKind::PathField)
        {
            return Some(pos);
        }
        let fqdn = self.fqdns.get(self.tree[parent].span.type_fqdn)?;
        self.active_entry_with_label(&format!("{fqdn}:{field}"), OverrideKind::FqdnField)
    }

    /// The index of the active entry whose origin has exactly `label`
    /// and kind `kind`, if any — spec 0183 G3.
    ///
    /// `OverrideCollection` keeps `entries` sorted by
    /// `OverrideOrigin::label()` (then type), and has a test pinning
    /// that, so every entry sharing a label is contiguous and a
    /// `partition_point` lands on the first of them. The alternative is
    /// a linear scan of the whole collection — three times over, once
    /// per origin kind, once per node.
    ///
    /// `kind` is checked as well as the label. In practice the label
    /// alone would do (a `PathField` label always starts with `/`, an
    /// `FqdnField` label never does), but relying on that would make
    /// this function's correctness depend on an unstated property of
    /// path syntax rather than on the discriminant that is right there.
    fn active_entry_with_label(&self, label: &str, kind: OverrideKind) -> Option<usize> {
        let entries = self.overrides.entries();
        let start = entries.partition_point(|e| e.origin.label().as_str() < label);
        for (offset, e) in entries[start..].iter().enumerate() {
            if e.origin.label() != label {
                break;
            }
            if e.active && e.origin.kind() == kind {
                return Some(start + offset);
            }
        }
        None
    }

    /// `path`'s own parent path, derived by trimming `path`'s last `/
    /// segment` rather than walking the tree again — see `resolve_
    /// active_override_entry_index_by_path`'s doc comment. `path` is
    /// always either `"/"` (the document root, which has no parent —
    /// callers never actually reach this case since they only call it
    /// after confirming `idx` has a parent) or of the form `"/a/b/.../
    /// n"` (`positional_path`'s own format), so its last `/` always
    /// exists and splits it into `parent_path` + `"/n"`.
    fn parent_path_of(path: &str) -> &str {
        match path.rfind('/') {
            Some(0) => "/",
            Some(pos) => &path[..pos],
            None => "/",
        }
    }

    /// Resolves to the type (or `None` = raw) that should currently be
    /// used to render `idx`'s payload, or the outer `None` when no active
    /// override applies at all — spec 0118 §2.
    pub(super) fn resolve_active_override(&self, idx: usize) -> Option<Option<String>> {
        self.resolve_active_override_entry(idx)
            .map(|e| e.r#type.clone())
    }
}
