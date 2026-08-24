// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `:override` (specs 0236, 0237): one command that sets an override
//! entry's origin, type and display name together.
//!
//! Before spec 0236, each of those three dimensions had its own
//! mechanism — the override pane or `:type-as` for the type, creation
//! time only for the origin, and a bespoke inline text-entry sub-mode
//! for the name — so re-scoping an override meant deleting it and
//! rebuilding it. Spec 0237 then made the origin the positional
//! argument, since it is what an override *is*, and moved the type
//! behind `--as`.
//!
//! Named `override_cmd` rather than `override` because `override` is a
//! reserved Rust keyword — the same reason `override_pane` is spelled
//! the way it is.

use super::*;

use crate::override_pane::{OverrideKind, OverrideOrigin};
use crate::tui::tiered::Tier;

/// The three origin shapes `<origin>` accepts, named in every parse
/// error so the message teaches the grammar rather than just rejecting.
const ORIGIN_SHAPES: &str = "expected /path, /path:field, or fqdn:field";

/// The origin kinds `<origin>` completion rotates through, narrowest to
/// widest (spec 0237 S6) — the order in which a user widens a scope,
/// and the order the manage pane's own `z` already rotates.
const ORIGIN_KINDS: [OverrideKind; 3] = [
    OverrideKind::Path,
    OverrideKind::PathField,
    OverrideKind::FqdnField,
];

/// One parsed `:override` command line. `origin` is required (spec 0237
/// S2); the other flags are `Option` because absent means default (spec
/// 0236 S4), and the defaults are resolved against the document rather
/// than here.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct OverrideArgs {
    pub(super) origin: OverrideOrigin,
    pub(super) r#type: Option<String>,
    pub(super) name: Option<String>,
    /// Whether `r#type` arrived as `--as-new` (spec 0315 S1) and must
    /// therefore be declared before it is applied.
    ///
    /// A `bool` beside `r#type`, not a second type field: `--as-new foo`
    /// and `--as foo` store the *identical* entry, because declaring is
    /// a property of the invocation and not of the entry. That is what
    /// makes every replay path — re-render, undo, `:restore`, a scripted
    /// step — correct at once: they all re-apply entries, and none of
    /// them re-declares anything.
    pub(super) declare: bool,
    /// Explicit cardinality (spec 0348 §S1): `None` defers to the
    /// schema-derived fallback; `Some(c)` forces `c` in
    /// `register_wrapper`/`splice_override`.
    pub(super) cardinality: Option<prost_reflect::Cardinality>,
}

/// Parse `<origin>` and the two flags, in any order (spec 0237 S2).
/// `args` is already whitespace-split, which is also the rule: no
/// argument may contain a space, because the display name is rendered
/// as a prototext field name and one with a space in it produces a
/// document that cannot be re-parsed.
pub(super) fn parse_override(args: &[&str]) -> Result<OverrideArgs, String> {
    let mut origin: Option<OverrideOrigin> = None;
    let mut r#type: Option<String> = None;
    let mut name: Option<String> = None;
    let mut declare = false;
    let mut saw_as = false;
    let mut cardinality: Option<prost_reflect::Cardinality> = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "--as" | "--as-new" => {
                let is_new = *arg == "--as-new";
                // Only the *pair* is refused, not a repeated flag: a
                // half-edited line that says `--as` twice still means
                // what its last one says, as it always did.
                if (saw_as || declare) && declare != is_new {
                    return Err(
                        "override: --as and --as-new are alternatives — name only one".to_string(),
                    );
                }
                let value = args
                    .next()
                    .ok_or_else(|| format!("override: {arg} needs a value"))?;
                r#type = Some((*value).to_string());
                declare = is_new;
                saw_as = !is_new;
            }
            "--field-name" => {
                let value = args
                    .next()
                    .ok_or_else(|| "override: --field-name needs a value".to_string())?;
                name = Some((*value).to_string());
            }
            "--card" => {
                let value = args
                    .next()
                    .ok_or_else(|| "override: --card needs a value".to_string())?;
                cardinality = Some(parse_cardinality(value)?);
            }
            other if other.starts_with("--") => {
                return Err(format!("override: unknown flag {other}"));
            }
            other if origin.is_some() => {
                return Err(format!("override: unexpected second origin '{other}'"));
            }
            other => origin = Some(parse_origin(other)?),
        }
    }
    let origin = origin.ok_or_else(|| format!("override: missing <origin> — {ORIGIN_SHAPES}"))?;
    Ok(OverrideArgs {
        origin,
        r#type,
        name,
        declare,
        cardinality,
    })
}

