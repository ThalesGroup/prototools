// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Load and merge per-file scoring-graph YAMLs (spec 0045 format).

use crate::hash::HashMap;

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
    /// In the document's order.
    messages: serde_yaml::Mapping,
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
    /// Declared packed (spec 0371 S2/S4); absent means false.
    #[serde(default)]
    packed: bool,
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
    /// The field is *declared* packed (spec 0371 S1). A reader accepts
    /// either encoding (spec 0175); the scorer charges the one that
    /// contradicts this.
    pub packed: bool,
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

// ── One file's scoring graph, as a structure (spec 0430 S6) ─────────────────

/// One file's scoring graph: what a spec 0045 YAML document holds, as
/// `reproto --emit-scoring-yaml` writes it and protoback builds it. The
/// merge takes these, whichever produced them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScoringFile {
    /// The run was asked for extension ranges (spec 0238 S2).
    pub extension_ranges: bool,
    /// The file's root FQDNs.
    pub entries: Vec<String>,
    /// Its messages, in the document's order.
    pub messages: Vec<(String, ScoringMessage)>,
}

/// One message of a scoring graph.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoringMessage {
    pub kind: NodeKind,
    pub fields: Vec<FieldSpec>,
    /// Canonical extension ranges, inclusive (spec 0238 S3); empty when
    /// the message declares none.
    pub ext_ranges: Vec<(u32, u32)>,
}

/// One field of a scoring-graph message, as the document spells it.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSpec {
    pub number: u32,
    pub ty: FieldType,
    pub child: Option<String>,
    pub range: Option<(i32, i32)>,
    pub label: FieldLabel,
    pub packed: bool,
}

/// A field's `type` in a scoring-graph document. Several spellings reach
/// one [`ScoringKind`]; the document keeps the one it was written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Int32,
    Uint32,
    Sint32,
    Int64,
    Uint64,
    Sint64,
    Bool,
    Enum,
    String,
    Bytes,
    Message,
    Group,
    Float,
    Double,
}

impl FieldType {
    pub fn parse(s: &str) -> Option<FieldType> {
        Some(match s {
            "int32" => FieldType::Int32,
            "uint32" => FieldType::Uint32,
            "sint32" => FieldType::Sint32,
            "int64" => FieldType::Int64,
            "uint64" => FieldType::Uint64,
            "sint64" => FieldType::Sint64,
            "bool" => FieldType::Bool,
            "enum" => FieldType::Enum,
            "string" => FieldType::String,
            "bytes" => FieldType::Bytes,
            "message" => FieldType::Message,
            "group" => FieldType::Group,
            "float" => FieldType::Float,
            "double" => FieldType::Double,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FieldType::Int32 => "int32",
            FieldType::Uint32 => "uint32",
            FieldType::Sint32 => "sint32",
            FieldType::Int64 => "int64",
            FieldType::Uint64 => "uint64",
            FieldType::Sint64 => "sint64",
            FieldType::Bool => "bool",
            FieldType::Enum => "enum",
            FieldType::String => "string",
            FieldType::Bytes => "bytes",
            FieldType::Message => "message",
            FieldType::Group => "group",
            FieldType::Float => "float",
            FieldType::Double => "double",
        }
    }

    pub fn kind(self) -> ScoringKind {
        match self {
            FieldType::Int32 => ScoringKind::Int32,
            FieldType::Uint32 | FieldType::Sint32 => ScoringKind::Uint32,
            FieldType::Int64 | FieldType::Uint64 | FieldType::Sint64 => ScoringKind::Uint64,
            FieldType::Bool | FieldType::Enum => ScoringKind::Range,
            FieldType::String => ScoringKind::LenString,
            FieldType::Bytes => ScoringKind::LenBytes,
            FieldType::Message | FieldType::Group => ScoringKind::Node,
            FieldType::Float => ScoringKind::I32,
            FieldType::Double => ScoringKind::I64,
        }
    }
}

impl ScoringFile {
    /// One YAML document. A malformed one is serde's error; a field whose
    /// type or label is unknown is named.
    pub fn from_yaml(text: &str) -> Result<ScoringFile, Box<dyn std::error::Error>> {
        Ok(Self::from_document(serde_yaml::from_str(text)?)?)
    }

