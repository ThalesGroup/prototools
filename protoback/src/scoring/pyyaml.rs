// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `--emit-scoring-yaml` (spec 0430 S3): a scoring graph as reproto's
//! `yaml.dump(doc, sort_keys=False, allow_unicode=True)` writes it.
//!
//! PyYAML's block style, for the values a scoring graph holds: mappings in
//! insertion order, a sequence under a key not indented further, a sequence
//! in a sequence opened on its item's line (`- - `), empty collections in
//! flow style (`[]`, `{}`). A key of 123 characters or more is not a simple
//! key: PyYAML writes it `? key`, its value after `: `.
//!
//! Strings are names (dotted protobuf identifiers) and fixed words; one is
//! plain unless YAML 1.1's resolver would read it back as a bool or a null,
//! then single-quoted, as PyYAML quotes it. A string outside that alphabet
//! is single-quoted too, which PyYAML may not do: none reaches here.

use prototext_graph::build_scoring_graph::{FieldLabel, NodeKind, ScoringFile};

/// A YAML node of a document.
pub enum Y {
    Map(Vec<(String, Y)>),
    Seq(Vec<Y>),
    Str(String),
    Int(i64),
    Bool(bool),
}

/// The YAML of one file's scoring graph (`_scoring_yaml_doc`).
pub fn dump(file: &ScoringFile) -> String {
    dump_map(&document(file))
}

/// `yaml.dump(doc, sort_keys=False, allow_unicode=True)` of a mapping.
pub fn dump_map(doc: &Y) -> String {
    let mut out = String::new();
    if let Y::Map(items) = doc {
        map(&mut out, items, 0, false);
    }
    out
}

fn document(file: &ScoringFile) -> Y {
    let mut doc = Vec::new();
    if file.extension_ranges {
        doc.push(("extension_ranges".to_string(), Y::Bool(true)));
    }
    doc.push((
        "entries".to_string(),
        Y::Seq(file.entries.iter().map(|e| Y::Str(e.clone())).collect()),
    ));
    let messages = file
        .messages
        .iter()
        .map(|(name, m)| {
            let mut body = vec![
                (
                    "kind".to_string(),
                    Y::Str(
                        match m.kind {
                            NodeKind::Group => "GROUP",
                            NodeKind::LenDel => "LENDEL",
                        }
                        .to_string(),
                    ),
                ),
                (
                    "fields".to_string(),
                    Y::Seq(
                        m.fields
                            .iter()
                            .map(|f| {
                                let mut e = vec![
                                    ("number".to_string(), Y::Int(i64::from(f.number))),
                                    ("type".to_string(), Y::Str(f.ty.as_str().to_string())),
                                ];
                                if let Some(child) = &f.child {
                                    e.push(("child".to_string(), Y::Str(child.clone())));
                                }
                                if let Some((lo, hi)) = f.range {
                                    e.push((
                                        "range".to_string(),
                                        Y::Seq(vec![Y::Int(i64::from(lo)), Y::Int(i64::from(hi))]),
                                    ));
                                }
                                match f.label {
                                    FieldLabel::Optional => {}
                                    FieldLabel::Required => e.push((
                                        "label".to_string(),
                                        Y::Str("required".to_string()),
                                    )),
                                    FieldLabel::Repeated => e.push((
                                        "label".to_string(),
                                        Y::Str("repeated".to_string()),
                                    )),
                                }
                                if f.packed {
                                    e.push(("packed".to_string(), Y::Bool(true)));
                                }
                                Y::Map(e)
                            })
                            .collect(),
                    ),
                ),
            ];
            if !m.ext_ranges.is_empty() {
                body.push((
                    "ext_ranges".to_string(),
                    Y::Seq(
                        m.ext_ranges
                            .iter()
                            .map(|&(s, e)| Y::Seq(vec![Y::Int(i64::from(s)), Y::Int(i64::from(e))]))
                            .collect(),
                    ),
                ));
            }
            (name.clone(), Y::Map(body))
        })
        .collect();
    doc.push(("messages".to_string(), Y::Map(messages)));
    Y::Map(doc)
}

/// YAML 1.1's implicit bools and nulls (PyYAML's resolver): a plain string
/// spelled so would read back as one.
const RESERVED: &[&str] = &[
    "yes", "Yes", "YES", "no", "No", "NO", "true", "True", "TRUE", "false", "False", "FALSE", "on",
    "On", "ON", "off", "Off", "OFF", "null", "Null", "NULL", "~", "",
];

fn scalar(s: &str) -> String {
    let mut chars = s.chars();
    let identifier = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if identifier && !RESERVED.contains(&s) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}

