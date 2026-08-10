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

/// What a step folds before it unfolds (spec 0271 S9).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Fold {
    /// The default: the step starts from an unfolded document.
    #[default]
    None,
    /// Every node that has children.
    All,
    /// Exactly these.
    These(Vec<Position>),
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
    pub fold: Fold,
    pub unfold: Vec<Position>,
    pub wire: Option<Wire>,
    /// Command-line text to prefill, without the leading `:`
    /// (spec 0271 S11). Never executed by the script.
    pub prefill: Option<String>,
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
    #[serde(default)]
    fold: Option<Scalars>,
    #[serde(default)]
    unfold: Option<Scalars>,
    #[serde(default, rename = "wire-line")]
    wire_line: Option<String>,
    #[serde(default, rename = "wire-lines")]
    wire_lines: Option<RawRange>,
    #[serde(default, rename = "wire-node")]
    wire_node: Option<String>,
    #[serde(default, rename = "override")]
    r#override: Option<String>,
}

/// One scalar or a list of them — the shape `fold:` and `unfold:` share.
#[derive(Deserialize)]
#[serde(untagged)]
enum Scalars {
    One(String),
    Many(Vec<String>),
}

impl Scalars {
    fn into_vec(self) -> Vec<String> {
        match self {
            Scalars::One(s) => vec![s],
            Scalars::Many(v) => v,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRange {
    from: String,
    to: String,
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
                    "at most one of `wire-line`, `wire-lines`, `wire-node` per step".to_string(),
                )
            }
        };
        let fold = match self.fold {
            None => Fold::None,
            Some(Scalars::One(word)) if word == "all" => Fold::All,
            Some(Scalars::One(word)) if word == "none" => Fold::None,
            Some(other) => Fold::These(
                other
                    .into_vec()
                    .iter()
                    .map(|s| Position::parse(s))
                    .collect(),
            ),
        };
        Ok(Step {
            text: self.text,
            node: self.node.as_deref().map(Position::parse),
            fold,
            unfold: self
                .unfold
                .map(Scalars::into_vec)
                .unwrap_or_default()
                .iter()
                .map(|s| Position::parse(s))
                .collect(),
            wire,
            prefill: self.r#override,
        })
    }
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
               fold: all\n  \
               unfold: [/4, google.protobuf.Any]\n  \
               wire-lines:\n    \
                 from: /4/2/1\n    \
                 to: /4/2/3\n  \
               override: override /7 --as google.protobuf.Any\n",
        )
        .expect("must parse");
        let step = &s.steps[0];
        assert_eq!(step.text, "hello");
        assert_eq!(step.node, Some(Position::Path("/4/2".into())));
        assert_eq!(step.fold, Fold::All);
        assert_eq!(
            step.unfold,
            vec![
                Position::Path("/4".into()),
                Position::Search("google.protobuf.Any".into())
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

    #[test]
    fn a_single_scalar_fold_is_a_one_element_list() {
        let s = parse("steps:\n- text: x\n  fold: /4\n").expect("must parse");
        assert_eq!(
            s.steps[0].fold,
            Fold::These(vec![Position::Path("/4".into())])
        );
    }

    #[test]
    fn an_unknown_key_is_a_load_error() {
        let err = parse("steps:\n- text: x\n  nodes: /4\n").expect_err("must fail");
        assert!(err.contains("nodes"), "error must name the key: {err}");
    }

    #[test]
    fn two_wire_directives_are_a_load_error() {
        let err =
            parse("steps:\n- text: x\n  wire-line: /1\n  wire-node: /2\n").expect_err("must fail");
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
