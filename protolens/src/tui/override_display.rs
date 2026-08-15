// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0136: how a type is named, in the status line and in the
//! override pane.
//!
//! Presentation only — the label that follows `type:`, and the leading
//! `.` that keeps a bare FQDN from reading as a primitive keyword. No
//! caller decides anything by these.

use super::*;

impl App {
    /// Status-line "type:" fragment for `idx` (spec 0136): the field's
    /// *currently effective* proto type — an active `:type-as` override
    /// if one applies, else the natural (schema-declared) type, enum
    /// included (`natural_type`). `None` when nothing is resolvable at
    /// all, or when the pinned `<raw / no type>` override entry is
    /// explicitly active — either case means the status line shows no
    /// fragment. The second tuple element is the `message`/`group`/
    /// `enum` tag (shown only in full-width mode, via `render.rs`),
    /// `None` for a bare primitive keyword.
    pub(super) fn status_type_label(&self, idx: usize) -> Option<(String, Option<&'static str>)> {
        let span = &self.tree[idx].span;
        if span.is_message {
            // `resettle_node` keeps `span.type_fqdn`/`is_message` in sync
            // with the currently effective override on every render pass
            // — no separate override lookup needed for this branch.
            let fqdn = self.fqdns.get(span.type_fqdn)?;
            let tag = if u32::from(span.wire_type) == prototext_core::helpers::WT_START_GROUP {
                "group"
            } else {
                "message"
            };
            // Spec 0299: the schema-free synthetic shows as the bare keyword.
            if fqdn == decode::SCHEMA_FREE_MESSAGE_FQDN {
                return Some((decode::MESSAGE_KEYWORD.to_string(), Some(tag)));
            }
            // The internal, globally-shared MessageSet Item FQDN
            // (`decode::MESSAGE_SET_ITEM_FQDN`) is never shown to the
            // user directly — show the friendly, MessageSet-specific
            // FQDN instead.
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
        if effective == decode::MESSAGE_KEYWORD {
            return Some((effective, Some("message")));
        }
        // Not a primitive or the `message` keyword: the only other
        // possibility reaching this branch is an enum FQDN from
        // `natural_type`.
        Some((format_fqdn_label(&effective), Some("enum")))
    }
}

/// Formats a message/group/enum status-line label (spec 0136): `fqdn`
/// prepended with a leading `.` only if omitting it would make the bare
/// `fqdn` collide with one of the override keywords.
pub(super) fn format_fqdn_label(fqdn: &str) -> String {
    if fqdn_needs_dot_prefix(fqdn) {
        format!(".{fqdn}")
    } else {
        fqdn.to_string()
    }
}

/// Spec 0136/0137 §G6: whether a bare message/group/enum FQDN would
/// collide with an override keyword if displayed undecorated. Two
/// terms, because the vocabulary has two halves: the names
/// `wrapper_target_for` resolves — the fifteen primitives and spec
/// 0299's `message` — and `none`, the "no type" sentinel, which
/// `render_node_as` intercepts before that ladder is ever reached.
///
/// Presentation only, as this module's header says: a real type named
/// `message` shows as `.message`, but it still *resolves* from the bare
/// name, exactly as a type named `bool` does — the ladder asks the pool
/// first, deliberately.
pub(super) fn fqdn_needs_dot_prefix(fqdn: &str) -> bool {
    decode::is_override_keyword(fqdn) || fqdn == decode::NONE_KEYWORD
}
