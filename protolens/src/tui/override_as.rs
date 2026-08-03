// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `:override-as` (spec 0236): one command that sets an override
//! entry's type, origin and display name together.
//!
//! Before this, each of those three dimensions had its own mechanism —
//! the override pane or `:type-as` for the type, creation time only for
//! the origin, and a bespoke inline text-entry sub-mode for the name —
//! so re-scoping an override meant deleting it and rebuilding it.

use super::*;

use crate::override_pane::{OverrideKind, OverrideOrigin};

/// The three origin shapes `<origin>` accepts, named in every parse
/// error so the message teaches the grammar rather than just rejecting.
const ORIGIN_SHAPES: &str = "expected /path, /path:field, or fqdn:field";

/// One parsed `:override-as` command line. Every field is `Option`
/// because absent means default (spec 0236 S4), and the defaults are
/// resolved against the document rather than here.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct OverrideAsArgs {
    pub(super) r#type: Option<String>,
    pub(super) origin: Option<OverrideOrigin>,
    pub(super) name: Option<String>,
}

/// Parse `<type>` and the two flags, in any order (spec 0236 S2/S3/S5).
/// `args` is already whitespace-split, which is also the rule: no
/// argument may contain a space, because the display name is rendered
/// as a prototext field name and one with a space in it produces a
/// document that cannot be re-parsed.
pub(super) fn parse_override_as(args: &[&str]) -> Result<OverrideAsArgs, String> {
    let mut parsed = OverrideAsArgs::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "--origin" => {
                let value = args
                    .next()
                    .ok_or_else(|| "override-as: --origin needs a value".to_string())?;
                parsed.origin = Some(parse_origin(value)?);
            }
            "--field-name" => {
                let value = args
                    .next()
                    .ok_or_else(|| "override-as: --field-name needs a value".to_string())?;
                parsed.name = Some((*value).to_string());
            }
            other if other.starts_with("--") => {
                return Err(format!("override-as: unknown flag {other}"));
            }
            other if parsed.r#type.is_some() => {
                return Err(format!("override-as: unexpected second type '{other}'"));
            }
            other => parsed.r#type = Some(other.to_string()),
        }
    }
    Ok(parsed)
}

/// `<origin>` parses by shape — the inverse of `OverrideOrigin::label`
/// (spec 0236 S5). The `:` split is on the *last* colon, so an FQDN
/// containing none is unambiguous and a path never is.
fn parse_origin(s: &str) -> Result<OverrideOrigin, String> {
    let Some((container, field)) = s.rsplit_once(':') else {
        return if s.starts_with('/') {
            Ok(OverrideOrigin::Path {
                path: s.to_string(),
            })
        } else {
            Err(format!("override-as: bad origin '{s}' — {ORIGIN_SHAPES}"))
        };
    };
    if container.is_empty() {
        return Err(format!("override-as: bad origin '{s}' — {ORIGIN_SHAPES}"));
    }
    let field: u64 = field
        .parse()
        .map_err(|_| format!("override-as: bad field number in origin '{s}' — {ORIGIN_SHAPES}"))?;
    if let Some(path) = container.strip_prefix('/') {
        Ok(OverrideOrigin::PathField {
            path: format!("/{path}"),
            field,
        })
    } else {
        Ok(OverrideOrigin::FqdnField {
            fqdn: container.to_string(),
            field,
        })
    }
}

