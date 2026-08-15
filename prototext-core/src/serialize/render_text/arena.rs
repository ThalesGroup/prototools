// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0216 — the structural decomposition of a wire blob, in two phases.
//!
//! **Phase 1** produces one node per field occurrence, in the order the
//! bytes are read. **Phase 2** sorts those into the level order the arena
//! requires. Only phase 2's output crosses out of this crate; phase 1's
//! arrays are internal and are dropped as soon as the sort is done (S25).
//!
//! The walk is `render_message`'s own recursion driven by an arena-shaped
//! [`Sink`], not a second traversal (S15). That is what makes the
//! decomposition and the render agree on every boundary — where a
//! malformed tail begins, how an unterminated `START_GROUP` recovers —
//! which is the property the whole design rests on. The one behavioral
//! difference, always recursing into an unknown length-delimited payload,
//! is routed through `Sink::unknown_len_is_message` (S14).

use std::ops::Range;

use super::sink::{
    narrow, GroupCloseFacts, MalformedKind, NestedKind, ScalarValue, Sink, TagFacts,
};
use super::{
    render_message, FieldOrExt, DEPTH, EXPAND_ANY, EXPAND_MESSAGE_SET, HIDE_UNKNOWN,
    MAX_INDEXED_BUFFER, MAX_WIRE_DEPTH,
};
use crate::CodecError;

/// A node's depth is stored as a `u16`, which only holds while the
/// recursion cap keeps depths small.
const _: () = assert!(MAX_WIRE_DEPTH <= u16::MAX as usize);

/// Deepest node the walk may produce (S9).
///
/// The arena's `depth` is zero-based while `DEPTH` counts `render_message`
/// frames and the outermost frame is frame 1, so a node at depth `d` is
/// walked with `DEPTH == d + 1`. `at_depth_cap` degrades once
/// `DEPTH >= MAX_WIRE_DEPTH`, which is depth `MAX_WIRE_DEPTH - 1`. Reaching
/// that depth is therefore exactly the condition under which the render may
/// have handed a payload back without descending into it — so the walk
/// refuses at it rather than after it, and no degraded node can ever be
/// recorded.
const MAX_NODE_DEPTH: u16 = (MAX_WIRE_DEPTH - 1) as u16;

/// Phase 1's output: five parallel arrays, one entry per node, in document
/// order (S8, S18).
///
/// `parent` holds a slot index into these same arrays; a depth-0 node is
/// its own parent, which terminates the climb without a sentinel.
pub(super) struct DocumentOrderNodes {
    pub(super) parent: Vec<u32>,
    pub(super) depth: Vec<u16>,
    pub(super) raw_start: Vec<u32>,
    pub(super) raw_end: Vec<u32>,
    /// Spec 0097's unknown-LEN probe verdict for this node's payload, as
    /// a by-product of the walk that was already reading those bytes —
    /// see [`Arena::probes_as_message`]. `false` for every node that is
    /// not a nested one, where the question does not arise.
    pub(super) probes_as_message: Vec<bool>,
}

impl DocumentOrderNodes {
    pub(super) fn len(&self) -> usize {
        self.parent.len()
    }
}

/// What an open node must restore when its subtree is done.
///
/// `raw_base` is the coordinate frame: `render_message` recurses on
/// re-sliced payloads, so the offsets a `Sink` receives restart at zero
/// inside every nested message. `payload_start` is the shift from the
/// parent's frame to the child's (`0` for a group, which shares its
/// parent's frame), and adding it up the stack is what turns local offsets
/// into absolute ones — the same bookkeeping `IndexingTextSink` does.
struct ArenaMark {
    slot: u32,
    raw_base: usize,
    parent: u32,
    depth: u16,
}

struct ArenaSink {
    nodes: DocumentOrderNodes,
    raw_base: usize,
    parent: u32,
    depth: u16,
    max_depth: u16,
}

impl ArenaSink {
    /// `capacity` is a slot count. S24: the worst case is one node per
    /// byte, since every node's tag costs at least one byte, so reserving
    /// `blob.len() + 1` removes reallocation outright at the price of
    /// address space that is never faulted in.
    fn with_capacity(capacity: usize) -> Self {
        ArenaSink {
            nodes: DocumentOrderNodes {
                parent: Vec::with_capacity(capacity),
                depth: Vec::with_capacity(capacity),
                raw_start: Vec::with_capacity(capacity),
                raw_end: Vec::with_capacity(capacity),
                probes_as_message: Vec::with_capacity(capacity),
            },
            raw_base: 0,
            parent: 0,
            depth: 0,
            max_depth: 0,
        }
    }

    /// Append one slot and return its index.
    fn push(&mut self, raw_start: u32, raw_end: u32) -> u32 {
        let slot = self.nodes.len() as u32;
        let parent = if self.depth == 0 { slot } else { self.parent };
        self.nodes.parent.push(parent);
        self.nodes.depth.push(self.depth);
        self.nodes.raw_start.push(raw_start);
        self.nodes.raw_end.push(raw_end);
        // A scalar or malformed node is not a nested one, so no probe
        // verdict applies; `end_nested` overwrites this for the slots
        // where the question is meaningful.
        self.nodes.probes_as_message.push(false);
        if self.depth > self.max_depth {
            self.max_depth = self.depth;
        }
        slot
    }

