// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! reproto's line model (`text.py`): a rendered file is a block of lines,
//! each code, a comment, or an orphan (code commented out, because it names
//! what the output lacks), at an indentation level, flushed to text at the
//! end.

/// Spaces per indentation level.
pub const TAB_SIZE: usize = 2;
/// The width past which a method's `rpc` line is wrapped.
pub const MAX_LINE_LENGTH: usize = 79;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineType {
    Code,
    Comment,
    Orphan,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub text: String,
    pub level: usize,
    pub kind: LineType,
}

impl Line {
    pub fn code(text: impl Into<String>, level: usize) -> Line {
        Line {
            text: text.into(),
            level,
            kind: LineType::Code,
        }
    }

    pub fn comment(text: impl Into<String>, level: usize) -> Line {
        Line {
            text: text.into(),
            level,
            kind: LineType::Comment,
        }
    }

    pub fn orphan(text: impl Into<String>, level: usize) -> Line {
        Line {
            text: text.into(),
            level,
            kind: LineType::Orphan,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Block {
    pub lines: Vec<Line>,
}

impl Block {
    pub fn new() -> Block {
        Block::default()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn push(&mut self, line: Line) {
        self.lines.push(line);
    }

    pub fn extend(&mut self, other: Block) {
        self.lines.extend(other.lines);
    }

    pub fn insert(&mut self, index: usize, line: Line) {
        self.lines.insert(index, line);
    }

    /// Every line made an orphan (`Block.abandon`).
    pub fn abandon(&mut self) {
        for line in &mut self.lines {
            line.kind = LineType::Orphan;
        }
    }

    /// A blank line, unless the block is empty or ends with one
    /// (`append_div_maybe`).
    pub fn div_maybe(&mut self, level: usize) {
        if self.lines.last().is_some_and(|l| !l.text.is_empty()) {
            self.push(Line::code("", level));
        }
    }

    /// Text added to the last line (`postpend`).
    pub fn postpend(&mut self, s: &str) {
        if let Some(last) = self.lines.last_mut() {
            last.text.push_str(s);
        }
    }

    /// Trailing blank lines dropped, keeping at least `keep` lines.
    pub fn trim_end(&mut self, keep: usize) {
        while self.lines.len() > keep && self.lines.last().is_some_and(|l| l.text.is_empty()) {
            self.lines.pop();
        }
    }

    /// The text (`Block.flush`): a comment `//` after its indentation; an
    /// orphan `///` first, eating up to three of its indentation's spaces
    /// (a blank orphan line deep enough keeps trailing spaces, as reproto
    /// writes it); code after its indentation, a blank line bare.
    pub fn flush(&self, redact_comments: bool, redact_orphans: bool) -> String {
        let mut out = String::new();
        for line in &self.lines {
            let indent = line.level * TAB_SIZE;
            match line.kind {
                LineType::Comment => {
                    if !redact_comments {
                        out.push_str(&" ".repeat(indent));
                        out.push_str("//");
                        out.push_str(&line.text);
                        out.push('\n');
                    }
                }
                LineType::Orphan => {
                    if !redact_orphans {
                        out.push_str("///");
                        out.push_str(&" ".repeat(indent - indent.min(3)));
                        out.push_str(&line.text);
                        out.push('\n');
                    }
                }
                LineType::Code => {
                    if !line.text.is_empty() {
                        out.push_str(&" ".repeat(indent));
                        out.push_str(&line.text);
                    }
                    out.push('\n');
                }
            }
        }
        out
    }
}