impl App {
    /// `o` (spec 0236 S15): open the command line pre-filled with the
    /// full `:override-as` for the current subject — the cursor node in
    /// the main pane, the highlighted entry in the manage pane.
    ///
    /// Every argument is present, none elided (S6), so the line opens
    /// showing what is already true and `Enter` on it is a no-op.
    pub(super) fn prefill_override_as(&mut self) {
        let entry = if self.manage_open && self.manage_focus {
            match self.overrides.entries().get(self.manage_highlight) {
                Some(entry) => Some(entry.clone()),
                // Nothing highlighted because there is nothing to
                // highlight — no subject, so no command to pre-fill.
                None => return,
            }
        } else {
            if !self.can_override(self.cursor) {
                self.message =
                    "cannot override: not a message/group or length-delimited field".to_string();
                return;
            }
            None
        };

        // The node the pre-fill describes: the highlighted entry's first
        // affected node when editing an entry, else the cursor. An entry
        // whose origin currently matches nothing still gets a line — it
        // just falls back to the cursor for the schema-derived name.
        let (origin, r#type, entry_name) = match &entry {
            Some(entry) => (
                entry.origin.clone(),
                entry.r#type.clone(),
                entry.name.clone(),
            ),
            None => (
                OverrideOrigin::Path {
                    path: self.positional_path(self.cursor),
                },
                self.effective_type(self.cursor),
                self.resolve_active_override_entry(self.cursor)
                    .and_then(|e| e.name.clone()),
            ),
        };
        let node = self.origin_subject_node(&origin);

        let mut buf = String::from("override-as");
        if let Some(r#type) = &r#type {
            buf.push(' ');
            buf.push_str(r#type);
        }
        buf.push_str(" --origin ");
        buf.push_str(&origin.label());
        buf.push_str(" --field-name ");
        buf.push_str(&self.display_name_for(node, entry_name));
        self.open_command_line(CommandLineKind::Command, buf);
    }

    /// `<type>`'s pre-filled value for a main-pane node (spec 0236 S7):
    /// what the node is being rendered as right now. `None` for a raw
    /// node, which is how the bare command spells "raw" (S4).
    ///
    /// Mirrors `status_type_label`'s fallback chain, but yields the FQDN
    /// itself rather than its display form: a scalar node has no
    /// `span.type_fqdn`, so an enum- or primitive-typed field falls back
    /// to its effective override and then to its natural type — the same
    /// spelling this command accepts back.
    fn effective_type(&self, idx: usize) -> Option<String> {
        if let Some(fqdn) = self.fqdns.get(self.tree[idx].span.type_fqdn) {
            return Some(fqdn.to_owned());
        }
        match self.resolve_active_override(idx) {
            Some(inner) => inner,
            None => self.natural_type(idx),
        }
    }

    /// `--field-name`'s pre-filled value (spec 0236 S8): the applicable
    /// entry's own `name`, else the schema-derived field name, else
    /// `f<P>` where `<P>` is the node's 1-based position among its
    /// siblings — the same number that forms the last segment of its
    /// positional path.
    fn display_name_for(&self, idx: usize, entry_name: Option<String>) -> String {
        if let Some(name) = entry_name {
            return name;
        }
        match self.schema_field_name(idx) {
            Some(name) => name,
            None => format!("f{}", self.sibling_position(idx)),
        }
    }

    /// The field name `idx` gets from its parent's schema, or `None`
    /// when the parent's type is unresolved or does not declare the
    /// field. Deliberately *not* `field_name_for`'s field-number
    /// fallback: S8's own fallback is `f<P>`, and a bare number would
    /// round-trip into the entry as a display-name override.
    fn schema_field_name(&self, idx: usize) -> Option<String> {
        self.parent_field(idx).map(|f| f.name().to_string())
    }

    /// The node an origin speaks about: its first affected node, or the
    /// cursor when it currently affects none. Used both to validate an
    /// edit and to derive the schema-derived name it is compared
    /// against, so that both answer for the same node.
    fn origin_subject_node(&self, origin: &OverrideOrigin) -> usize {
        self.manage_affected_nodes(origin)
            .first()
            .copied()
            .unwrap_or(self.cursor)
    }

    /// `:override-as [<type>] [--origin <origin>] [--field-name <name>]`
    /// — spec 0236 S9/S10/S11.
    pub(super) fn run_override_as(&mut self, args: Vec<&str>) {
        let parsed = match parse_override_as(&args) {
            Ok(parsed) => parsed,
            Err(e) => {
                self.message = e;
                return;
            }
        };
        let origin = parsed.origin.unwrap_or_else(|| OverrideOrigin::Path {
            path: self.positional_path(self.cursor),
        });
        let subject = self.origin_subject_node(&origin);
        if let Err(e) = self.validate_override_target(subject, parsed.r#type.as_deref()) {
            self.message = e;
            return;
        }

        // Spec 0236 S10: `activate` already reactivates an existing
        // entry with this exact origin *and* type rather than
        // duplicating it, and already deactivates every other entry
        // sharing the origin (spec 0117 §1) — so an edit that lands on
        // an existing entry merges into it.
        self.overrides
            .activate(origin.clone(), parsed.r#type.clone());

        // Spec 0236 S8: a `--field-name` equal to the schema-derived
        // name is stored as `None`. Without this, accepting the
        // pre-filled line on a schema-named field would write a
        // redundant name override into the entry and into the saved
        // YAML, and `o`-then-`Enter` would not be the no-op S6 claims.
        let name = parsed
            .name
            .filter(|name| Some(name.as_str()) != self.schema_field_name(subject).as_deref());
        if let Some(idx) = self.entry_index_of(&origin, &parsed.r#type) {
            self.overrides.rename(idx, name);
        }
        self.render_overrides(self.first_node);

        // Spec 0236 S11: changing origin or type re-sorts the
        // collection, and `render_overrides` can seed further entries of
        // its own, so the highlight is re-derived from the resulting
        // entry rather than left on its index.
        if let Some(idx) = self.entry_index_of(&origin, &parsed.r#type) {
            if self.manage_open {
                self.set_manage_highlight(idx);
            }
        }

        // The affected-node count is what makes a re-scope safe to
        // perform blind: changing `PathField` to `FqdnField` silently
        // widens an override from one node to every occurrence of that
        // field, and this is the only place that widening is visible.
        // Counted after the render pass, since an `FqdnField` origin
        // matches on types the pass itself may have just resettled.
        let nodes = self.manage_affected_nodes(&origin).len();
        let as_what = parsed.r#type.as_deref().unwrap_or("raw");
        let plural = if nodes == 1 { "node" } else { "nodes" };
        self.message = format!("{} as {as_what} — {nodes} {plural}", origin.label());
    }

    /// The index of the entry with exactly this origin and type — the
    /// one `activate` just created or reactivated. Unambiguous: the pair
    /// is an entry's identity (`activate_impl`).
    fn entry_index_of(&self, origin: &OverrideOrigin, r#type: &Option<String>) -> Option<usize> {
        self.overrides
            .entries()
            .iter()
            .position(|e| &e.origin == origin && &e.r#type == r#type)
    }

    /// The validation `:type-as` used to do (spec 0114 §1, spec 0135
    /// §G4), against an arbitrary node rather than always the cursor:
    /// the target must
    /// be an eligible override target, and a primitive type keyword must
    /// be wire-compatible with the target's current wire type.
    ///
    /// Only the *subject* node is checked. An origin that also covers
    /// nodes of other wire types is handled where it already is —
    /// `render_overrides` reports a refusal per node (spec 0221).
    pub(super) fn validate_override_target(
        &self,
        idx: usize,
        new_fqdn: Option<&str>,
    ) -> Result<(), String> {
        if !self.can_override(idx) {
            return Err(
                "cannot override: not a message/group or length-delimited field".to_string(),
            );
        }
        let Some(name) = new_fqdn else {
            return Ok(());
        };
        if decode::primitive_type_for_keyword(name).is_none() {
            return Ok(());
        }
        let wire_type = decode::effective_wire_type(&self.tree[idx].span);
        if decode::primitive_keywords_for_wire_type(wire_type).contains(&name) {
            Ok(())
        } else {
            Err(format!(
                "type '{name}' not wire-compatible with this node's wire type"
            ))
        }
    }

    /// Tab-completion for `:override-as` (spec 0236 S14). Unlike every
    /// other command's, this one dispatches on the *token being
    /// completed* rather than on a fixed argument position: the flags
    /// may appear in either order, before or after the positional.
    ///
    /// `rest` is everything after the command name's first space, up to
    /// the cursor.
    pub(super) fn complete_override_as(&mut self, cmd: &str, rest: &str) {
        let token_byte = rest.rfind(' ').map_or(0, |i| i + 1);
        let (before, token) = rest.split_at(token_byte);
        let token_start = cmd.chars().count() + 1 + before.chars().count();
        match before.split_whitespace().next_back() {
            // A display name has no candidate list to draw on — it is
            // whatever the user wants the field called.
            Some("--field-name") => {}
            Some("--origin") => self.complete_override_origin(token_start, token),
            _ => self.complete_type_at(token_start, token),
        }
    }

    /// `--origin`'s completion (spec 0236 S13): the ≤3 origins
    /// `origin_for_kind` can build for the cursor node, not free text.
    ///
    /// This is what keeps re-scoping honest. `FqdnField` needs the
    /// parent's resolved type FQDN, which does not exist when the parent
    /// is raw, and `PathField` needs a parent at all; `origin_for_kind`
    /// already returns `Err` in exactly those cases. So the candidate
    /// list is the set of re-scopings that are actually possible here —
    /// the user Tabs through them instead of typing an FQDN by hand and
    /// discovering the constraint from an error. Free text is still
    /// *accepted* (S5), for a pasted or saved origin; it is simply not
    /// what completion offers.
    fn complete_override_origin(&mut self, token_start: usize, prefix: &str) {
        let labels: Vec<String> = [
            OverrideKind::Path,
            OverrideKind::PathField,
            OverrideKind::FqdnField,
        ]
        .into_iter()
        .filter_map(|kind| self.origin_for_kind(self.cursor, kind).ok())
        .map(|origin| origin.label())
        .collect();
        let mut matches: Vec<String> = complete_prefix(prefix, labels.iter().map(String::as_str))
            .into_iter()
            .map(String::from)
            .collect();
        if matches.is_empty() {
            self.message = format!("no origin matches '{prefix}'");
            return;
        }
        matches.sort_unstable();
        self.apply_completion(token_start, prefix.chars().count(), matches);
    }
}
