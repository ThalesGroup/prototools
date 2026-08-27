// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Spec 0271: the `.script` file — a declarative walk through a blob.
//!
//! This module is the format and nothing else: parsing, classification
//! and validation, with no reference to `App`. Applying a step to a live
//! session is `tui::script_pane`.
//!
//! The one rule worth keeping in mind while reading: **a step declares a
//! view, it does not describe a change.** Nothing here is a delta, so
//! nothing here needs to know what the step before it said.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::tui::heat_cue::HeatCueMode;

/// One position-sensitive step directive (spec 0366).
///
/// The order of directives within a step is the order the script author
/// wrote the corresponding YAML keys.  `script_apply` executes them in
/// that order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Directive {
    Node(Position),
    Fold(Vec<FoldEntry>),
    Wire(Wire),
    SelectLine,
    SelectNode,
    Search(String),
}

/// The extension a script is found under (spec 0271 S1).
pub const SCRIPT_EXTENSION: &str = "script";

/// A place in the document, as written in a script (spec 0271 S3).
///
/// The classification is syntactic and total — if it looks like a
/// positional path it is one, and everything else is a search string —
/// so it is decided once, here, at load. That is what removes the need
/// for an escape syntax: `/name` is not a well-formed path, so it is a
/// search for the text `/name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Position {
    /// `/`, or `/` followed by 1-based child numbers: the notation
    /// `App::resolve_path` already accepts.
    Path(String),
    /// Anything else: matched from the top of the document, first hit
    /// wins.
    Search(String),
}

impl Position {
    pub fn parse(text: &str) -> Self {
        if looks_like_path(text) {
            Position::Path(text.to_string())
        } else {
            Position::Search(text.to_string())
        }
    }

    /// The scalar the script author wrote, for diagnostics and the
    /// transcript.
    pub fn as_written(&self) -> &str {
        match self {
            Position::Path(s) | Position::Search(s) => s,
        }
    }
}

/// `/`, or `/` followed by one or more non-empty runs of decimal digits
/// naming a 1-based child.
///
/// Deliberately strict at both ends: a trailing slash, an empty segment,
/// a leading zero-width segment or a non-digit anywhere all make this a
/// search string instead. A path that is *nearly* well-formed is far
/// more likely to be prose than a typo'd path, and treating it as a
/// search gives a visible, recoverable result either way.
fn looks_like_path(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('/') else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    rest.split('/')
        .all(|seg| !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()))
}

/// One predicate item in a step's `advance_when:` list (spec 0356).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    Visible {
        position: Position,
    },
    Folded {
        position: Position,
    },
    Wire {
        position: Position,
    },
    Type {
        position: Position,
        fqdn: String,
    },
    FieldName {
        position: Position,
        name: String,
    },
    FileExists {
        path: String,
    },
    Caret {
        position: Position,
    },
    Annotations {
        on: bool,
    },
    HeatCues {
        mode: HeatCueMode,
    },
    /// Conjunction of `inner` is negated.
    /// `inner` empty → negation of vacuous truth → always false.
    Not {
        inner: Vec<Predicate>,
    },
}

/// One entry in a step's `fold:` list (spec 0359).
///
/// Mirrors the interactive `0`–`9` and `Z` keys: `depth` is the value
/// passed to `set_fold_depth`, where `usize::MAX` means "fully open"
/// (the `Z` key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldEntry {
    pub position: Position,
    /// `0`–`9` or `usize::MAX` for `Z`.
    pub depth: usize,
}

/// A step's wire-byte declaration (spec 0271 S10). At most one per step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wire {
    /// The `w` gesture on one node's own line.
    Line(Position),
    /// The `w` gesture over a range of lines.
    Lines { from: Position, to: Position },
    /// The `W` gesture: a whole subtree.
    Node(Position),
}

/// One step: a piece of commentary and the view that goes with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub text: String,
    /// Spec 0366: position-sensitive directives in YAML key order.
    /// `script_apply` executes them in this order.
    pub directives: Vec<Directive>,
    /// Command-line text to prefill, without the leading `:`
    /// (spec 0271 S11). Never executed by the script.
    pub prefill: Option<String>,
    /// Spec 0356: predicates that auto-advance when all hold simultaneously.
    pub advance_when: Vec<Predicate>,
    /// Spec 0356 S8: set `self.annotations` on step entry (`None` = no change).
    pub set_annotations: Option<bool>,
    /// Spec 0356 S8: set `self.heat_cues` on step entry (`None` = no change).
    pub set_heat_cues: Option<HeatCueMode>,
}