fn leaf(y: &Y) -> Option<String> {
    match y {
        Y::Str(s) => Some(scalar(s)),
        Y::Int(i) => Some(i.to_string()),
        Y::Bool(b) => Some(if *b { "true" } else { "false" }.to_string()),
        Y::Seq(v) if v.is_empty() => Some("[]".to_string()),
        Y::Map(v) if v.is_empty() => Some("{}".to_string()),
        _ => None,
    }
}

fn pad(out: &mut String, indent: usize) {
    out.extend(std::iter::repeat_n(' ', indent));
}

/// A block mapping at `indent`; `inline`: its first key goes on the line
/// already open (after `- ` or `: `).
fn map(out: &mut String, items: &[(String, Y)], indent: usize, inline: bool) {
    for (i, (key, value)) in items.iter().enumerate() {
        if i > 0 || !inline {
            pad(out, indent);
        }
        // PyYAML's `check_simple_key`: a simple key is under 128 characters,
        // counting the raw string and its tag, `!!str` (5) even implicit.
        let long = key.chars().count() + 5 >= 128;
        let key = scalar(key);
        if long {
            out.push_str("? ");
            out.push_str(&key);
            out.push('\n');
            pad(out, indent);
            out.push(':');
            node(out, value, indent);
        } else {
            out.push_str(&key);
            out.push(':');
            match (leaf(value), value) {
                (Some(text), _) => {
                    out.push(' ');
                    out.push_str(&text);
                    out.push('\n');
                }
                (None, Y::Map(m)) => {
                    out.push('\n');
                    map(out, m, indent + 2, false);
                }
                (None, Y::Seq(s)) => {
                    out.push('\n');
                    seq(out, s, indent, false);
                }
                (None, _) => unreachable!("scalars are leaves"),
            }
        }
    }
}

/// A node after an indicator (`- ` or `: `) at `indent`.
fn node(out: &mut String, y: &Y, indent: usize) {
    out.push(' ');
    match (leaf(y), y) {
        (Some(text), _) => {
            out.push_str(&text);
            out.push('\n');
        }
        (None, Y::Map(m)) => map(out, m, indent + 2, true),
        (None, Y::Seq(s)) => seq(out, s, indent + 2, true),
        (None, _) => unreachable!("scalars are leaves"),
    }
}

/// A block sequence at `indent`; `inline`: its first item goes on the line
/// already open.
fn seq(out: &mut String, items: &[Y], indent: usize, inline: bool) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 || !inline {
            pad(out, indent);
        }
        out.push('-');
        node(out, item, indent);
    }
}

#[cfg(test)]
mod tests {
    use prototext_graph::build_scoring_graph::{
        FieldLabel, FieldSpec, FieldType, NodeKind, ScoringFile, ScoringMessage,
    };

    mod cases {
        include!("pyyaml/cases.rs");
    }

    #[test]
    fn scalars_are_quoted_as_pyyaml_quotes_them() {
        for &(s, item, key) in cases::SCALARS {
            assert_eq!(super::scalar(s), item, "{s:?} as an item");
            let mut out = String::new();
            super::map(&mut out, &[(s.to_string(), super::Y::Int(1))], 0, false);
            assert_eq!(out, key, "{s:?} as a key");
        }
    }

    #[test]
    fn a_document_is_dumped_as_pyyaml_dumps_it() {
        let long = format!("a.{}", "B".repeat(130));
        let spec = |number, ty, child: Option<&str>, range, label, packed| FieldSpec {
            number,
            ty,
            child: child.map(str::to_string),
            range,
            label,
            packed,
        };
        let file = ScoringFile {
            extension_ranges: true,
            entries: vec!["a.B".into(), "yes".into(), long.clone()],
            messages: vec![
                (
                    "a.B".into(),
                    ScoringMessage {
                        kind: NodeKind::LenDel,
                        fields: vec![
                            spec(
                                1,
                                FieldType::Bool,
                                None,
                                Some((0, 1)),
                                FieldLabel::Repeated,
                                true,
                            ),
                            spec(
                                2,
                                FieldType::Message,
                                Some(&long),
                                None,
                                FieldLabel::Required,
                                false,
                            ),
                        ],
                        ext_ranges: vec![(1000, 2999), (5000, 536_870_911)],
                    },
                ),
                (
                    long.clone(),
                    ScoringMessage {
                        kind: NodeKind::Group,
                        fields: Vec::new(),
                        ext_ranges: Vec::new(),
                    },
                ),
                (
                    "True".into(),
                    ScoringMessage {
                        kind: NodeKind::LenDel,
                        fields: vec![spec(
                            3,
                            FieldType::Enum,
                            None,
                            Some((-1, 4)),
                            FieldLabel::Optional,
                            false,
                        )],
                        ext_ranges: Vec::new(),
                    },
                ),
            ],
        };
        assert_eq!(super::dump(&file), cases::DOCUMENT);
        let empty = ScoringFile::default();
        assert_eq!(super::dump(&empty), cases::EMPTY);
    }
}
