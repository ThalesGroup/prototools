// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Load and merge per-file scoring-graph YAMLs (spec 0045 format).

use std::collections::HashMap;

use serde::Deserialize;

// ── YAML schema ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct YamlFile {
    /// True iff reproto was asked for extension ranges (spec 0238 S2). Absent
    /// from a pre-0238 file and from any run without the flag, which is
    /// exactly the distinction the SCAN policy needs: without it, a message
    /// carrying no `ext_ranges` is *unrecorded*, not *closed*.
    #[serde(default)]
    extension_ranges: bool,
    entries: Vec<String>,
    messages: HashMap<String, YamlMessage>,
}

#[derive(Debug, Deserialize)]
struct YamlMessage {
    /// "LENDEL" (default) or "GROUP" — framing of this node (spec 0058).
    #[serde(default)]
    kind: String,
    fields: Vec<YamlField>,
    /// Canonical extension ranges, inclusive at both ends (spec 0238 S3):
    /// sorted, disjoint and non-adjacent. Absent means this message declares
    /// none, i.e. it is closed.
    #[serde(default)]
    ext_ranges: Vec<(u32, u32)>,
}

#[derive(Debug, Deserialize)]
struct YamlField {
    number: u32,
    #[serde(rename = "type")]
    kind: String,
    child: Option<String>,
    /// Range [min, max] for bool and enum fields.
    range: Option<(i32, i32)>,
    /// "optional" (default), "required", or "repeated"
    #[serde(default)]
    label: String,
}

// ── Public types ──────────────────────────────────────────────────────────────

/// One field in a scoring state.
#[derive(Debug, Clone)]
pub struct ScoringField {
    pub number: u32,
    pub kind: ScoringKind,
    /// FQDN of child message type; set iff kind is Node.
    pub child: Option<String>,
    /// Value range [min, max]; set iff kind is Range (bool or enum).
    pub range: Option<(i32, i32)>,
    /// Field cardinality: optional (default), required, or repeated.
    pub label: FieldLabel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldLabel {
    Optional,
    Required,
    Repeated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoringKind {
    /// `int32`: 2's-complement 32-bit; veto if wire value in invalid gap.
    Int32,
    /// `uint32`, `sint32`: unsigned/zigzag 32-bit; veto if wire value > 0xFFFF_FFFF.
    Uint32,
    /// `int64`, `uint64`, `sint64`: any 64-bit varint; no range veto.
    Uint64,
    /// `bool` or enum field: dynamic range `[min, max]` checked at walk time.
    Range,
    I64,
    LenString,
    LenBytes,
    /// Non-leaf node (length-delimited message or group).  The actual framing
    /// (LENDEL vs GROUP) is encoded in the source node's NodeKind (spec 0058).
    Node,
    I32,
}

impl ScoringKind {
    /// True for kinds whose edge points to a child non-leaf node.
    pub fn is_node(self) -> bool {
        matches!(self, ScoringKind::Node)
    }
}

/// Framing of a non-leaf node: how it is entered from the wire stream (spec 0058).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Length-delimited message (WT_LEN = 2).
    LenDel,
    /// Group (WT_START_GROUP = 3).
    Group,
}

/// Merged result of all loaded YAML files.
///
/// `Default` exists for the test fixtures that build one by hand: they care
/// about two or three fields and should not have to be edited every time the
/// scoring graph learns a new one (spec 0238 S11 records the same lesson for
/// `ScoringOpts`). `merge_from_strings` still fills every field explicitly.
#[derive(Default)]
pub struct Merged {
    /// FQDN → fields (sorted by field number).
    pub states: HashMap<String, Vec<ScoringField>>,
    /// FQDN → node framing (LENDEL or GROUP); defaults to LenDel if absent.
    pub node_kinds: HashMap<String, NodeKind>,
    /// Root entry FQDNs (from all `entries` lists, deduplicated).
    pub roots: Vec<String>,
    /// FQDN → canonical extension ranges (spec 0238 S3). A missing key means
    /// the message declares none; no entry ever holds an empty vector.
    pub ext_ranges: HashMap<String, Vec<(u32, u32)>>,
    /// True iff **every** merged file declared `extension_ranges: true`.
    ///
    /// Conjunction, not disjunction: one file from a run without the flag is
    /// enough to make "no `ext_ranges` key" ambiguous across the merged graph,
    /// and the safe reading of an ambiguous graph is that ranges were never
    /// recorded at all (spec 0238 S9).
    pub has_extension_ranges: bool,
}

// ── Loading ───────────────────────────────────────────────────────────────────

/// Load and merge from in-memory YAML strings (no filesystem access).
pub fn merge_from_strings(scoring_graphs: &[String]) -> Result<Merged, Box<dyn std::error::Error>> {
    let mut states: HashMap<String, Vec<ScoringField>> = HashMap::new();
    let mut node_kinds: HashMap<String, NodeKind> = HashMap::new();
    let mut roots: Vec<String> = Vec::new();
    let mut roots_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut ext_ranges: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
    let mut has_extension_ranges = true;

    for (i, text) in scoring_graphs.iter().enumerate() {
        let yaml: YamlFile =
            serde_yaml::from_str(text).map_err(|e| format!("scoring_graph[{i}]: {e}"))?;
        has_extension_ranges &= yaml.extension_ranges;
        for fqdn in yaml.entries {
            if roots_seen.insert(fqdn.clone()) {
                roots.push(fqdn);
            }
        }
        for (fqdn, msg) in yaml.messages {
            let nk = parse_node_kind(&msg.kind);
            node_kinds.entry(fqdn.clone()).or_insert(nk);
            let fields = parse_fields(&fqdn, msg.fields)?;
            let msg_ext_ranges = check_ext_ranges(&fqdn, msg.ext_ranges)?;
            match states.entry(fqdn.clone()) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(fields);
                    if !msg_ext_ranges.is_empty() {
                        ext_ranges.insert(fqdn.clone(), msg_ext_ranges);
                    }
                }
                std::collections::hash_map::Entry::Occupied(existing) => {
                    // Spec 0238 S4: extensibility is part of what makes two
                    // definitions of one FQDN the same definition. Comparing
                    // only the fields would silently keep the first and drop a
                    // genuine disagreement about which unknown numbers are legal.
                    let ranges_differ = ext_ranges.get(&fqdn).map(Vec::as_slice).unwrap_or(&[])
                        != msg_ext_ranges.as_slice();
                    if *existing.get() != fields || ranges_differ {
                        eprintln!(
                            "warning: conflicting definitions for '{fqdn}' in scoring_graph[{i}]; using first",
                        );
                    }
                }
            }
        }
    }

    Ok(Merged {
        states,
        node_kinds,
        roots,
        ext_ranges,
        // Vacuously true over zero files, which is the wrong reading: an empty
        // merge has recorded nothing.
        has_extension_ranges: has_extension_ranges && !scoring_graphs.is_empty(),
    })
}