    /// Absolute range of a local one, in the frame currently active.
    fn absolute(&self, raw_range: Range<usize>) -> (u32, u32) {
        (
            narrow(self.raw_base + raw_range.start),
            narrow(self.raw_base + raw_range.end),
        )
    }
}

impl Sink for ArenaSink {
    type Mark = ArenaMark;

    fn scalar_field(
        &mut self,
        _field_number: u64,
        _field_schema: Option<&FieldOrExt>,
        _tag: TagFacts,
        _value: ScalarValue<'_>,
        raw_range: Range<usize>,
        _schema_present: bool,
    ) {
        // The value is not looked at. A packed record is one node, not one
        // per element (S6), and a `ScalarValue::Bytes` payload is a
        // childless node exactly like a varint leaf (S3) — refusal and
        // emptiness are the same shape here.
        let (start, end) = self.absolute(raw_range);
        self.push(start, end);
    }

    fn begin_nested(
        &mut self,
        _field_number: u64,
        _field_schema: Option<&FieldOrExt>,
        _tag: TagFacts,
        kind: NestedKind,
        raw_start: usize,
        payload_start: usize,
    ) -> ArenaMark {
        let raw_base = self.raw_base;
        let start = narrow(raw_base + raw_start);
        // The slot is claimed on the way down so that document order is the
        // order the bytes are read; `raw_end` is unknown until the subtree's
        // walk returns and is backpatched by `end_nested`. That is what lets
        // groups need no special case (S20): a group carries no length
        // prefix, and its extent is simply where its own walk stopped.
        let slot = self.push(start, start);
        // The one thing about this node the bytes alone do not say: what
        // spec 0097's cascade would have made of it. The walk descends
        // either way (S14), so the verdict has to be recorded here or lost.
        // `Group` and a schema-decided message never faced a probe.
        self.nodes.probes_as_message[slot as usize] = matches!(
            kind,
            NestedKind::Message {
                probed_as_message: Some(true)
            }
        );
        let mark = ArenaMark {
            slot,
            raw_base,
            parent: self.parent,
            depth: self.depth,
        };
        self.raw_base = raw_base + payload_start;
        self.parent = slot;
        self.depth += 1;
        mark
    }

    fn end_nested(
        &mut self,
        mark: ArenaMark,
        raw_range: Range<usize>,
        _close_facts: Option<GroupCloseFacts>,
    ) {
        self.raw_base = mark.raw_base;
        self.parent = mark.parent;
        self.depth = mark.depth;
        self.nodes.raw_end[mark.slot as usize] = narrow(mark.raw_base + raw_range.end);
    }

    fn virtual_scalar(
        &mut self,
        _name: &str,
        _annotation: Option<&str>,
        _value_str: &str,
        _raw_range: Range<usize>,
    ) {
        unreachable!("{VIRTUAL_UNREACHABLE}");
    }

    fn begin_virtual_nested(
        &mut self,
        _name: &str,
        _annotation: Option<&str>,
        _type_fqdn: Option<&str>,
        _raw_start: usize,
        _payload_start: usize,
    ) -> ArenaMark {
        unreachable!("{VIRTUAL_UNREACHABLE}");
    }

    fn malformed(
        &mut self,
        _field_number: u64,
        _tag: TagFacts,
        kind: MalformedKind,
        raw: &[u8],
        raw_range: Range<usize>,
    ) {
        // S4: a malformed region is a node like any other, at its position
        // among its siblings. Where such a region begins is fixed by the
        // bytes, so that position is the same under every interpretation.
        //
        // Spec 0302: a TruncatedBytes field is descended into like any other
        // LEN field — the available bytes may contain valid sub-fields, and a
        // `message` override (spec 0299) needs arena slots for them. `raw` is
        // exactly the available payload bytes (from after the length varint to
        // end-of-buffer), which `render_message` can walk directly.
        // `raw_range.end - raw.len()` is the payload offset in the current
        // frame (= the position right after the length varint).
        // Empty payload: no children can exist; fall through to the leaf path.
        if matches!(kind, MalformedKind::TruncatedBytes { .. }) && !raw.is_empty() {
            let (start, end) = self.absolute(raw_range.clone());
            let slot = self.push(start, start); // raw_end backpatched below
            let payload_offset = raw_range.end - raw.len();
            let saved = (self.raw_base, self.parent, self.depth);
            self.raw_base += payload_offset;
            self.parent = slot;
            self.depth += 1;
            render_message(raw, 0, None, None, false, self);
            (self.raw_base, self.parent, self.depth) = saved;
            self.nodes.raw_end[slot as usize] = end;
            return;
        }
        let (start, end) = self.absolute(raw_range);
        self.push(start, end);
    }

    fn unknown_len_is_message(&self) -> bool {
        true
    }

    fn tracks_level(&self) -> bool {
        false
    }
}

/// Both virtual-node hooks exist for `Any` and `MessageSet` expansion, and
/// both of those live in `render_len_field`'s *schema-driven* branch, behind
/// a thread-local switch. This walk passes no schema and turns both switches
/// off, so neither can fire.
const VIRTUAL_UNREACHABLE: &str =
    "the arena walk is schemaless with Any and MessageSet expansion off, \
     so no virtual node can be emitted";