    fn from_document(yaml: YamlFile) -> Result<ScoringFile, String> {
        let mut messages = Vec::with_capacity(yaml.messages.len());
        for (key, value) in yaml.messages {
            let fqdn: String = serde_yaml::from_value(key).map_err(|e| e.to_string())?;
            let msg: YamlMessage = serde_yaml::from_value(value).map_err(|e| e.to_string())?;
            let mut fields = Vec::with_capacity(msg.fields.len());
            for f in msg.fields {
                let ty = FieldType::parse(&f.kind)
                    .ok_or_else(|| format!("{fqdn}: unknown kind '{}'", f.kind))?;
                let label = match f.label.as_str() {
                    "required" => FieldLabel::Required,
                    "repeated" => FieldLabel::Repeated,
                    "" | "optional" => FieldLabel::Optional,
                    other => {
                        return Err(format!(
                            "{fqdn} field {}: unknown label '{other}'",
                            f.number
                        ))
                    }
                };
                fields.push(FieldSpec {
                    number: f.number,
                    ty,
                    child: f.child,
                    range: f.range,
                    label,
                    packed: f.packed,
                });
            }
            messages.push((
                fqdn,
                ScoringMessage {
                    kind: parse_node_kind(&msg.kind),
                    fields,
                    ext_ranges: msg.ext_ranges,
                },
            ));
        }
        Ok(ScoringFile {
            extension_ranges: yaml.extension_ranges,
            entries: yaml.entries,
            messages,
        })
    }
}

// ── Loading ───────────────────────────────────────────────────────────────────

/// One scoring-graph YAML document as a [`ScoringFile`]: for the stage
/// timing (`build_scoring_graph::timing`), which parses outside the clock
/// what protoback hands over as structures.
#[cfg(test)]
pub(crate) fn parse_file(text: &str) -> Result<ScoringFile, Box<dyn std::error::Error>> {
    Ok(ScoringFile::from_document(serde_yaml::from_str(text)?)?)
}

/// Load and merge from in-memory YAML strings (no filesystem access).
pub fn merge_from_strings(scoring_graphs: &[String]) -> Result<Merged, Box<dyn std::error::Error>> {
    let mut files = Vec::with_capacity(scoring_graphs.len());
    for (i, text) in scoring_graphs.iter().enumerate() {
        let yaml: YamlFile =
            serde_yaml::from_str(text).map_err(|e| format!("scoring_graph[{i}]: {e}"))?;
        files.push(ScoringFile::from_document(yaml)?);
    }
    merge_files(&files)
}

/// Merge scoring graphs, given as structures (spec 0430 S6): the first
/// definition of an FQDN is kept, a conflicting later one is warned
/// about; the roots are every file's entries, in order, once.
pub fn merge_files(files: &[ScoringFile]) -> Result<Merged, Box<dyn std::error::Error>> {
    let mut states: HashMap<String, Vec<ScoringField>> = HashMap::default();
    let mut node_kinds: HashMap<String, NodeKind> = HashMap::default();
    let mut roots: Vec<String> = Vec::new();
    let mut roots_seen: crate::hash::HashSet<String> = crate::hash::HashSet::default();
    let mut ext_ranges: HashMap<String, Vec<(u32, u32)>> = HashMap::default();
    let mut has_extension_ranges = true;

    for (i, file) in files.iter().enumerate() {
        has_extension_ranges &= file.extension_ranges;
        for fqdn in &file.entries {
            if roots_seen.insert(fqdn.clone()) {
                roots.push(fqdn.clone());
            }
        }
        for (fqdn, msg) in &file.messages {
            node_kinds.entry(fqdn.clone()).or_insert(msg.kind);
            let fields = scoring_fields(fqdn, &msg.fields)?;
            let msg_ext_ranges = check_ext_ranges(fqdn, msg.ext_ranges.clone())?;
            match states.entry(fqdn.clone()) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(fields);
                    if !msg_ext_ranges.is_empty() {
                        ext_ranges.insert(fqdn.clone(), msg_ext_ranges);
                    }
                }
                std::collections::hash_map::Entry::Occupied(existing) => {
                    if definitions_conflict(
                        existing.get(),
                        ext_ranges.get(fqdn).map(Vec::as_slice).unwrap_or(&[]),
                        &fields,
                        &msg_ext_ranges,
                    ) {
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
        has_extension_ranges: has_extension_ranges && !files.is_empty(),
    })
}

/// Whether a second definition of one FQDN disagrees with the first
/// (spec 0238 S4).
///
/// Extensibility is part of what makes two definitions *the same* definition.
/// Comparing only the fields would keep the first silently and drop a genuine
/// disagreement about which unknown field numbers are legal there.
///
/// A closed message and one declaring no ranges are the same message, so the
/// caller passes `&[]` for both — the "absent key" and "empty vector" spellings
/// of closedness must not read as a conflict.
fn definitions_conflict(
    first_fields: &[ScoringField],
    first_ranges: &[(u32, u32)],
    second_fields: &[ScoringField],
    second_ranges: &[(u32, u32)],
) -> bool {
    first_fields != second_fields || first_ranges != second_ranges
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

fn scoring_fields(
    fqdn: &str,
    specs: &[FieldSpec],
) -> Result<Vec<ScoringField>, Box<dyn std::error::Error>> {
    let mut fields = Vec::with_capacity(specs.len());
    for f in specs {
        let kind = f.ty.kind();
        if kind.is_node() && f.child.is_none() {
            return Err(format!(
                "{fqdn} field {}: kind {} requires a child FQDN",
                f.number,
                f.ty.as_str()
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
        fields.push(ScoringField {
            number: f.number,
            kind,
            child: f.child.clone(),
            range,
            label: f.label,
            packed: f.packed,
        });
    }
    // Ensure sorted by field number (spec says they already are, but be defensive).
    fields.sort_by_key(|f| f.number);
    Ok(fields)
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
            && self.packed == other.packed
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
    fn message_equality_distinguishes_ext_ranges() {
        // Spec 0238 S4. Two definitions of one FQDN that agree on every field
        // but disagree on extensibility are *not* the same definition: they
        // disagree about which unknown field numbers are legal, which is the
        // whole of what `Policy::Scan` reads.
        let m = merge_from_strings(&[one_file(true, "")]).unwrap();
        let f = m.states["A"].as_slice();

        let open: &[(u32, u32)] = &[(1000, 2999)];
        let closed: &[(u32, u32)] = &[];

        assert!(!definitions_conflict(f, closed, f, closed));
        assert!(!definitions_conflict(f, open, f, open));

        assert!(definitions_conflict(f, closed, f, open));
        assert!(definitions_conflict(f, open, f, closed));
        assert!(definitions_conflict(f, open, f, &[(1000, 3999)]));

        // The fields half of the predicate still bites.
        let mut other = m.states["A"].clone();
        other[0].number = 2;
        assert!(definitions_conflict(f, open, &other, open));
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
