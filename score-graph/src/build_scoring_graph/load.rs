// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Load and merge per-file scoring-graph YAMLs (spec 0045 format).

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

// ── YAML schema ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct YamlFile {
    entries: Vec<String>,
    messages: HashMap<String, YamlMessage>,
}

#[derive(Debug, Deserialize)]
struct YamlMessage {
    fields: Vec<YamlField>,
}

#[derive(Debug, Deserialize)]
struct YamlField {
    number: u32,
    kind: String,
    child: Option<String>,
    enum_min: Option<i32>,
    enum_max: Option<i32>,
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
    /// FQDN of child message type; set iff kind is LenMsg or Group.
    pub child: Option<String>,
    /// Enum value range [min, max]; set iff kind is Enum.
    pub enum_range: Option<(i32, i32)>,
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
    Varint,
    I64,
    LenString,
    LenBytes,
    LenMsg,
    LenPacked,
    Group,
    I32,
    Enum,
}

impl ScoringKind {
    /// Wire type used on the wire for this kind.
    pub fn wire_type(self) -> u32 {
        match self {
            ScoringKind::Varint | ScoringKind::Enum => 0,
            ScoringKind::I64 => 1,
            ScoringKind::LenString
            | ScoringKind::LenBytes
            | ScoringKind::LenMsg
            | ScoringKind::LenPacked => 2,
            ScoringKind::Group => 3,
            ScoringKind::I32 => 5,
        }
    }

    /// True for kinds whose edge points to a child message node (not a leaf).
    pub fn is_message(self) -> bool {
        matches!(self, ScoringKind::LenMsg | ScoringKind::Group)
    }
}

/// Merged result of all loaded YAML files.
pub struct Merged {
    /// FQDN → fields (sorted by field number).
    pub states: HashMap<String, Vec<ScoringField>>,
    /// Root entry FQDNs (from all `entries` lists, deduplicated).
    pub roots: Vec<String>,
}

// ── Loading ───────────────────────────────────────────────────────────────────

/// Load and merge from in-memory YAML strings (no filesystem access).
pub fn merge_from_strings(scoring_graphs: &[String]) -> Result<Merged, Box<dyn std::error::Error>> {
    let mut states: HashMap<String, Vec<ScoringField>> = HashMap::new();
    let mut roots: Vec<String> = Vec::new();
    let mut roots_seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (i, text) in scoring_graphs.iter().enumerate() {
        let yaml: YamlFile =
            serde_yaml::from_str(text).map_err(|e| format!("scoring_graph[{i}]: {e}"))?;
        for fqdn in yaml.entries {
            if roots_seen.insert(fqdn.clone()) {
                roots.push(fqdn);
            }
        }
        for (fqdn, msg) in yaml.messages {
            let fields = parse_fields(&fqdn, msg.fields)?;
            match states.entry(fqdn.clone()) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(fields);
                }
                std::collections::hash_map::Entry::Occupied(existing) => {
                    if *existing.get() != fields {
                        eprintln!(
                            "warning: conflicting definitions for '{fqdn}' in scoring_graph[{i}]; using first",
                        );
                    }
                }
            }
        }
    }

    Ok(Merged { states, roots })
}

pub fn load_and_merge(paths: &[std::path::PathBuf]) -> Result<Merged, Box<dyn std::error::Error>> {
    let mut states: HashMap<String, Vec<ScoringField>> = HashMap::new();
    let mut roots: Vec<String> = Vec::new();
    let mut roots_seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for path in paths {
        let yaml = load_file(path)?;

        // Collect root entries in order, deduplicated across files.
        for fqdn in yaml.entries {
            if roots_seen.insert(fqdn.clone()) {
                roots.push(fqdn);
            }
        }

        // Merge message states.
        for (fqdn, msg) in yaml.messages {
            let fields = parse_fields(&fqdn, msg.fields)?;
            match states.entry(fqdn.clone()) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(fields);
                }
                std::collections::hash_map::Entry::Occupied(existing) => {
                    if *existing.get() != fields {
                        eprintln!(
                            "warning: conflicting definitions for '{fqdn}' in {}; using first",
                            path.display()
                        );
                    }
                }
            }
        }
    }

    Ok(Merged { states, roots })
}

fn load_file(path: &Path) -> Result<YamlFile, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let yaml: YamlFile =
        serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(yaml)
}

fn parse_fields(
    fqdn: &str,
    raw: Vec<YamlField>,
) -> Result<Vec<ScoringField>, Box<dyn std::error::Error>> {
    let mut fields = Vec::with_capacity(raw.len());
    for f in raw {
        let kind =
            parse_kind(&f.kind).ok_or_else(|| format!("{fqdn}: unknown kind '{}'", f.kind))?;
        if kind.is_message() && f.child.is_none() {
            return Err(format!(
                "{fqdn} field {}: kind {} requires a child FQDN",
                f.number, f.kind
            )
            .into());
        }
        let enum_range = if kind == ScoringKind::Enum {
            let min = f
                .enum_min
                .ok_or_else(|| format!("{fqdn} field {}: ENUM kind requires enum_min", f.number))?;
            let max = f
                .enum_max
                .ok_or_else(|| format!("{fqdn} field {}: ENUM kind requires enum_max", f.number))?;
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
            enum_range,
            label,
        });
    }
    // Ensure sorted by field number (spec says they already are, but be defensive).
    fields.sort_by_key(|f| f.number);
    Ok(fields)
}

fn parse_kind(s: &str) -> Option<ScoringKind> {
    match s {
        "VARINT" => Some(ScoringKind::Varint),
        "I64" => Some(ScoringKind::I64),
        "LEN_STRING" => Some(ScoringKind::LenString),
        "LEN_BYTES" => Some(ScoringKind::LenBytes),
        "LEN_MSG" => Some(ScoringKind::LenMsg),
        "LEN_PACKED" => Some(ScoringKind::LenPacked),
        "GROUP" => Some(ScoringKind::Group),
        "I32" => Some(ScoringKind::I32),
        "ENUM" => Some(ScoringKind::Enum),
        _ => None,
    }
}

// ── PartialEq for conflict detection ─────────────────────────────────────────

impl PartialEq for ScoringField {
    fn eq(&self, other: &Self) -> bool {
        self.number == other.number
            && self.kind == other.kind
            && self.child == other.child
            && self.enum_range == other.enum_range
            && self.label == other.label
    }
}
