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
    pub node: Option<Position>,
    /// Spec 0359: ordered list of `(position, depth)` fold directives,
    /// applied after `script_reset_folds`. Empty means start fully unfolded.
    pub fold: Vec<FoldEntry>,
    pub wire: Option<Wire>,
    /// Command-line text to prefill, without the leading `:`
    /// (spec 0271 S11). Never executed by the script.
    pub prefill: Option<String>,
    /// Whether to select the caret's header line when the step is applied
    /// (spec 0357).
    pub select_line: bool,
    /// Regex to highlight via the search machinery when the step is applied
    /// (spec 0357).
    pub search: Option<String>,
    /// Spec 0356: predicates that auto-advance when all hold simultaneously.
    pub advance_when: Vec<Predicate>,
    /// Spec 0356 S8: set `self.annotations` on step entry (`None` = no change).
    pub set_annotations: Option<bool>,
    /// Spec 0356 S8: set `self.heat_cues` on step entry (`None` = no change).
    pub set_heat_cues: Option<HeatCueMode>,
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
            .map(|(i, s)| s.into_step().map_err(|e| format!("step {}: {e}", i + 1)))
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
// The raw shapes exist so that the public types above can be plain enums
// with no serde attributes on them, and so that "at most one wire key"
// is checked in one place rather than encoded as an untagged enum that
// would silently pick the first key that parsed.
// ---------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScript {
    #[serde(default)]
    title: Option<String>,
    steps: Vec<RawStep>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    #[serde(default)]
    text: String,
    #[serde(default)]
    node: Option<String>,
    /// Spec 0359: sequence of `"<position> <depth>"` strings.
    /// A bare scalar (not a sequence) is a load error via serde.
    #[serde(default)]
    fold: Option<Vec<String>>,
    #[serde(default)]
    wire_line: Option<String>,
    #[serde(default)]
    wire_lines: Option<RawRange>,
    #[serde(default)]
    wire_node: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default, rename = "select_line")]
    select_line: bool,
    #[serde(default)]
    search: Option<String>,
    #[serde(default, rename = "advance_when")]
    advance_when: Option<Vec<RawPredicate>>,
    #[serde(default)]
    annotations: Option<bool>,
    #[serde(default, rename = "heat_cues")]
    heat_cues: Option<HeatCueMode>,
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

impl RawStep {
    fn into_step(self) -> Result<Step, String> {
        let wire = match (self.wire_line, self.wire_lines, self.wire_node) {
            (None, None, None) => None,
            (Some(p), None, None) => Some(Wire::Line(Position::parse(&p))),
            (None, Some(r), None) => Some(Wire::Lines {
                from: Position::parse(&r.from),
                to: Position::parse(&r.to),
            }),
            (None, None, Some(p)) => Some(Wire::Node(Position::parse(&p))),
            _ => {
                return Err(
                    "at most one of `wire_line`, `wire_lines`, `wire_node` per step".to_string(),
                )
            }
        };
        let fold = self
            .fold
            .unwrap_or_default()
            .into_iter()
            .map(|s| parse_fold_entry(&s))
            .collect::<Result<Vec<_>, _>>()?;
        let advance_when = self
            .advance_when
            .unwrap_or_default()
            .into_iter()
            .map(RawPredicate::into_predicate)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Step {
            text: self.text,
            node: self.node.as_deref().map(Position::parse),
            fold,
            wire,
            prefill: self.command,
            select_line: self.select_line,
            search: self.search,
            advance_when,
            set_annotations: self.annotations,
            set_heat_cues: self.heat_cues,
        })
    }
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
        assert_eq!(step.node, Some(Position::Path("/4/2".into())));
        assert_eq!(
            step.fold,
            vec![
                FoldEntry {
                    position: Position::Path("/4/2".into()),
                    depth: 0,
                },
                FoldEntry {
                    position: Position::Path("/4".into()),
                    depth: usize::MAX,
                },
            ]
        );
        assert_eq!(
            step.wire,
            Some(Wire::Lines {
                from: Position::Path("/4/2/1".into()),
                to: Position::Path("/4/2/3".into()),
            })
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
            s.steps[0].fold,
            vec![
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