impl Step {
    /// Whether any `Node` directive appears in this step.  Used by
    /// `script_focus` to decide whether to adjust the scroll position.
    pub fn has_node(&self) -> bool {
        self.directives
            .iter()
            .any(|d| matches!(d, Directive::Node(_)))
    }
}

#[derive(Debug, Clone)]
pub struct Script {
    pub title: Option<String>,
    /// Where it was loaded from — reported in the transcript, and in the
    /// error when a step misbehaves.
    pub path: PathBuf,
    pub steps: Vec<Step>,
}

impl Script {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read script {}: {e}", path.display()))?;
        Self::parse(&text, path.to_path_buf())
    }

    pub fn parse(text: &str, path: PathBuf) -> Result<Self, String> {
        let raw: RawScript = serde_norway::from_str(text)
            .map_err(|e| format!("cannot parse script {}: {e}", path.display()))?;
        if raw.steps.is_empty() {
            return Err(format!("script {} has no steps", path.display()));
        }
        let steps = raw
            .steps
            .into_iter()
            .enumerate()
            .map(|(i, s)| parse_step(s).map_err(|e| format!("step {}: {e}", i + 1)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("script {}: {e}", path.display()))?;
        Ok(Script {
            title: raw.title,
            path,
            steps,
        })
    }

    /// The script sitting beside `blob` under the same stem, if there is
    /// one (spec 0271 S1).
    pub fn beside(blob: &Path) -> Option<PathBuf> {
        let candidate = blob.with_extension(SCRIPT_EXTENSION);
        // `with_extension` on an extensionless path *adds* the
        // extension, so a blob literally named `foo.script` would
        // otherwise be offered itself.
        if candidate == blob {
            return None;
        }
        candidate.is_file().then_some(candidate)
    }
}

// ---------------------------------------------------------------------
// Deserialization
//
// Steps are deserialized via serde_norway::Value so that the insertion
// order of YAML keys is preserved (serde_norway::Mapping uses IndexMap).
// Position-sensitive keys become Directive variants appended in encounter
// order; position-insensitive keys (text, annotations, heat_cues,
// advance_when, command) are collected into named slots regardless of
// where they appear (spec 0366 S3).
// ---------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScript {
    #[serde(default)]
    title: Option<String>,
    steps: Vec<serde_norway::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRange {
    from: String,
    to: String,
}

/// Spec 0356 S6: one item in an `advance_when:` list.
///
/// `#[serde(untagged)]` means serde tries each arm in order and uses
/// the first whose key is present. `deny_unknown_fields` on each arm
/// rejects any key that is not the one the arm expects.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawPredicate {
    Visible { visible: String },
    Folded { folded: String },
    Wire { wire: String },
    Type { r#type: String },
    FieldName { field_name: String },
    FileExists { file_exists: String },
    Caret { caret: String },
    Annotations { annotations: bool },
    HeatCues { heat_cues: HeatCueMode },
    Not { not: Vec<RawPredicate> },
}