/// Decompose `buf` into one node per field occurrence, in document order
/// (spec 0216 phase 1).
///
/// The walk descends into *every* length-delimited payload whatever it
/// looks like, and never declines on a heuristic (S2). That is what makes
/// the result maximal, and maximality is what makes it a superset of every
/// render: a payload judged not to be a message is one that a schema — or a
/// user override, which is the point of the app — could still declare one,
/// and the render would then need nodes the decomposition never created.
///
/// Fails if `buf` exceeds `MAX_INDEXED_BUFFER`, the bound that keeps the
/// `u32` offsets sound (S18), or if the tree reaches the recursion cap (S9).
pub(super) fn walk_document_order(buf: &[u8]) -> Result<DocumentOrderNodes, CodecError> {
    if buf.len() > MAX_INDEXED_BUFFER {
        return Err(CodecError::InputTooLarge {
            len: buf.len(),
            max: MAX_INDEXED_BUFFER,
        });
    }

    // Render-mode state this walk depends on. `HIDE_UNKNOWN` would drop
    // nodes; the two expansion switches would produce virtual nodes. All
    // three are ambient thread-locals left over from whatever ran last, so
    // they are set rather than assumed. The rest — `LEVEL`, `ANNOTATIONS`,
    // `INDENT_SIZE`, `CBL_START` — is text formatting that `ArenaSink`
    // never reads, and `tracks_level` keeps the walk from disturbing it.
    HIDE_UNKNOWN.with(|c| c.set(false));
    EXPAND_ANY.with(|c| c.set(false));
    EXPAND_MESSAGE_SET.with(|c| c.set(false));
    DEPTH.with(|c| c.set(0));

    let mut sink = ArenaSink::with_capacity(buf.len() + 1);
    render_message(buf, 0, None, None, false, &mut sink);

    if sink.max_depth >= MAX_NODE_DEPTH {
        return Err(CodecError::InputTooDeep {
            max: MAX_WIRE_DEPTH,
        });
    }

    // S24: hand the over-reservation back. Every array is well past
    // glibc's mmap threshold at any interesting blob size, so this should
    // be a tail unmap rather than a copy.
    let mut nodes = sink.nodes;
    nodes.parent.shrink_to_fit();
    nodes.depth.shrink_to_fit();
    nodes.raw_start.shrink_to_fit();
    nodes.raw_end.shrink_to_fit();
    nodes.probes_as_message.shrink_to_fit();
    Ok(nodes)
}

// ── Phase 2 — the sort into level order ──────────────────────────────────────

/// `first_child` slot not yet written. `0` can never be a real value: slot 0
/// is always a root, so no node's first child is slot 0.
const NO_FIRST_CHILD: u32 = 0;

/// The finished arena: the structure of a blob, and nothing else (S18, S19).
///
/// Children of slot `i` are the slots `first_child[i] .. first_child[i + 1]`,
/// so a child count is a subtraction and a sibling ordinal is
/// `i - first_child[parent[i]]`. A root is its own parent, which terminates
/// the climb without a sentinel value.
///
/// The arrays live in **one** allocation, sliced rather than owned
/// separately: they are built together, have identical lifetimes, and the
/// passes over them are disjoint, so one `malloc` and one first touch beat
/// four (S8).
pub struct Arena {
    /// `first_child` (n + 1), then `parent`, `raw_start`, `raw_end` (n
    /// each), then the `probes_as_message` bitset (`n.div_ceil(32)`).
    cells: Vec<u32>,
    len: usize,
}

impl Arena {
    /// Number of nodes.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `n + 1` entries: children of `i` are `first_child[i]..first_child[i+1]`.
    pub fn first_child(&self) -> &[u32] {
        &self.cells[..self.len + 1]
    }

    /// Each node's parent slot; a root is its own parent.
    pub fn parent(&self) -> &[u32] {
        &self.cells[self.len + 1..2 * self.len + 1]
    }

    /// First byte of each node's *tag* — not of its payload (S19).
    pub fn raw_start(&self) -> &[u32] {
        &self.cells[2 * self.len + 1..3 * self.len + 1]
    }

    /// One past each node's last byte.
    pub fn raw_end(&self) -> &[u32] {
        &self.cells[3 * self.len + 1..4 * self.len + 1]
    }

    /// Whether spec 0097's unknown-LEN cascade would render this node's
    /// payload as a nested message rather than as a string or bytes.
    ///
    /// The walk that builds the arena descends into every LEN payload
    /// whatever it looks like — that is what makes the arena maximal — so
    /// the arena's *shape* cannot answer this. The verdict is recorded
    /// alongside it instead, taken verbatim from the one place that
    /// computes it (`render_len_field`) rather than re-derived here, so
    /// that the two can never come to disagree about what the cascade
    /// would do.
    ///
    /// Two limits are worth stating rather than inferring:
    ///
    /// - It answers the *schema-free* question only. A LEN field whose
    ///   descriptor says `message` is rendered as one with no probe at
    ///   all, and a caller with a schema should not consult this.
    /// - It is `false` for every node that is not a nested one — a scalar,
    ///   a packed run, a malformed region — where the question does not
    ///   arise. `false` therefore means "not a message *or* not nested",
    ///   and the caller is expected to know which it is looking at.
    pub fn probes_as_message(&self, slot: usize) -> bool {
        debug_assert!(slot < self.len, "slot out of range");
        let word = self.cells[4 * self.len + 1 + slot / 32];
        word & (1 << (slot % 32)) != 0
    }
}

