// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use crate::helpers::write_varint_ohb;

// ── Constants ──────────────────────────────────────────────────────────────────

/// Sentinel stored in `next_placeholder` when there is no successor.
pub(super) const NO_NEXT: u64 = 0xFF_FFFF_FFFF; // 5 bytes of 0xFF

/// Base overhead of one placeholder: waste(1) + next(5) + varint_room_base(5).
pub(super) const BASE_OVERHEAD: usize = 11;

// ── Placeholder helpers (Strategy C2) ─────────────────────────────────────────

/// Write `BASE_OVERHEAD + ohb` placeholder bytes at the current end of `out`.
///
/// Updates the forward linked list: the previously opened placeholder's
/// `next_placeholder` field is updated to point at the new placeholder.
///
/// Returns `(placeholder_start, content_start)`.
pub(super) fn write_placeholder(
    out: &mut Vec<u8>,
    ohb: usize,
    first_placeholder: &mut Option<usize>,
    last_placeholder: &mut Option<usize>,
) -> (usize, usize) {
    let placeholder_start = out.len();

    // waste (1 byte, filled on `}`)
    out.push(0u8);

    // next_placeholder (5 bytes, initially SENTINEL)
    let sentinel = NO_NEXT.to_le_bytes();
    out.extend_from_slice(&sentinel[..5]);

    // varint_room (5 + ohb bytes, all zeros; filled flush-right on `}`)
    for _ in 0..5 + ohb {
        out.push(0u8);
    }

    // Link into the forward linked list (buffer order = opening order).
    if let Some(last_ph) = *last_placeholder {
        let next_bytes = (placeholder_start as u64).to_le_bytes();
        out[last_ph + 1..last_ph + 6].copy_from_slice(&next_bytes[..5]);
    }
    if first_placeholder.is_none() {
        *first_placeholder = Some(placeholder_start);
    }
    *last_placeholder = Some(placeholder_start);

    (placeholder_start, out.len())
}

/// Fill in a MESSAGE placeholder when its `}` is reached.
///
/// `frame_acw` is the accumulated waste from inner placeholders within this
/// frame's content region (needed to compute the correct compacted length).
/// Returns the total waste (placeholder waste + frame_acw) to propagate up.
pub(super) fn fill_placeholder(
    out: &mut [u8],
    placeholder_start: usize,
    ohb: usize,
    content_start: usize,
    frame_acw: usize,
) -> usize {
    // Compacted child length = raw length − waste from inner placeholders.
    let child_len_raw = out.len() - content_start;
    let child_len_compacted = child_len_raw - frame_acw;

    // Encode compacted length (with optional ohb non-minimal bytes).
    let ohb_opt = if ohb > 0 { Some(ohb as u64) } else { None };
    let mut tmp = Vec::new();
    write_varint_ohb(child_len_compacted as u64, ohb_opt, &mut tmp);
    let k = tmp.len(); // actual bytes used (varint_bytes + ohb)

    // Write varint flush-right into varint_room.
    let varint_room_end = placeholder_start + BASE_OVERHEAD + ohb;
    let varint_write_start = varint_room_end - k;
    out[varint_write_start..varint_room_end].copy_from_slice(&tmp);

    // Set waste.
    let waste = BASE_OVERHEAD + ohb - k;
    out[placeholder_start] = waste as u8;

    // Return total waste to propagate to the parent frame.
    waste + frame_acw
}

// ── Forward compaction pass ───────────────────────────────────────────────────

/// Remove placeholder waste bytes in a single left-to-right pass.
///
/// Traverses the forward linked list of placeholders and uses `copy_within`
/// to compact the buffer in-place.  Each byte is moved at most once → O(n).
pub(super) fn compact(out: &mut Vec<u8>, first_placeholder: usize) {
    let total_len = out.len();
    let mut read_pos = 0usize;
    let mut write_pos = 0usize;
    let mut cursor = first_placeholder;

    #[cfg(debug_assertions)]
    eprintln!(
        "[encode_text] compact: total_len={} first_placeholder={}",
        total_len, first_placeholder
    );

    loop {
        // Copy real data that sits before this placeholder.
        if cursor > read_pos {
            out.copy_within(read_pos..cursor, write_pos);
            write_pos += cursor - read_pos;
        }

        // Read waste and next BEFORE any copy_within can overwrite them.
        let waste = out[cursor] as usize;
        let mut next_bytes = [0u8; 8];
        next_bytes[..5].copy_from_slice(&out[cursor + 1..cursor + 6]);
        let next = u64::from_le_bytes(next_bytes);

        // Skip the wasted prefix; the varint starts at cursor + waste.
        read_pos = cursor + waste;

        if next == NO_NEXT {
            break;
        }
        cursor = next as usize;
    }

    // Copy everything from the last varint onwards.
    if read_pos < total_len {
        out.copy_within(read_pos..total_len, write_pos);
        write_pos += total_len - read_pos;
    }

    out.truncate(write_pos);

    #[cfg(debug_assertions)]
    eprintln!(
        "[encode_text] compact done: final_len={} (saved {} bytes)",
        write_pos,
        total_len - write_pos
    );
}