impl RawPredicate {
    fn into_predicate(self) -> Result<Predicate, String> {
        Ok(match self {
            RawPredicate::Visible { visible } => Predicate::Visible {
                position: Position::parse(&visible),
            },
            RawPredicate::Folded { folded } => Predicate::Folded {
                position: Position::parse(&folded),
            },
            RawPredicate::Wire { wire } => Predicate::Wire {
                position: Position::parse(&wire),
            },
            RawPredicate::Type { r#type } => {
                // Value must be "<position> <fqdn>" — at least two tokens.
                let raw = &r#type;
                let (pos_str, fqdn) = raw
                    .split_once(char::is_whitespace)
                    .map(|(p, rest)| (p, rest.trim_start().to_string()))
                    .filter(|(_, f)| !f.is_empty())
                    .ok_or_else(|| {
                        format!("advance_when type: {raw:?} must be \"<position> <fqdn>\"")
                    })?;
                Predicate::Type {
                    position: Position::parse(pos_str),
                    fqdn,
                }
            }
            RawPredicate::FieldName { field_name } => {
                // Value must be "<position> <name>" — at least two tokens.
                let raw = &field_name;
                let (pos_str, name) = raw
                    .split_once(char::is_whitespace)
                    .map(|(p, rest)| (p, rest.trim_start().to_string()))
                    .filter(|(_, n)| !n.is_empty())
                    .ok_or_else(|| {
                        format!("advance_when field_name: {raw:?} must be \"<position> <name>\"")
                    })?;
                Predicate::FieldName {
                    position: Position::parse(pos_str),
                    name,
                }
            }
            RawPredicate::FileExists { file_exists } => Predicate::FileExists { path: file_exists },
            RawPredicate::Caret { caret } => Predicate::Caret {
                position: Position::parse(&caret),
            },
            RawPredicate::Annotations { annotations } => Predicate::Annotations { on: annotations },
            RawPredicate::HeatCues { heat_cues } => Predicate::HeatCues { mode: heat_cues },
            RawPredicate::Not { not } => Predicate::Not {
                inner: not
                    .into_iter()
                    .map(RawPredicate::into_predicate)
                    .collect::<Result<Vec<_>, _>>()?,
            },
        })
    }
}

/// Parse one step from a `serde_norway::Value` (must be a mapping).
/// Keys are visited in insertion order, so YAML key order becomes
/// `directives` order (spec 0366 S4).
fn parse_step(value: serde_norway::Value) -> Result<Step, String> {
    let mapping = value
        .as_mapping()
        .ok_or("step must be a YAML mapping")?
        .clone();

    let mut text = String::new();
    let mut directives: Vec<Directive> = Vec::new();
    let mut prefill: Option<String> = None;
    let mut advance_when: Vec<Predicate> = Vec::new();
    let mut set_annotations: Option<bool> = None;
    let mut set_heat_cues: Option<HeatCueMode> = None;
    // Accumulate wire keys to enforce "at most one" in the order they appear.
    let mut wire_key: Option<(String, serde_norway::Value)> = None;

    for (k, v) in &mapping {
        let key = k
            .as_str()
            .ok_or_else(|| format!("step key must be a string, got {k:?}"))?;
        match key {
            "text" => {
                text = v.as_str().ok_or("text: must be a string")?.to_string();
            }
            "annotations" => {
                set_annotations = Some(v.as_bool().ok_or("annotations: must be a boolean")?);
            }
            "heat_cues" => {
                set_heat_cues = Some(
                    serde_norway::from_value(v.clone()).map_err(|e| format!("heat_cues: {e}"))?,
                );
            }
            "advance_when" => {
                let raw: Vec<RawPredicate> = serde_norway::from_value(v.clone())
                    .map_err(|e| format!("advance_when: {e}"))?;
                advance_when = raw
                    .into_iter()
                    .map(RawPredicate::into_predicate)
                    .collect::<Result<Vec<_>, _>>()?;
            }
            "command" => {
                prefill = Some(v.as_str().ok_or("command: must be a string")?.to_string());
            }
            "node" => {
                let pos = Position::parse(v.as_str().ok_or("node: must be a string")?);
                directives.push(Directive::Node(pos));
            }
            "fold" => {
                let raw: Vec<String> =
                    serde_norway::from_value(v.clone()).map_err(|e| format!("fold: {e}"))?;
                let entries = raw
                    .into_iter()
                    .map(|s| parse_fold_entry(&s))
                    .collect::<Result<Vec<_>, _>>()?;
                directives.push(Directive::Fold(entries));
            }
            "select_line" => {
                if v.as_bool().ok_or("select_line: must be a boolean")? {
                    directives.push(Directive::SelectLine);
                }
            }
            "select_node" => {
                if v.as_bool().ok_or("select_node: must be a boolean")? {
                    directives.push(Directive::SelectNode);
                }
            }
            "search" => {
                let pat = v.as_str().ok_or("search: must be a string")?.to_string();
                directives.push(Directive::Search(pat));
            }
            "wire_line" | "wire_lines" | "wire_node" => {
                if wire_key.is_some() {
                    return Err(
                        "at most one of `wire_line`, `wire_lines`, `wire_node` per step"
                            .to_string(),
                    );
                }
                wire_key = Some((key.to_string(), v.clone()));
            }
            other => {
                return Err(format!("unknown step key: `{other}`"));
            }
        }
    }

    // Build the Wire directive from whichever wire key was seen (if any),
    // and insert it at the position in `directives` corresponding to where
    // the wire key appeared in the YAML.  Since we deferred wire key parsing,
    // we need to find its insertion position.  Re-walk the mapping in order.
    if wire_key.is_some() {
        // Re-build directives list with Wire inserted at the right position.
        let mut new_directives: Vec<Directive> = Vec::with_capacity(directives.len() + 1);
        let mut wire_inserted = false;
        let mut non_wire_idx = 0; // index into the pre-built directives (no wire)

        for (k, v) in &mapping {
            let key = k.as_str().unwrap_or("");
            match key {
                "wire_line" => {
                    let pos = Position::parse(v.as_str().ok_or("wire_line: must be a string")?);
                    new_directives.push(Directive::Wire(Wire::Line(pos)));
                    wire_inserted = true;
                }
                "wire_lines" => {
                    let raw: RawRange = serde_norway::from_value(v.clone())
                        .map_err(|e| format!("wire_lines: {e}"))?;
                    new_directives.push(Directive::Wire(Wire::Lines {
                        from: Position::parse(&raw.from),
                        to: Position::parse(&raw.to),
                    }));
                    wire_inserted = true;
                }
                "wire_node" => {
                    let pos = Position::parse(v.as_str().ok_or("wire_node: must be a string")?);
                    new_directives.push(Directive::Wire(Wire::Node(pos)));
                    wire_inserted = true;
                }
                "text" | "annotations" | "heat_cues" | "advance_when" | "command" => {
                    // position-insensitive: skip
                }
                _ => {
                    // position-sensitive non-wire: take from pre-built list
                    if non_wire_idx < directives.len() {
                        new_directives.push(directives[non_wire_idx].clone());
                        non_wire_idx += 1;
                    }
                }
            }
        }
        let _ = wire_inserted; // always true when wire_key.is_some()
        directives = new_directives;
    }

    Ok(Step {
        text,
        directives,
        prefill,
        advance_when,
        set_annotations,
        set_heat_cues,
    })
}