/// Sort phase 1's document-order nodes into level order (S8 phase 2, S16).
///
/// A counting sort, not a comparison sort: the key is a depth, a small
/// integer bounded by the recursion cap, so it buckets rather than compares
/// and the whole pass is O(n + D).
///
/// **Depth alone is a sufficient key.** Within a level, document order
/// already *is* the order parent order induces. Take two nodes at depth
/// *d+1* whose parents P₁ and P₂ sit at depth *d* with P₁ before P₂: all of
/// P₁'s subtree precedes P₂ in document order, so P₁'s children precede
/// P₂'s. And nothing at depth *d+1* can fall between two children of one
/// parent, since anything between them in document order is a descendant of
/// the earlier one and so at depth *d+2* or below. Stability is not
/// engineered either — it falls out of visiting in document order with
/// bucket cursors that only advance. The result is exactly S16: sibling
/// blocks contiguous, blocks ordered by parent slot.
fn sort_into_level_order(nodes: DocumentOrderNodes) -> Arena {
    let n = nodes.len();
    let DocumentOrderNodes {
        parent,
        depth,
        raw_start,
        raw_end,
        probes_as_message,
    } = nodes;

    // 1-2. Count per depth, then prefix-sum in place so that `base[d]` is
    // the first slot of level `d`. At most `MAX_WIRE_DEPTH + 1` entries, so
    // a 4 KB array however large the blob.
    let levels = depth.iter().copied().max().map_or(0, |d| d as usize + 1);
    let mut base = vec![0u32; levels + 1];
    for &d in &depth {
        base[d as usize] += 1;
    }
    let mut acc = 0;
    for slot in base.iter_mut() {
        let count = *slot;
        *slot = acc;
        acc += count;
    }

    // 3. Scatter: each node takes the next free slot of its level.
    let mut new_index = vec![0u32; n];
    for (i, &d) in depth.iter().enumerate() {
        let cursor = &mut base[d as usize];
        new_index[i] = *cursor;
        *cursor += 1;
    }

    // One exact-size allocation for all the output arrays. `n` is known
    // now, so nothing here grows or reallocates. The probe verdicts go in
    // as a bitset: one bit per slot rather than one word costs a fifth of
    // the arena's whole footprint against a thirtieth of it.
    let mut cells = vec![0u32; 4 * n + 1 + n.div_ceil(32)];
    {
        let (first_child, rest) = cells.split_at_mut(n + 1);
        let (out_parent, rest) = rest.split_at_mut(n);
        let (out_start, rest) = rest.split_at_mut(n);
        let (out_end, out_probe) = rest.split_at_mut(n);

        for i in 0..n {
            let j = new_index[i] as usize;
            out_parent[j] = new_index[parent[i] as usize];
            out_start[j] = raw_start[i];
            out_end[j] = raw_end[i];
            if probes_as_message[i] {
                out_probe[j / 32] |= 1 << (j % 32);
            }
        }

        // `first_child`, in two sweeps. Forward: a parent's children are
        // contiguous, so the first slot claiming `p` is where `p`'s block
        // starts. Slot 0 is skipped and self-parenting roots with it —
        // a root's own slot is not its first child.
        for (j, &p) in out_parent.iter().enumerate().skip(1) {
            let p = p as usize;
            if p != j && first_child[p] == NO_FIRST_CHILD {
                first_child[p] = j as u32;
            }
        }
        // Backward: a childless node takes the value of the next node that
        // has one, which is what makes `first_child[i+1] - first_child[i]`
        // the child count for every node rather than only for parents.
        let mut next = n as u32;
        first_child[n] = next;
        for slot in first_child[..n].iter_mut().rev() {
            if *slot == NO_FIRST_CHILD {
                *slot = next;
            } else {
                next = *slot;
            }
        }
    }

    Arena { cells, len: n }
}

