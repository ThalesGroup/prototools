// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

// ── Stack frame ────────────────────────────────────────────────────────────────

/// One entry on the nesting stack.
pub(super) enum Frame {
    Message {
        placeholder_start: usize, // absolute offset of this placeholder in `out`
        ohb: usize,               // length_overhang_count (extra bytes in varint_room)
        content_start: usize,     // first byte of child content (after placeholder)
        acw: usize,               // accumulated child waste from inner placeholders
    },
    Group {
        field_number: u64,
        open_ended: bool,
        mismatched_end: Option<u64>,
        end_tag_ohb: Option<u64>,
        acw: usize, // accumulated child waste (propagated to parent)
    },
}

impl Frame {
    pub(super) fn acw_mut(&mut self) -> &mut usize {
        match self {
            Frame::Message { acw, .. } => acw,
            Frame::Group { acw, .. } => acw,
        }
    }
}