/// Parse one `"<position> <depth>"` fold-entry string (spec 0359 S2).
fn parse_fold_entry(s: &str) -> Result<FoldEntry, String> {
    let Some(space) = s.rfind(' ') else {
        return Err(format!(
            "fold entry {s:?} must be \"<position> <depth>\" (0–9 or Z)"
        ));
    };
    let position = Position::parse(s[..space].trim_end());
    let depth_str = &s[space + 1..];
    let depth = match depth_str {
        "Z" => usize::MAX,
        d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => (d.as_bytes()[0] - b'0') as usize,
        _ => {
            return Err(format!(
                "fold entry {s:?}: depth must be 0–9 or Z, got {depth_str:?}"
            ))
        }
    };
    Ok(FoldEntry { position, depth })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Script, String> {
        Script::parse(text, PathBuf::from("t.script"))
    }

    #[test]
    fn a_well_formed_path_is_a_path_and_everything_else_is_a_search() {
        for good in ["/", "/1", "/4/2", "/10/300/1"] {
            assert_eq!(
                Position::parse(good),
                Position::Path(good.to_string()),
                "{good} must classify as a path"
            );
        }
        // Each of these begins with `/` but is not well-formed, which is
        // exactly the case that needs no escape syntax.
        for bad in ["/name", "/4/x", "/4/", "//2", "/-1", "name", "", "/ 1"] {
            assert_eq!(
                Position::parse(bad),
                Position::Search(bad.to_string()),
                "{bad} must classify as a search"
            );
        }
    }

    #[test]
    fn a_step_reads_its_directives() {
        let s = parse(
            "steps:\n\
             - text: hello\n  \
               node: /4/2\n  \
               fold: [\"/4/2 0\", \"/4 Z\"]\n  \
               wire_lines:\n    \
                 from: /4/2/1\n    \
                 to: /4/2/3\n  \
               command: override /7 --as google.protobuf.Any\n",
        )
        .expect("must parse");
        let step = &s.steps[0];
        assert_eq!(step.text, "hello");
        assert_eq!(
            step.directives,
            vec![
                Directive::Node(Position::Path("/4/2".into())),
                Directive::Fold(vec![
                    FoldEntry {
                        position: Position::Path("/4/2".into()),
                        depth: 0,
                    },
                    FoldEntry {
                        position: Position::Path("/4".into()),
                        depth: usize::MAX,
                    },
                ]),
                Directive::Wire(Wire::Lines {
                    from: Position::Path("/4/2/1".into()),
                    to: Position::Path("/4/2/3".into()),
                }),
            ]
        );
        assert_eq!(
            step.prefill.as_deref(),
            Some("override /7 --as google.protobuf.Any")
        );
    }

    /// Spec 0359 S2: each entry parsed into position + depth.
    #[test]
    fn fold_entries_parse_position_and_depth() {
        let s = parse("steps:\n- text: x\n  fold: [\"/3 2\", \"/ 0\", \"/1 Z\"]\n")
            .expect("must parse");
        assert_eq!(
            s.steps[0].directives,
            vec![Directive::Fold(vec![
                FoldEntry {
                    position: Position::Path("/3".into()),
                    depth: 2,
                },
                FoldEntry {
                    position: Position::Path("/".into()),
                    depth: 0,
                },
                FoldEntry {
                    position: Position::Path("/1".into()),
                    depth: usize::MAX,
                },
            ])]
        );
    }

    /// Spec 0366: YAML key order becomes directive execution order.
    #[test]
    fn directive_order_matches_yaml_key_order() {
        let s = parse(
            "steps:\n\
             - text: x\n  \
               search: foo\n  \
               node: /2\n  \
               select_line: true\n",
        )
        .expect("must parse");
        assert_eq!(
            s.steps[0].directives,
            vec![
                Directive::Search("foo".into()),
                Directive::Node(Position::Path("/2".into())),
                Directive::SelectLine,
            ]
        );
    }

    /// Spec 0359 S2: an unrecognised depth token is a load error.
    #[test]
    fn unknown_depth_is_a_load_error() {
        let err = parse("steps:\n- text: x\n  fold: [\"/3 X\"]\n").expect_err("must fail");
        assert!(err.contains("depth"), "error must mention depth: {err}");
    }

    /// Spec 0359 S1: a bare scalar (not a sequence) is a load error.
    #[test]
    fn bare_scalar_fold_is_a_load_error() {
        let err = parse("steps:\n- text: x\n  fold: \"/ 0\"\n").expect_err("must fail");
        assert!(!err.is_empty(), "{err}");
    }

    /// Spec 0359: the old `unfold:` key is gone — unknown field error.
    #[test]
    fn old_unfold_key_is_a_load_error() {
        let err = parse("steps:\n- text: x\n  unfold: [\"/3\"]\n").expect_err("must fail");
        assert!(err.contains("unfold"), "error must name the key: {err}");
    }

    #[test]
    fn an_unknown_key_is_a_load_error() {
        let err = parse("steps:\n- text: x\n  nodes: /4\n").expect_err("must fail");
        assert!(err.contains("nodes"), "error must name the key: {err}");
    }

    #[test]
    fn two_wire_directives_are_a_load_error() {
        let err =
            parse("steps:\n- text: x\n  wire_line: /1\n  wire_node: /2\n").expect_err("must fail");
        assert!(err.contains("at most one"), "{err}");
    }

    /// Spec 0271 S1 / test-plan item 1: the extension is *replaced*, not
    /// appended, and the file has to be there.
    #[test]
    fn a_script_is_found_beside_the_blob_under_the_same_stem() {
        let dir = std::env::temp_dir().join(format!("protolens-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let blob = dir.join("anomalies.pb");
        let script = dir.join("anomalies.script");

        assert_eq!(Script::beside(&blob), None, "nothing there yet");
        std::fs::write(&script, "steps:\n- text: x\n").expect("write script");
        assert_eq!(Script::beside(&blob), Some(script.clone()));
        // A blob that *is* a script must not be offered itself.
        assert_eq!(Script::beside(&script), None);

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_script_with_no_steps_is_a_load_error() {
        parse("steps: []\n").expect_err("must fail");
    }
}