/// Validate that `ranges` is in the canonical form reproto promises
/// (spec 0238 S3): inclusive, non-degenerate, ascending, disjoint and
/// non-adjacent.
///
/// Checked rather than assumed because the intern index downstream *is* the
/// equality test — two spellings of one set that reach this point uncanonical
/// would intern as two and split states that admit the same field numbers.
/// A hand-edited YAML is the realistic way that happens.
fn check_ext_ranges(
    fqdn: &str,
    ranges: Vec<(u32, u32)>,
) -> Result<Vec<(u32, u32)>, Box<dyn std::error::Error>> {
    for (i, &(start, end)) in ranges.iter().enumerate() {
        if start > end {
            return Err(format!("{fqdn}: ext_ranges [{start}, {end}] is empty").into());
        }
        if end > MAX_FIELD_NUMBER {
            return Err(format!(
                "{fqdn}: ext_ranges [{start}, {end}] exceeds the maximum field number \
                 {MAX_FIELD_NUMBER}"
            )
            .into());
        }
        if i > 0 {
            let prev_end = ranges[i - 1].1;
            if start <= prev_end + 1 {
                return Err(format!(
                    "{fqdn}: ext_ranges are not canonical — [{start}, {end}] is not \
                     strictly after [{}, {prev_end}] with a gap",
                    ranges[i - 1].0
                )
                .into());
            }
        }
    }
    Ok(ranges)
}

/// 2²⁹−1, the largest legal protobuf field number.
const MAX_FIELD_NUMBER: u32 = 536_870_911;

