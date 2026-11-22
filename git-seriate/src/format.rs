// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The commit block (spec 0452 S4): a subject, a line `---`, and a YAML
//! block, `format: 1`.
//!
//! Written by hand, in a fixed key order and with every string quoted, so
//! that one snapshot always gives one message, byte for byte. Read with
//! `serde_yaml`.

use std::fmt::Write as _;

/// The block's format version this crate writes and reads.
pub const FORMAT: u64 = 1;

/// Where a snapshot comes from (spec 0450 S3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub identity: String,
    pub build_time: i64,
    pub order_hint: Option<u64>,
    pub group: Option<String>,
    /// Shown, never interpreted. Kept in the order given.
    pub metadata: Vec<(String, String)>,
}

/// A candidate refused, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub name: String,
    pub reason: String,
}

/// A snapshot commit's block, or a record-only one's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub source: Source,
    /// The tree's paths, sorted; `None` for a record-only commit.
    pub contents: Option<Vec<String>>,
    pub rejected: Vec<Rejection>,
    /// The extractor's, shown, never interpreted.
    pub record: Vec<(String, String)>,
    /// History commits only: each composed rule set's version.
    pub canon_rules: Vec<(String, u32)>,
    /// History commits only: `compact` removed items from it.
    pub reduced: bool,
}

/// Why a message could not be read as a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// `format` is above [`FORMAT`]: a newer tool wrote it.
    Newer(u64),
    /// A block of this format, but not a well-formed one.
    Malformed(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Newer(v) => write!(
                f,
                "written in format {v}; this git-seriate reads format {FORMAT}: upgrade it"
            ),
            ReadError::Malformed(m) => write!(f, "malformed block: {m}"),
        }
    }
}

/// A YAML double-quoted scalar: JSON's escapes, which YAML accepts.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                write!(out, "\\u{:04x}", c as u32).unwrap();
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Block {
    /// The commit message: a subject, a blank line, `---`, the block.
    pub fn message(&self) -> String {
        let kind = if self.contents.is_some() {
            "Snapshot"
        } else {
            "Record"
        };
        let mut m = format!("{kind} {}\n\n---\n", self.source.identity);
        writeln!(m, "format: {FORMAT}").unwrap();
        let s = &self.source;
        m.push_str("source:\n");
        writeln!(m, "  identity: {}", quote(&s.identity)).unwrap();
        writeln!(m, "  build_time: {}", s.build_time).unwrap();
        if let Some(h) = s.order_hint {
            writeln!(m, "  order_hint: {h}").unwrap();
        }
        if let Some(g) = &s.group {
            writeln!(m, "  group: {}", quote(g)).unwrap();
        }
        if !s.metadata.is_empty() {
            m.push_str("  metadata:\n");
            for (k, v) in &s.metadata {
                writeln!(m, "    {}: {}", quote(k), quote(v)).unwrap();
            }
        }
        match &self.contents {
            None => m.push_str("kind: record-only\n"),
            Some(paths) => {
                writeln!(m, "items: {}", paths.len()).unwrap();
                if paths.is_empty() {
                    m.push_str("contents: {}\n");
                } else {
                    m.push_str("contents:\n");
                    write_tree(&mut m, paths);
                }
            }
        }
        if !self.rejected.is_empty() {
            m.push_str("rejected:\n");
            for r in &self.rejected {
                writeln!(
                    m,
                    "  - {{name: {}, reason: {}}}",
                    quote(&r.name),
                    quote(&r.reason)
                )
                .unwrap();
            }
        }
        if !self.record.is_empty() {
            m.push_str("record:\n");
            for (k, v) in &self.record {
                writeln!(m, "  {}: {}", quote(k), quote(v)).unwrap();
            }
        }
        if !self.canon_rules.is_empty() {
            m.push_str("canon_rules:\n");
            for (k, v) in &self.canon_rules {
                writeln!(m, "  {}: {v}", quote(k)).unwrap();
            }
        }
        if self.reduced {
            m.push_str("reduced: true\n");
        }
        m
    }

    /// Read a commit message. `Ok(None)` when it has no block of this
    /// family: no `---` line, or a block without `format` (an older format,
    /// for the legacy reader).
    pub fn read(message: &str) -> Result<Option<Block>, ReadError> {
        let Some(at) = message.find("\n---\n") else {
            return Ok(None);
        };
        let yaml = &message[at + 5..];
        let value: serde_yaml::Value =
            serde_yaml::from_str(yaml).map_err(|e| ReadError::Malformed(e.to_string()))?;
        let Some(map) = value.as_mapping() else {
            return Ok(None);
        };
        let Some(format) = map.get("format") else {
            return Ok(None);
        };
        let format = format
            .as_u64()
            .ok_or_else(|| ReadError::Malformed("format is not a number".into()))?;
        if format > FORMAT {
            return Err(ReadError::Newer(format));
        }
        let bad = |what: &str| ReadError::Malformed(what.to_string());
        let source = map.get("source").ok_or_else(|| bad("no source"))?;
        let text = |v: &serde_yaml::Value, what: &str| {
            v.as_str().map(str::to_string).ok_or_else(|| bad(what))
        };
        let pairs = |v: Option<&serde_yaml::Value>,
                     what: &str|
         -> Result<Vec<(String, String)>, ReadError> {
            let Some(v) = v else { return Ok(Vec::new()) };
            let m = v.as_mapping().ok_or_else(|| bad(what))?;
            m.iter()
                .map(|(k, v)| Ok((text(k, what)?, text(v, what)?)))
                .collect()
        };
        let source = Source {
            identity: text(
                source.get("identity").ok_or_else(|| bad("no identity"))?,
                "identity",
            )?,
            build_time: source
                .get("build_time")
                .and_then(serde_yaml::Value::as_i64)
                .ok_or_else(|| bad("no build_time"))?,
            order_hint: match source.get("order_hint") {
                None => None,
                Some(v) => Some(v.as_u64().ok_or_else(|| bad("order_hint"))?),
            },
            group: match source.get("group") {
                None => None,
                Some(v) => Some(text(v, "group")?),
            },
            metadata: pairs(source.get("metadata"), "metadata")?,
        };
        let contents = if map.get("kind").and_then(serde_yaml::Value::as_str) == Some("record-only")
        {
            None
        } else {
            let mut paths = Vec::new();
            flatten(
                map.get("contents").ok_or_else(|| bad("no contents"))?,
                "",
                &mut paths,
            )
            .map_err(ReadError::Malformed)?;
            paths.sort();
            Some(paths)
        };
        let rejected = match map.get("rejected") {
            None => Vec::new(),
            Some(v) => v
                .as_sequence()
                .ok_or_else(|| bad("rejected"))?
                .iter()
                .map(|r| {
                    Ok(Rejection {
                        name: text(
                            r.get("name").ok_or_else(|| bad("rejected name"))?,
                            "rejected name",
                        )?,
                        reason: text(
                            r.get("reason").ok_or_else(|| bad("rejected reason"))?,
                            "rejected reason",
                        )?,
                    })
                })
                .collect::<Result<_, ReadError>>()?,
        };
        let canon_rules = match map.get("canon_rules") {
            None => Vec::new(),
            Some(v) => v
                .as_mapping()
                .ok_or_else(|| bad("canon_rules"))?
                .iter()
                .map(|(k, v)| {
                    let version = v.as_u64().and_then(|n| u32::try_from(n).ok());
                    Ok((
                        text(k, "canon_rules")?,
                        version.ok_or_else(|| bad("canon_rules"))?,
                    ))
                })
                .collect::<Result<_, ReadError>>()?,
        };
        Ok(Some(Block {
            source,
            contents,
            rejected,
            record: pairs(map.get("record"), "record")?,
            canon_rules,
            reduced: map.get("reduced").and_then(serde_yaml::Value::as_bool) == Some(true),
        }))
    }
}