/// Parse a `--card` value (spec 0348 §S2).
fn parse_cardinality(s: &str) -> Result<prost_reflect::Cardinality, String> {
    match s {
        "optional" => Ok(prost_reflect::Cardinality::Optional),
        "repeated" => Ok(prost_reflect::Cardinality::Repeated),
        "required" => Ok(prost_reflect::Cardinality::Required),
        _ => Err("override: --card value must be optional, repeated, or required".to_string()),
    }
}

/// Format a `Cardinality` as the `--card` token string.
fn cardinality_str(c: prost_reflect::Cardinality) -> &'static str {
    match c {
        prost_reflect::Cardinality::Optional => "optional",
        prost_reflect::Cardinality::Repeated => "repeated",
        prost_reflect::Cardinality::Required => "required",
    }
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
            Err(format!("override: bad origin '{s}' — {ORIGIN_SHAPES}"))
        };
    };
    if container.is_empty() {
        return Err(format!("override: bad origin '{s}' — {ORIGIN_SHAPES}"));
    }
    let field: u64 = field
        .parse()
        .map_err(|_| format!("override: bad field number in origin '{s}' — {ORIGIN_SHAPES}"))?;
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

/// The origin already on the line, if any — what the `--as` and
/// `--field-name` completers take as their subject, since the line's
/// own positional is a better answer than any pane cursor. Scans the
/// way `parse_override` does (a flag consumes the token after it) but
/// tolerates everything, being fed a half-typed line.
fn line_origin(rest: &str) -> Option<OverrideOrigin> {
    let mut args = rest.split_whitespace();
    while let Some(arg) = args.next() {
        if arg.starts_with("--") {
            args.next();
        } else if let Ok(origin) = parse_origin(arg) {
            return Some(origin);
        }
    }
    None
}

