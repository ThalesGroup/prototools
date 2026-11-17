// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The output's SourceCodeInfo (`source_info.py`, spec 0141), synthesized
//! from the text: while a file renders, each element's lines are marked
//! with its source path; once the text is complete, each marker becomes a
//! location, its span in the lines `flush` writes (spec 0429 S10).

use crate::hash::HashMap;
use std::cell::{Cell, RefCell};

use super::text::{Block, LineType, TAB_SIZE};

/// A marker: an element's path, and the ids of its first and closing lines.
struct Marker {
    path: Vec<i32>,
    open: u32,
    close: u32,
}

/// A location: its path and span (`[line, start, end]` on one line,
/// `[start line, start, end line, end]` over several).
pub struct Location {
    pub path: Vec<i32>,
    pub span: Vec<i32>,
}

/// The markers of the file rendering (`ctx.out_sci`).
#[derive(Default)]
pub struct Sci {
    markers: RefCell<Vec<Marker>>,
    next: Cell<u32>,
}

impl Sci {
    fn id(&self) -> u32 {
        let id = self.next.get();
        self.next.set(id + 1);
        id
    }

    /// An element's block, marked with `path`: its first line, and its
    /// closing line, the last but trailing blank ones (`closing_line`).
    pub fn mark(&self, block: &mut Block, path: Vec<i32>) {
        if block.lines.is_empty() {
            return;
        }
        let mut close = block.lines.len() - 1;
        while close > 0 && block.lines[close].text.is_empty() {
            close -= 1;
        }
        let (open_id, close_id) = (self.id(), self.id());
        block.lines[0].marks.push(open_id);
        block.lines[close].marks.push(close_id);
        self.markers.borrow_mut().push(Marker {
            path,
            open: open_id,
            close: close_id,
        });
    }

    /// `resolve_source_code_info_locations`: each marker, in the order
    /// made, as a location over the lines `flush` writes (a redacted line
    /// is not counted); a marker whose line is not written is dropped.
    /// Columns are the line's indentation, and the end its indentation
    /// plus its text's length in code points.
    pub fn resolve(
        &self,
        out: &Block,
        redact_comments: bool,
        redact_orphans: bool,
    ) -> Vec<Location> {
        let mut index: HashMap<u32, (usize, usize, usize)> = HashMap::default();
        let mut counter = 0;
        for line in &out.lines {
            let survives = match line.kind {
                LineType::Comment => !redact_comments,
                LineType::Orphan => !redact_orphans,
                LineType::Code => true,
            };
            if !survives {
                continue;
            }
            let start = line.level * TAB_SIZE;
            let end = start + line.text.chars().count();
            for &m in &line.marks {
                index.insert(m, (counter, start, end));
            }
            counter += 1;
        }
        let mut locations = Vec::new();
        for m in self.markers.borrow().iter() {
            let (Some(&(start_line, start_col, _)), Some(&(end_line, _, end_col))) =
                (index.get(&m.open), index.get(&m.close))
            else {
                continue;
            };
            let span = if start_line == end_line {
                vec![start_line as i32, start_col as i32, end_col as i32]
            } else {
                vec![
                    start_line as i32,
                    start_col as i32,
                    end_line as i32,
                    end_col as i32,
                ]
            };
            locations.push(Location {
                path: m.path.clone(),
                span,
            });
        }
        locations
    }
}