/// Sorted paths as nested maps, leaves `null`, two spaces per level.
fn write_tree(out: &mut String, paths: &[String]) {
    let mut open: Vec<&str> = Vec::new();
    for path in paths {
        let parts: Vec<&str> = path.split('/').collect();
        let (dirs, leaf) = parts.split_at(parts.len() - 1);
        let common = open.iter().zip(dirs).take_while(|(a, b)| a == b).count();
        open.truncate(common);
        for dir in &dirs[common..] {
            writeln!(out, "{}{}:", "  ".repeat(open.len() + 1), quote(dir)).unwrap();
            open.push(dir);
        }
        writeln!(
            out,
            "{}{}: null",
            "  ".repeat(open.len() + 1),
            quote(leaf[0])
        )
        .unwrap();
    }
}

fn flatten(v: &serde_yaml::Value, prefix: &str, out: &mut Vec<String>) -> Result<(), String> {
    let map = v.as_mapping().ok_or("contents is not a map")?;
    for (k, child) in map {
        let name = k.as_str().ok_or("a contents key is not a string")?;
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        if child.is_null() {
            out.push(path);
        } else {
            flatten(child, &path, out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> Block {
        Block {
            source: Source {
                identity: "build0001/part0".into(),
                build_time: 1_700_000_000,
                order_hint: Some(0),
                group: Some("build0001".into()),
                metadata: vec![("origin".into(), "a \"quoted\"\nvalue".into())],
            },
            contents: Some(vec![
                "a.txt".into(),
                "d/e/f.txt".into(),
                "d/g.txt".into(),
                "z".into(),
            ]),
            rejected: vec![Rejection {
                name: "../x".into(),
                reason: "dotdot".into(),
            }],
            record: vec![("storage".into(), "extent".into())],
            canon_rules: vec![("generic".into(), 1)],
            reduced: true,
        }
    }

    #[test]
    fn a_block_reads_back_as_written() {
        let b = block();
        assert_eq!(Block::read(&b.message()).unwrap(), Some(b));
        let record_only = Block {
            contents: None,
            ..block()
        };
        assert_eq!(
            Block::read(&record_only.message()).unwrap(),
            Some(record_only)
        );
        let empty = Block {
            contents: Some(Vec::new()),
            rejected: Vec::new(),
            record: Vec::new(),
            canon_rules: Vec::new(),
            reduced: false,
            ..block()
        };
        assert_eq!(Block::read(&empty.message()).unwrap(), Some(empty));
    }

    #[test]
    fn keys_come_in_a_fixed_order() {
        let m = block().message();
        let at = |k: &str| m.find(k).unwrap_or_else(|| panic!("{k} missing"));
        let order = [
            "format:",
            "source:",
            "identity:",
            "build_time:",
            "order_hint:",
            "group:",
            "metadata:",
            "items:",
            "contents:",
            "rejected:",
            "record:",
            "canon_rules:",
            "reduced:",
        ];
        assert!(order.windows(2).all(|w| at(w[0]) < at(w[1])), "{m}");
        assert!(m.starts_with("Snapshot build0001/part0\n\n---\nformat: 1\n"));
    }

    #[test]
    fn other_messages_are_not_blocks() {
        assert_eq!(Block::read("docs: notes"), Ok(None));
        assert_eq!(
            Block::read("x\n---\npackage_name: p\n"),
            Ok(None),
            "an older format"
        );
        let newer = block().message().replace("format: 1", "format: 2");
        assert_eq!(Block::read(&newer), Err(ReadError::Newer(2)));
    }
}