impl App {
    /// `o` (spec 0236 S15): open the command line pre-filled with the
    /// full `:override` for the pane's own subject — the highlighted
    /// entry in the manage pane, the target node in the selection pane.
    ///
    /// Every argument is present, none elided (S6), so the line opens
    /// showing what is already true and `Enter` on it is a no-op. In
    /// particular the selection pane pre-fills the target's *current*
    /// type, not the type currently highlighted in its candidate list —
    /// picking from that list is what `Enter` is for.
    pub(super) fn prefill_override_cmd(&mut self) {
        let entry = if self.manage_open && self.manage_focus {
            match self.overrides.entries().get(self.manage_highlight) {
                Some(entry) => Some(entry.clone()),
                // Nothing highlighted because there is nothing to
                // highlight — no subject, so no command to pre-fill.
                None => return,
            }
        } else {
            None
        };

        // The node the pre-fill describes: the highlighted entry's first
        // affected node when editing an entry, else the selection pane's
        // target (which is the cursor when it was opened from the main
        // pane). An entry whose origin currently matches nothing still
        // gets a line — it just falls back for the schema-derived name.
        let from_manage = entry.is_some();
        let (origin, r#type, entry_name) = match entry {
            Some(entry) => (entry.origin, entry.r#type, entry.name),
            None => {
                let idx = self.override_cmd_subject();
                if !self.can_override(idx) {
                    self.message = "cannot override: not a message/group or length-delimited field"
                        .to_string();
                    return;
                }
                // Spec 0321 S1: the origin the pane's own status line is
                // projecting (spec 0309 S4) — pinned kind if `z`/`Z` set
                // one, else spec 0308's widest-first ladder. This branch
                // is reached only from the selection pane, and the pane
                // must not describe its subject one way in the status
                // line and another in the line `o` opens.
                //
                // This supersedes spec 0237 S4, which took the covering
                // entry's origin. Because the ladder is *widest*-first,
                // a node covered by an `fqdn:field` entry still projects
                // that entry's own origin, so S4's `o`-then-`Enter`
                // no-op survives where it was aimed; a node covered by a
                // narrower entry now widens, exactly as `t`-then-`Enter`
                // already does. `z`/`Z` is how a reader asks for the
                // narrow one, and the status line shows which is in
                // force before either key is pressed.
                //
                // The fallback matches the status line's (spec 0309 S4)
                // so that the two still agree when nothing projects.
                let origin =
                    self.projected_override_origin()
                        .unwrap_or_else(|_| OverrideOrigin::Path {
                            path: self.positional_path(idx),
                        });
                // The name follows the origin, not the node: the origin
                // decides which entry this line describes, which may no
                // longer be the one covering `idx`.
                let entry_name = self
                    .overrides
                    .entries()
                    .iter()
                    .find(|e| e.origin == origin)
                    .and_then(|e| e.name.clone());
                // Spec 0236 S7: pre-fill --as with what the node is
                // currently rendered as. When that is None (raw node
                // with no override and no natural type), fall back to
                // the type currently highlighted in the selection pane
                // so the user sees what they navigated to.
                let effective = self.effective_type(idx).or_else(|| {
                    self.override_candidates
                        .get(self.override_highlight)
                        .map(|(fqdn, _)| fqdn.clone())
                        .filter(|f| f != crate::decode::NONE_KEYWORD)
                });
                (origin, effective, entry_name)
            }
        };
        let node = self.origin_subject_node(&origin);

        let mut buf = String::from("override ");
        buf.push_str(&origin.label());
        if let Some(r#type) = &r#type {
            buf.push_str(" --as ");
            buf.push_str(r#type);
        }
        // Spec 0348 §S3: prefill --card only from the selection pane.
        // The management pane pre-fills a stored entry; adding --card
        // there would silently change entries that were stored without
        // one, breaking the o-then-Enter no-op invariant (spec 0236 S6).
        if !from_manage {
            let card = self.field_cardinality(node);
            buf.push_str(" --card ");
            buf.push_str(cardinality_str(card));
        }
        buf.push_str(" --field-name ");
        buf.push_str(&self.display_name_for(node, entry_name));
        self.open_command_line(CommandLineKind::Command, buf);
    }

    /// The node `:override` speaks about when the line names no origin:
    /// the selection pane's target while it is open, else the cursor.
    /// The two are the same node whenever the pane was opened with `t`,
    /// and differ only when it was opened from the manage pane — where
    /// the target, not the cursor, is what the pane is visibly about.
    fn override_cmd_subject(&self) -> usize {
        self.override_target.unwrap_or(self.cursor)
    }

    /// `--as`'s pre-filled value for a main-pane node (spec 0236 S7):
    /// what the node is being rendered as right now. `None` for a raw
    /// node, which is how the command spells "raw" (S4).
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

    /// `--field-name`'s pre-filled value (spec 0237 S5): the first of
    /// the four derivations that is available.
    fn display_name_for(&self, idx: usize, entry_name: Option<String>) -> String {
        // Never empty: derivation (4) always applies.
        self.field_name_candidates(idx, entry_name).swap_remove(0)
    }

    /// The four ways protolens can name a field (spec 0237 S7), in
    /// order, with duplicates dropped keeping the first occurrence.
    ///
    /// (1) and (2) coincide whenever the stored name came from the
    /// schema, which is exactly the case spec 0236 S8 normalizes away —
    /// without the dedup, Tab there would appear to do nothing. (3) and
    /// (4) were one candidate in spec 0236, spelled `f<position>`, which
    /// read as a field number and was not one.
    fn field_name_candidates(&self, idx: usize, entry_name: Option<String>) -> Vec<String> {
        // `field_number == 0` is the virtual-wrapper/root sentinel, not
        // a field number, so (3) has nothing to offer there.
        let field_number = self.tree[idx].span.field_number;
        let derivations = [
            entry_name,
            self.schema_field_name(idx),
            (field_number != 0).then(|| format!("f{field_number}")),
            Some(format!("p{}", self.sibling_position(idx))),
        ];
        let mut out: Vec<String> = Vec::new();
        for candidate in derivations.into_iter().flatten() {
            if !out.contains(&candidate) {
                out.push(candidate);
            }
        }
        out
    }

    /// The field name `idx` gets from its parent's schema, or `None`
    /// when the parent's type is unresolved or does not declare the
    /// field. Deliberately *not* `field_name_for`'s field-number
    /// fallback: S7 spells that derivation `f<N>` itself, and a bare
    /// number would round-trip into the entry as a display-name
    /// override.
    fn schema_field_name(&self, idx: usize) -> Option<String> {
        self.parent_field(idx).map(|f| f.name().to_string())
    }

    /// The node an origin speaks about: its first affected node, or the
    /// pane's own subject when it currently affects none. Used both to
    /// validate an edit and to derive the schema-derived name it is
    /// compared against, so that both answer for the same node.
    fn origin_subject_node(&self, origin: &OverrideOrigin) -> usize {
        self.manage_affected_nodes(origin)
            .first()
            .copied()
            .unwrap_or_else(|| self.override_cmd_subject())
    }

    /// `:override <origin> [--as <fqdn>] [--field-name <name>]` — spec
    /// 0237 S2/S3 over spec 0236 S9/S10/S11.
    pub(super) fn run_override_cmd(&mut self, args: Vec<&str>) {
        let parsed = match parse_override(&args) {
            Ok(parsed) => parsed,
            Err(e) => {
                self.message = e;
                return;
            }
        };
        let origin = parsed.origin;
        let subject = self.origin_subject_node(&origin);

        // Spec 0329 S3: the node whose row must not move, read here
        // because `close_override` below clears `override_target`.
        //
        // The *caret's* node, not `subject`. `origin_subject_node` gives
        // the origin's first match, which for an `fqdn-field` origin is
        // the topmost one in the document — anchoring there would hold a
        // node the reader may never have seen, and would leave the one
        // they were pointing at to move, which is the whole case S2's
        // signed term exists for.
        let anchored = self.override_cmd_subject();

        // Spec 0315 S2: before validation and before `activate`, so the
        // type exists by the time `render_overrides` resolves it. A
        // re-declaration is not an error (S3) — it only earns a note on
        // the message this command ends with.
        let mut note = String::new();
        if parsed.declare {
            let fqdn = parsed
                .r#type
                .clone()
                .expect("parse_override only sets `declare` alongside a type");
            match self.ctx.declare_type(&fqdn) {
                Ok(decode::Declared::Fresh) => {}
                Ok(decode::Declared::Reused) => {
                    note = format!("{fqdn} already declared — reusing it; ");
                }
                Err(e) => {
                    self.message = e;
                    return;
                }
            }
        }

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
            // Spec 0348 §S5/§S6: store explicit cardinality when present,
            // but normalize away a value that merely echoes what
            // `field_cardinality` would return — mirrors the `--field-name`
            // normalization (spec 0236 S8) so that accepting the pre-filled
            // line on a schema-cardinality field is still a no-op.
            let cardinality = parsed
                .cardinality
                .filter(|&c| c != self.field_cardinality(subject));
            self.overrides.set_cardinality(idx, cardinality);
        }

        // Spec 0321 S2: the command has just answered the question the
        // selection pane was asking, so the pane goes. Before the splice
        // rather than after, because `close_override` drops the preview
        // overlay and spec 0185 S6 forbids the overlay outliving a
        // render pass — its anchor is a row position the splice
        // invalidates. Every early return above leaves the pane up: a
        // refused command has answered nothing.
        //
        // This also restores the management pane when the selection pane
        // was opened from it (spec 0200 S2), which is why the highlight
        // below is set afterwards — `manage_open` is not true until now.
        if self.override_target.is_some() {
            self.close_override();
        }

        // Spec 0329 S3: a `path-field` or `fqdn-field` origin retypes
        // nodes *above* the caret too, which is exactly the case a
        // top-row anchor cannot hold.
        self.capture_target_scroll_anchor(anchored);
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
        self.message = format!("{note}{} as {as_what} — {nodes} {plural}", origin.label());
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
    /// be an eligible override target, and an override keyword must
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
        // Only a keyword is checked here — spec 0299's `message`
        // included, which is why this asks `is_override_keyword` rather
        // than `primitive_type_for_keyword`. An FQDN names a type the
        // pool has to resolve, and its fitness is not a wire-type
        // question.
        if !decode::is_override_keyword(name) {
            return Ok(());
        }
        let wire_type = decode::effective_wire_type(&self.tree[idx].span);
        if decode::override_keywords_for_wire_type(wire_type).contains(&name) {
            Ok(())
        } else {
            Err(format!(
                "type '{name}' not wire-compatible with this node's wire type"
            ))
        }
    }

    /// Tab-completion for `:override` (spec 0236 S14). Unlike every
    /// other command's, this one dispatches on the *token being
    /// completed* rather than on a fixed argument position: the flags
    /// may appear in either order, before or after the positional.
    ///
    /// `rest` is everything after the command name's first space, up to
    /// the cursor.
    pub(super) fn complete_override_cmd(&mut self, cmd: &str, rest: &str) {
        let token_byte = rest.rfind(' ').map_or(0, |i| i + 1);
        let (before, token) = rest.split_at(token_byte);
        let token_start = cmd.chars().count() + 1 + before.chars().count();
        match before.split_whitespace().next_back() {
            Some("--field-name") => self.complete_field_name(token_start, token, before),
            Some("--as") => self.complete_override_type(token_start, token, before),
            // Spec 0315 S7: `--as-new` completes nothing. It declares a
            // name; `--as` is where an existing one is picked, and
            // offering the existing ones here would offer exactly the
            // names S4 refuses.
            Some("--as-new") => {}
            // Spec 0348 §S4: unfiltered rotation through the three
            // cardinality literals, narrowest to widest.
            Some("--card") => self.complete_card(token_start, token),
            _ => self.complete_override_origin(token_start, token),
        }
    }

    /// `<origin>`'s completion (spec 0237 S6): an unfiltered rotation
    /// through the origin shapes `origin_for_kind` can build for the
    /// subject, narrowest first.
    ///
    /// Unfiltered because the token being completed is almost always
    /// the pre-filled origin, and prefix-matching it against the other
    /// two shapes would match nothing — leaving Tab dead exactly where
    /// it is most wanted. The three shapes are alternatives to each
    /// other, not entries in a namespace to search.
    ///
    /// Shapes that cannot be built are skipped rather than offered and
    /// then refused: `FqdnField` needs the parent's resolved type FQDN,
    /// which does not exist when the parent is raw, and `PathField`
    /// needs a parent at all. Free text is still *accepted* (spec 0236
    /// S5), for a pasted or saved origin; it is simply not what
    /// completion offers.
    fn complete_override_origin(&mut self, token_start: usize, prefix: &str) {
        // The token being completed *is* the origin, so it — not the
        // rest of the line — is what says which node to rotate around.
        let subject = match parse_origin(prefix) {
            Ok(origin) => self.origin_subject_node(&origin),
            Err(_) => self.override_cmd_subject(),
        };
        let labels: Vec<String> = ORIGIN_KINDS
            .into_iter()
            .filter_map(|kind| self.origin_for_kind(subject, kind).ok())
            .map(|origin| origin.label())
            .collect();
        self.apply_rotation(token_start, prefix, labels);
    }

    /// `--field-name`'s completion (spec 0237 S7): an unfiltered
    /// rotation through the four derivations of the subject's display
    /// name. Unfiltered for the same reason `<origin>` is.
    fn complete_field_name(&mut self, token_start: usize, prefix: &str, before: &str) {
        let subject = self.completion_subject(before);
        let entry_name = self
            .resolve_active_override_entry(subject)
            .and_then(|e| e.name.clone());
        let candidates = self.field_name_candidates(subject, entry_name);
        self.apply_rotation(token_start, prefix, candidates);
    }

    /// `--card`'s completion (spec 0348 §S4): unfiltered rotation through
    /// the three cardinality literals in `optional` → `repeated` →
    /// `required` order, identical in style to `complete_field_name`.
    fn complete_card(&mut self, token_start: usize, prefix: &str) {
        let candidates = vec![
            "optional".to_string(),
            "repeated".to_string(),
            "required".to_string(),
        ];
        self.apply_rotation(token_start, prefix, candidates);
    }

    /// `--as`'s completion (spec 0237 S8): prefix-match the inferred
    /// candidates for the subject, in decreasing score order; only if
    /// that yields nothing, prefix-match the lexicographic list.
    ///
    /// The two lists are tried in sequence rather than concatenated: a
    /// prefix that matches an inferred type must not also drag in the
    /// hundreds of unranked FQDNs sharing it, which is the whole value
    /// of ranking them.
    ///
    /// A cache miss queues the scoring request and yields an empty
    /// inferred list, so the fallback runs — silently (spec 0237 N2). A
    /// completer that sometimes ignores a keystroke is worse than one
    /// whose order is sometimes alphabetical.
    fn complete_override_type(&mut self, token_start: usize, prefix: &str, before: &str) {
        let subject = self.completion_subject(before);
        let range = self.heat_scored_range(subject);
        // The same window at the same tier the selection pane reads:
        // one screenful of `top_n`, jumping the queue because this
        // directly follows a keystroke. With no scoring graph there is
        // never an entry, so this is a miss that pushes nothing (there
        // is no worker either) and the fallback runs.
        let inferred: Vec<String> = self
            .heat_lookup(&range, None, 0, self.override_list_height, Tier::User)
            .unwrap_or_default()
            .into_iter()
            .map(|(fqdn, _)| fqdn)
            .filter(|fqdn| fqdn.starts_with(prefix))
            .collect();
        if inferred.is_empty() {
            self.complete_type_at(subject, token_start, prefix);
            return;
        }
        // Deliberately unsorted: decreasing score *is* the order.
        self.apply_completion(token_start, prefix.chars().count(), inferred);
    }

    /// The node the flag completers speak about: whatever origin is
    /// already on the line, else the pane's own subject. The line's
    /// positional is the better answer — in the manage pane the
    /// pre-filled entry's origin routinely names a node the main-pane
    /// cursor is nowhere near.
    fn completion_subject(&self, before: &str) -> usize {
        match line_origin(before) {
            Some(origin) => self.origin_subject_node(&origin),
            None => self.override_cmd_subject(),
        }
    }
}