fn parse_fields(
    fqdn: &str,
    raw: Vec<YamlField>,
) -> Result<Vec<ScoringField>, Box<dyn std::error::Error>> {
    let mut fields = Vec::with_capacity(raw.len());
    for f in raw {
        let kind =
            parse_kind(&f.kind).ok_or_else(|| format!("{fqdn}: unknown kind '{}'", f.kind))?;
        if kind.is_node() && f.child.is_none() {
            return Err(format!(
                "{fqdn} field {}: kind {} requires a child FQDN",
                f.number, f.kind
            )
            .into());
        }
        let range = if kind == ScoringKind::Range {
            let (min, max) = f.range.ok_or_else(|| {
                format!("{fqdn} field {}: bool/enum kind requires range", f.number)
            })?;
            Some((min, max))
        } else {
            None
        };
        let label = match f.label.as_str() {
            "required" => FieldLabel::Required,
            "repeated" => FieldLabel::Repeated,
            "" | "optional" => FieldLabel::Optional,
            other => {
                return Err(format!("{fqdn} field {}: unknown label '{other}'", f.number).into())
            }
        };
        fields.push(ScoringField {
            number: f.number,
            kind,
            child: f.child,
            range,
            label,
        });
    }
    // Ensure sorted by field number (spec says they already are, but be defensive).
    fields.sort_by_key(|f| f.number);
    Ok(fields)
}

fn parse_kind(s: &str) -> Option<ScoringKind> {
    match s {
        "int32" => Some(ScoringKind::Int32),
        "uint32" | "sint32" => Some(ScoringKind::Uint32),
        "int64" | "uint64" | "sint64" => Some(ScoringKind::Uint64),
        "bool" | "enum" => Some(ScoringKind::Range),
        "string" => Some(ScoringKind::LenString),
        "bytes" => Some(ScoringKind::LenBytes),
        "message" | "group" => Some(ScoringKind::Node),
        "float" => Some(ScoringKind::I32),
        "double" => Some(ScoringKind::I64),
        _ => None,
    }
}

fn parse_node_kind(s: &str) -> NodeKind {
    match s {
        "GROUP" => NodeKind::Group,
        _ => NodeKind::LenDel, // "LENDEL" or absent → default
    }
}

// ── PartialEq for conflict detection ─────────────────────────────────────────

impl PartialEq for ScoringField {
    fn eq(&self, other: &Self) -> bool {
        self.number == other.number
            && self.kind == other.kind
            && self.child == other.child
            && self.range == other.range
            && self.label == other.label
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// One file declaring `A` with the given `ext_ranges` body (may be empty).
    fn one_file(marker: bool, ext_ranges: &str) -> String {
        let marker = if marker {
            "extension_ranges: true\n"
        } else {
            ""
        };
        format!(
            "{marker}entries: [A]\n\
             messages:\n  \
               A:\n    \
                 fields:\n      \
                   - number: 1\n        \
                     type: uint64\n{ext_ranges}"
        )
    }

    #[test]
    fn ext_ranges_are_parsed() {
        let m = merge_from_strings(&[one_file(true, "    ext_ranges: [[1000, 2999]]\n")]).unwrap();
        assert_eq!(m.ext_ranges["A"], vec![(1000, 2999)]);
        assert!(m.has_extension_ranges);
    }

    #[test]
    fn a_closed_message_gets_no_entry_rather_than_an_empty_one() {
        // Spec 0238 S5: closedness is the `NO_EXT_RANGES` sentinel downstream,
        // so an empty vector must never occupy a key here either.
        let m = merge_from_strings(&[one_file(true, "")]).unwrap();
        assert!(!m.ext_ranges.contains_key("A"));
    }

    #[test]
    fn the_marker_is_a_conjunction_over_the_merged_files() {
        // Spec 0238 S9: one file from a run without the flag makes "no
        // ext_ranges key" ambiguous across the whole merge.
        let both = merge_from_strings(&[one_file(true, ""), one_file(true, "")]).unwrap();
        assert!(both.has_extension_ranges);

        let mixed = merge_from_strings(&[one_file(true, ""), one_file(false, "")]).unwrap();
        assert!(!mixed.has_extension_ranges);
    }

    #[test]
    fn the_marker_is_false_over_zero_files() {
        let m = merge_from_strings(&[]).unwrap();
        assert!(!m.has_extension_ranges);
    }

    #[test]
    fn a_hand_edited_uncanonical_range_set_is_rejected() {
        // Not reachable from reproto, which canonicalizes (S3) — but the
        // intern index *is* the equality test downstream, so two spellings of
        // one set reaching it uncanonical would split equivalent states.
        for bad in [
            "    ext_ranges: [[2999, 1000]]\n",               // empty
            "    ext_ranges: [[1000, 536870912]]\n",          // past the maximum
            "    ext_ranges: [[2000, 2999], [1000, 1999]]\n", // out of order
            "    ext_ranges: [[1000, 1999], [2000, 2999]]\n", // adjacent, unmerged
        ] {
            assert!(
                merge_from_strings(&[one_file(true, bad)]).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }
}