/// Decompose `buf` into the arena its bytes determine (spec 0216).
///
/// The structure is a function of the bytes alone: no schema, no override
/// and no rendering choice takes part, which is what lets one arena serve
/// every interpretation of the same blob.
///
/// Fails if `buf` is larger than `MAX_INDEXED_BUFFER` (S18) or nests as
/// deep as the walk's recursion cap (S9).
pub fn build_arena(buf: &[u8]) -> Result<Arena, CodecError> {
    Ok(sort_into_level_order(walk_document_order(buf)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::{parse_varint, parse_wiretag, WT_LEN};

    /// One length-delimited field: a single-byte tag, a varint length, the
    /// payload. The length is a real varint because the deep-nesting tests
    /// below build payloads far past 127 bytes.
    fn len_field(field: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![(field << 3) as u8 | 2];
        let mut len = payload.len() as u64;
        while len >= 0x80 {
            out.push((len as u8) | 0x80);
            len >>= 7;
        }
        out.push(len as u8);
        out.extend_from_slice(payload);
        out
    }

    /// `(depth, raw_start, raw_end, parent)` per node, in document order.
    fn shape(nodes: &DocumentOrderNodes) -> Vec<(u16, u32, u32, u32)> {
        (0..nodes.len())
            .map(|i| {
                (
                    nodes.depth[i],
                    nodes.raw_start[i],
                    nodes.raw_end[i],
                    nodes.parent[i],
                )
            })
            .collect()
    }

    /// A node's range runs from the first byte of its *tag*, not of its
    /// payload (S19) — that is what makes everything the tag says one parse
    /// away, so no flag has to be stored. Offsets are absolute despite
    /// `render_message` recursing on re-sliced payloads.
    #[test]
    fn ranges_are_absolute_and_start_at_the_tag() {
        // 1 { 2 { 3: 42 } }
        let leaf = vec![0x18, 0x2A]; // field 3, varint, 42
        let mid = len_field(2, &leaf);
        let buf = len_field(1, &mid);
        assert_eq!(buf.len(), 6);

        let nodes = walk_document_order(&buf).expect("walks");
        assert_eq!(
            shape(&nodes),
            vec![
                (0, 0, 6, 0), // field 1: whole buffer, its own parent
                (1, 2, 6, 0), // field 2: tag at 2, runs to the end
                (2, 4, 6, 1), // field 3: the varint leaf
            ]
        );
    }

    /// S2/S14. Five bytes that spec 0097's probe declines as a message —
    /// read as wire format `"hello"` is a varint tag for field 13 and then
    /// an unmatched `END_GROUP` — still yield child nodes here, because the
    /// walk never judges.
    #[test]
    fn an_unknown_payload_is_descended_into_regardless() {
        let buf = len_field(1, b"hello");
        let nodes = walk_document_order(&buf).expect("walks");
        assert!(
            nodes.len() > 1,
            "greedy descent must produce children, got {}",
            nodes.len()
        );
        assert_eq!(nodes.depth[0], 0);
        assert!(nodes.depth[1..].iter().all(|&d| d == 1));
    }

    /// S3. A payload with nothing readable in it is a childless node, the
    /// same shape a scalar leaf has — the walk has no failure mode.
    #[test]
    fn an_empty_payload_is_a_childless_node() {
        let buf = len_field(1, b"");
        let nodes = walk_document_order(&buf).expect("walks");
        assert_eq!(shape(&nodes), vec![(0, 0, 2, 0)]);
    }

    /// S20. A group carries no length prefix, so its extent is only known
    /// once its body has been walked; the slot is claimed on the way down
    /// and its end backpatched on the way up.
    #[test]
    fn a_group_gets_its_extent_backpatched() {
        // 1 { 2: 42 } as a group: START_GROUP(1), varint field 2, END_GROUP(1)
        let buf = vec![0x0B, 0x10, 0x2A, 0x0C];
        let nodes = walk_document_order(&buf).expect("walks");
        assert_eq!(
            shape(&nodes),
            vec![
                (0, 0, 4, 0), // the group, closing tag included
                (1, 1, 3, 0), // the varint inside it
            ]
        );
    }

    /// S4. A malformed region gets a node at its position among its
    /// siblings, so the sibling ordinals either side of it are the same as
    /// they would be under any other reading of the bytes.
    #[test]
    fn a_malformed_region_gets_a_node() {
        // A well-formed varint, then a tag claiming wire type 7.
        let buf = vec![0x08, 0x2A, 0x0F];
        let nodes = walk_document_order(&buf).expect("walks");
        assert_eq!(shape(&nodes), vec![(0, 0, 2, 0), (0, 2, 3, 1)]);
    }

    /// S9. Refusal, not truncation: a tree that reaches the recursion cap
    /// is where the renderer stops descending, so a decomposition taken
    /// there would be missing exactly the nodes an override could ask for.
    ///
    /// Release-only, like the other deep-nesting tests in this crate: a
    /// debug `render_message` frame is roughly eight times as wide and a
    /// walk this deep overflows a default thread stack outright.
    #[test]
    #[cfg(not(debug_assertions))]
    fn a_tree_at_the_recursion_cap_is_refused() {
        let mut payload = vec![0x08, 0x2A];
        for _ in 0..MAX_WIRE_DEPTH {
            payload = len_field(1, &payload);
        }
        match walk_document_order(&payload) {
            Err(CodecError::InputTooDeep { max }) => assert_eq!(max, MAX_WIRE_DEPTH),
            Err(other) => panic!("expected InputTooDeep, got {other:?}"),
            Ok(nodes) => panic!("expected InputTooDeep, got {} nodes", nodes.len()),
        }
    }

    /// The other side of the same bound: a tree just under the cap walks,
    /// and every node it produced is one the renderer would also have
    /// descended into.
    #[test]
    #[cfg(not(debug_assertions))]
    fn a_tree_just_under_the_cap_walks() {
        let mut payload = vec![0x08, 0x2A];
        // The leaf sits one level below the last wrapper, so `depth`
        // reaches `MAX_NODE_DEPTH - 1`.
        for _ in 0..MAX_WIRE_DEPTH - 2 {
            payload = len_field(1, &payload);
        }
        let nodes = walk_document_order(&payload).expect("walks");
        assert_eq!(*nodes.depth.last().expect("a leaf"), MAX_NODE_DEPTH - 1);
    }

    /// The invariants phase 2 will rely on, checked against a real blob
    /// rather than a hand-built one: 18 KB of `FileDescriptorSet`, walked
    /// with no schema, which under always-recurse means most of it is
    /// strings being read as messages.
    #[test]
    fn a_real_blob_comes_out_in_document_order() {
        let buf = include_bytes!("../../../fixtures/descriptor.pb");
        let nodes = walk_document_order(buf).expect("walks");

        // Enough of the blob is descended into that this is exercising the
        // recursion, not just the top level. The two bracketing group
        // recovery rules in `examples/maximal_walk.rs` put the count at
        // 3 662 (flat) to 3 702 (nested) for this fixture; `render_message`
        // recovers differently again, so the bracket is the assertion.
        assert!(
            (3_000..=4_500).contains(&nodes.len()),
            "got {} nodes",
            nodes.len()
        );

        let mut deepest = 0;
        for i in 0..nodes.len() {
            let p = nodes.parent[i] as usize;
            // Document order: a parent is always allocated before its
            // children, and the root's self-reference is the only fixed
            // point. This is what phase 2's scatter will depend on.
            assert!(p <= i, "node {i} has a forward parent {p}");
            assert_eq!(p == i, nodes.depth[i] == 0, "node {i}: root-ness");
            if p != i {
                assert_eq!(nodes.depth[i], nodes.depth[p] + 1, "node {i}: depth");
                // Containment: a child's bytes lie inside its parent's, and
                // a node's range starts at its tag (S19), so the parent's
                // start is strictly the earlier one.
                assert!(nodes.raw_start[p] < nodes.raw_start[i], "node {i}: start");
                assert!(nodes.raw_end[i] <= nodes.raw_end[p], "node {i}: end");
            }
            assert!(nodes.raw_start[i] <= nodes.raw_end[i], "node {i}: empty");
            if i > 0 {
                assert!(
                    nodes.raw_start[i - 1] <= nodes.raw_start[i],
                    "node {i} starts before its predecessor"
                );
            }
            deepest = deepest.max(nodes.depth[i]);
        }
        assert!(deepest >= 5, "expected real nesting, got {deepest}");
    }

    #[test]
    fn an_oversized_buffer_is_refused() {
        // `Vec::with_capacity` is never reached: the bound is checked first,
        // so this allocates the input and nothing else.
        let oversize = vec![0u8; MAX_INDEXED_BUFFER + 1];
        match walk_document_order(&oversize) {
            Err(CodecError::InputTooLarge { len, max }) => {
                assert_eq!(len, MAX_INDEXED_BUFFER + 1);
                assert_eq!(max, MAX_INDEXED_BUFFER);
            }
            Err(other) => panic!("expected InputTooLarge, got {other:?}"),
            Ok(nodes) => panic!("expected InputTooLarge, got {} nodes", nodes.len()),
        }
    }

    // ── Phase 2 ─────────────────────────────────────────────────────────

    /// Depth of every slot, by climbing. Only sound on a level-ordered
    /// arena, where a non-root's parent always precedes it — which is the
    /// first thing every caller asserts.
    fn depths(arena: &Arena) -> Vec<u16> {
        let parent = arena.parent();
        let mut out = vec![0u16; arena.len()];
        for j in 0..arena.len() {
            let p = parent[j] as usize;
            if p != j {
                out[j] = out[p] + 1;
            }
        }
        out
    }

    /// Everything S16 and S18 claim about the finished layout, checked as
    /// one pass so that no caller has to remember the list.
    fn assert_level_ordered(arena: &Arena) {
        let n = arena.len();
        let (first_child, parent) = (arena.first_child(), arena.parent());
        assert_eq!(first_child.len(), n + 1);
        assert_eq!(first_child[n], n as u32, "the trailing sentinel");

        let depth = depths(arena);
        let mut children_seen = 0;
        for i in 0..n {
            let p = parent[i] as usize;
            assert!(p <= i, "slot {i}: a parent must precede its child");
            assert_eq!(p == i, depth[i] == 0, "slot {i}: root-ness");
            if i > 0 {
                assert!(depth[i - 1] <= depth[i], "slot {i}: depth must not fall");
                assert!(
                    first_child[i - 1] <= first_child[i],
                    "slot {i}: first_child must not fall"
                );
            }
            // Sibling blocks: the range `first_child[i]..first_child[i+1]`
            // holds exactly the slots that name `i` as their parent.
            let block = first_child[i] as usize..first_child[i + 1] as usize;
            children_seen += block.len();
            for c in block {
                assert_eq!(parent[c], i as u32, "slot {c} is not a child of {i}");
                assert_eq!(depth[c], depth[i] + 1, "slot {c}: depth under {i}");
            }
        }
        // Every non-root appears in exactly one block, so the blocks
        // partition the arena and `first_child` is a bijection onto it.
        let roots = (0..n).filter(|&i| parent[i] as usize == i).count();
        assert_eq!(children_seen, n - roots, "blocks must partition the arena");
    }

    /// The smallest case where document order and level order actually
    /// differ: two top-level messages, each with one child. Document order
    /// interleaves them (A, A's child, B, B's child); level order does not.
    #[test]
    fn the_sort_moves_document_order_into_level_order() {
        let a = len_field(1, &[0x18, 0x07]); // 1 { 3: 7 }
        let b = len_field(2, &[0x20, 0x08]); // 2 { 4: 8 }
        let mut buf = a.clone();
        buf.extend_from_slice(&b);
        assert_eq!(buf.len(), 8);

        let doc = walk_document_order(&buf).expect("walks");
        assert_eq!(
            doc.raw_start,
            vec![0, 2, 4, 6],
            "document order interleaves"
        );

        let arena = build_arena(&buf).expect("builds");
        assert_level_ordered(&arena);
        assert_eq!(arena.len(), 4);
        // A, B, then A's child, then B's child.
        assert_eq!(arena.raw_start(), [0, 4, 2, 6]);
        assert_eq!(arena.raw_end(), [4, 8, 4, 8]);
        assert_eq!(arena.parent(), [0, 1, 0, 1]);
        assert_eq!(arena.first_child(), [2, 3, 4, 4, 4]);
    }

    #[test]
    fn a_flat_blob_is_already_level_ordered() {
        let buf = vec![0x08, 0x2A, 0x10, 0x07];
        let arena = build_arena(&buf).expect("builds");
        assert_level_ordered(&arena);
        assert_eq!(arena.parent(), [0, 1], "two roots, each its own parent");
        assert_eq!(arena.first_child(), [2, 2, 2], "and neither has children");
    }

    /// Groups are the case a level-by-level build could not have handled at
    /// all — no length prefix, so the extent is only known once the body has
    /// been walked. Depth-first plus a sort has no such problem.
    #[test]
    fn nested_groups_sort_like_anything_else() {
        // 1 { 2 { 3: 42 } 4: 7 }, all as groups.
        let buf = vec![
            0x0B, // START_GROUP 1
            0x13, // START_GROUP 2
            0x18, 0x2A, // 3: 42
            0x14, // END_GROUP 2
            0x20, 0x07, // 4: 7
            0x0C, // END_GROUP 1
        ];
        let arena = build_arena(&buf).expect("builds");
        assert_level_ordered(&arena);
        assert_eq!(arena.len(), 4);
        assert_eq!(arena.raw_start(), [0, 1, 5, 2]);
        assert_eq!(arena.raw_end(), [8, 5, 7, 4]);
    }

    #[test]
    fn an_empty_blob_gives_an_empty_arena() {
        let arena = build_arena(&[]).expect("builds");
        assert!(arena.is_empty());
        assert_eq!(arena.first_child(), [0], "only the sentinel");
        assert_level_ordered(&arena);
    }

    /// The same real blob phase 1 is checked against, now through the sort.
    /// Phase 1's output is the reference: the two must agree on the node
    /// count and on the multiset of byte ranges, since the sort is a
    /// permutation and nothing else.
    #[test]
    fn a_real_blob_sorts_into_level_order() {
        let buf = include_bytes!("../../../fixtures/descriptor.pb");
        let doc = walk_document_order(buf).expect("walks");
        let arena = build_arena(buf).expect("builds");

        assert_eq!(arena.len(), doc.len());
        assert_level_ordered(&arena);

        let mut before: Vec<(u32, u32)> = doc
            .raw_start
            .iter()
            .zip(&doc.raw_end)
            .map(|(&s, &e)| (s, e))
            .collect();
        let mut after: Vec<(u32, u32)> = arena
            .raw_start()
            .iter()
            .zip(arena.raw_end())
            .map(|(&s, &e)| (s, e))
            .collect();
        before.sort_unstable();
        after.sort_unstable();
        assert_eq!(before, after, "the sort is a permutation");

        // Stability, which is what makes the sibling ordinal S17 derives
        // agree with document order: within one parent's block, the slots
        // are in the order the bytes put them.
        let first_child = arena.first_child();
        let raw_start = arena.raw_start();
        for i in 0..arena.len() {
            let block = first_child[i] as usize..first_child[i + 1] as usize;
            assert!(
                raw_start[block.clone()].windows(2).all(|w| w[0] < w[1]),
                "slot {i}: siblings out of document order"
            );
        }
        assert!(
            depths(&arena).iter().any(|&d| d >= 5),
            "expected real nesting"
        );
    }

    /// S24. The reservation is one slot per input byte and the shrink hands
    /// the rest back, so a finished walk holds capacity proportional to the
    /// node count rather than to the blob.
    #[test]
    fn the_over_reservation_is_returned() {
        let leaf = vec![0x08, 0x2A];
        let mut buf = Vec::new();
        for _ in 0..64 {
            buf.extend_from_slice(&leaf);
        }
        let nodes = walk_document_order(&buf).expect("walks");
        assert_eq!(nodes.len(), 64);
        assert_eq!(nodes.parent.capacity(), nodes.len());
    }

    // ── The cached probe verdict ────────────────────────────────────────────

    /// Spec 0097's cascade Step 1 — the answer the cached bit claims to
    /// already know. It calls the same `says_message` `render_len_field`
    /// does (spec 0266 S4), so the live verdict and the cached one cannot
    /// drift by the test transcribing the rule and then falling behind it.
    fn probe_says_message(payload: &[u8]) -> bool {
        let mut probe = super::super::sink::ProbeSink::default();
        let (next_pos, _) = render_message(payload, 0, None, None, false, &mut probe);
        probe.says_message(next_pos, payload)
    }

    /// Every node whose bytes are a well-framed LEN field must carry the
    /// verdict a real probe of its payload gives. That set is exactly the
    /// nodes `render_len_field` opened as nested messages: a malformed
    /// region runs to the end of its frame and so is framed correctly only
    /// by coincidence, and a group's bytes start with a `START_GROUP` tag.
    fn assert_cached_verdicts_are_real(buf: &[u8]) {
        let arena = build_arena(buf).expect("builds");
        let (raw_start, raw_end) = (arena.raw_start(), arena.raw_end());
        let mut checked = 0;
        for slot in 0..arena.len() {
            let node = &buf[raw_start[slot] as usize..raw_end[slot] as usize];
            let expected = well_framed_len_payload(node).map(probe_says_message);
            // A node that is not a well-framed LEN field never faced the
            // cascade — a group, a scalar, a packed run, a malformed region
            // — and must read `false`. Asserting that is the whole point:
            // it is where an implementation that re-derived the verdict
            // instead of being handed it went wrong, by answering a
            // question nobody asked.
            checked += usize::from(expected.is_some());
            assert_eq!(
                arena.probes_as_message(slot),
                expected.unwrap_or(false),
                "slot {slot} at {}..{}",
                raw_start[slot],
                raw_end[slot]
            );
        }
        assert!(checked > 0, "the fixture contains no LEN node to check");
    }

    /// A node's payload, if its bytes are exactly one length-delimited
    /// field — which is what `render_len_field` opened a nested message on.
    fn well_framed_len_payload(node: &[u8]) -> Option<&[u8]> {
        let tag = parse_wiretag(node, 0);
        if tag.wtype != Some(WT_LEN) {
            return None;
        }
        let len = parse_varint(node, tag.next_pos);
        let length = len.varint? as usize;
        (len.next_pos + length == node.len()).then(|| &node[len.next_pos..])
    }

    /// The bit the arena's *shape* cannot carry: these five bytes get
    /// children either way (see `an_unknown_payload_is_descended_into_
    /// regardless`), and only the verdict distinguishes them from a real
    /// message.
    #[test]
    fn a_payload_the_cascade_declines_is_recorded_as_declined() {
        let arena = build_arena(&len_field(1, b"hello")).expect("builds");
        assert!(!arena.probes_as_message(0));
        assert!(arena.len() > 1, "and it was descended into anyway");

        let inner = len_field(2, &[0x18, 0x2A]);
        let arena = build_arena(&len_field(1, &inner)).expect("builds");
        assert!(arena.probes_as_message(0));
    }

    /// `ProbeSink` is shallow for LEN (`treat_len_as_opaque`), so a defect
    /// buried in a nested message is invisible to the probe of the node
    /// containing it — that node validated a length prefix and stopped.
    #[test]
    fn a_defect_inside_a_nested_message_does_not_reach_its_parent() {
        let broken = len_field(2, b"hello");
        let buf = len_field(1, &broken);
        let arena = build_arena(&buf).expect("builds");
        assert!(arena.probes_as_message(0), "the outer sees a framed field");
        assert!(!arena.probes_as_message(1), "the inner sees the defect");
        assert_cached_verdicts_are_real(&buf);
    }

    /// A group has no length prefix, so the probe of the node containing it
    /// can only find its end by parsing through it, and takes on whatever
    /// it meets on the way — including the group never closing at all
    /// (spec 0110 Open Issue #1). The group's *own* slot still reads
    /// `false`: no cascade ever judged it.
    #[test]
    fn a_group_that_never_closes_condemns_the_node_containing_it() {
        // START_GROUP(1), varint field 2 = 42, and no END_GROUP.
        let buf = len_field(1, &[0x0B, 0x10, 0x2A]);
        let arena = build_arena(&buf).expect("builds");
        assert!(!arena.probes_as_message(0));

        // The same group, closed: nothing to charge to anyone.
        let buf = len_field(1, &[0x0B, 0x10, 0x2A, 0x0C]);
        let arena = build_arena(&buf).expect("builds");
        assert!(arena.probes_as_message(0));
    }

    /// The verdict survives the sort into level order, and agrees with a
    /// real probe on every node of a real blob.
    #[test]
    fn a_real_blob_carries_the_verdict_a_real_probe_gives() {
        assert_cached_verdicts_are_real(include_bytes!("../../../fixtures/descriptor.pb"));
        if let Ok(p) = std::env::var("PROBE_BITS_CORPUS") {
            assert_cached_verdicts_are_real(&std::fs::read(p).expect("read"));
        }
    }
}
