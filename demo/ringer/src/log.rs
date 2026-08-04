// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The call log: a `Log` message holding one `Entry` per round trip.
//!
//! ```text
//! message Log {
//!   repeated Entry entry = 2042;
//! }
//!
//! message Entry {
//!   bytes query    = 42;
//!   bytes response = 43;
//! }
//! ```
//!
//! **These two messages are deliberately outside the reflected schema.**  They
//! are not in `routes_service.proto`, so they are not in the descriptor set
//! ringer embeds, and nothing here consults a `DescriptorPool` — the wire form
//! is written by hand, a few lines below.  That is the point.  When protolens
//! opens the log it knows `ComputeRoutesRequest` and knows nothing whatsoever
//! about `Log` or `Entry`, so the outer envelope has to be read as raw wire
//! structure while the payloads inside it get named by the inference sweep.
//!
//! The field numbers are chosen to be awkward on purpose.  2042 needs a
//! two-byte tag, and 42/43 sit well above the one-byte range too, so none of
//! the three can be mistaken for a low-numbered field of the payload.

use std::io::Write;

/// `Log.entry`.
pub const LOG_ENTRY_FIELD: u32 = 2042;
/// `Entry.query`.
pub const ENTRY_QUERY_FIELD: u32 = 42;
/// `Entry.response`.
pub const ENTRY_RESPONSE_FIELD: u32 = 43;

/// The protobuf `LEN` wire type, the only one this module emits.
const WIRE_TYPE_LEN: u32 = 2;

/// One query and, if the call got that far, its response.
#[derive(Debug, Default)]
struct Entry {
    query: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
}

/// Accumulates entries, then renders the whole `Log`.
///
/// Shared between the codec's encoder and its decoder (see [`crate::codec`]),
/// which is why the caller keeps it behind a mutex.
#[derive(Debug, Default)]
pub struct Recorder {
    done: Vec<Entry>,
    /// A query whose response has not arrived yet.
    open: Option<Entry>,
}

impl Recorder {
    /// Records the bytes of a request **as they are about to leave**.
    ///
    /// Called from the encoder with the exact buffer handed to tonic, so a
    /// later step that rewrites the encoding into a non-canonical form has
    /// only to do so before this call for the log to stay truthful.
    pub fn record_query(&mut self, bytes: &[u8]) {
        // A second query before the first was answered: close the first out
        // with no response rather than lose it.
        if let Some(open) = self.open.take() {
            self.done.push(open);
        }
        self.open = Some(Entry {
            query: Some(bytes.to_vec()),
            response: None,
        });
    }

    /// Records the bytes of a response, pairing it with the open query.
    pub fn record_response(&mut self, bytes: &[u8]) {
        let mut entry = self.open.take().unwrap_or_default();
        entry.response = Some(bytes.to_vec());
        self.done.push(entry);
    }

    /// Serializes the accumulated entries as a `Log` message.
    ///
    /// A query still awaiting a response is included with `Entry.response`
    /// absent — a call that failed is exactly the one whose bytes you want to
    /// look at.
    pub fn encode_log(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for entry in self.done.iter().chain(self.open.iter()) {
            let mut buf = Vec::new();
            if let Some(query) = &entry.query {
                push_len_field(&mut buf, ENTRY_QUERY_FIELD, query);
            }
            if let Some(response) = &entry.response {
                push_len_field(&mut buf, ENTRY_RESPONSE_FIELD, response);
            }
            push_len_field(&mut out, LOG_ENTRY_FIELD, &buf);
        }
        out
    }

    /// True if nothing was ever recorded, so the caller can skip the write.
    pub fn is_empty(&self) -> bool {
        self.done.is_empty() && self.open.is_none()
    }
}

/// Appends `field` as a length-delimited record carrying `value`.
fn push_len_field(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    push_varint(out, u64::from(field) << 3 | u64::from(WIRE_TYPE_LEN));
    push_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

/// Appends a base-128 varint, low group first, continuation bit set on all
/// but the last.
fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Writes `bytes` to `path`, creating the parent directory.
pub fn write_to(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_matches_the_worked_examples() {
        let mut out = Vec::new();
        push_varint(&mut out, 0);
        assert_eq!(out, [0x00]);

        // Log.entry = 2042 -> tag (2042 << 3) | 2 = 16338.
        let mut out = Vec::new();
        push_varint(&mut out, u64::from(LOG_ENTRY_FIELD) << 3 | 2);
        assert_eq!(out, [0xd2, 0x7f]);

        // Entry.query = 42 -> 338; Entry.response = 43 -> 346.
        let mut out = Vec::new();
        push_varint(&mut out, u64::from(ENTRY_QUERY_FIELD) << 3 | 2);
        assert_eq!(out, [0xd2, 0x02]);
        let mut out = Vec::new();
        push_varint(&mut out, u64::from(ENTRY_RESPONSE_FIELD) << 3 | 2);
        assert_eq!(out, [0xda, 0x02]);
    }

    #[test]
    fn one_round_trip_is_one_entry_holding_both_payloads() {
        let mut rec = Recorder::default();
        assert!(rec.is_empty());
        rec.record_query(b"\x08\x01");
        rec.record_response(b"\x10\x02");
        assert!(!rec.is_empty());

        assert_eq!(
            rec.encode_log(),
            [
                0xd2, 0x7f, 0x0a, // Log.entry, 10 bytes
                0xd2, 0x02, 0x02, 0x08, 0x01, // Entry.query, 2 bytes
                0xda, 0x02, 0x02, 0x10, 0x02, // Entry.response, 2 bytes
            ]
        );
    }

    #[test]
    fn an_unanswered_query_is_still_logged() {
        let mut rec = Recorder::default();
        rec.record_query(b"\x08\x01");
        assert_eq!(
            rec.encode_log(),
            [0xd2, 0x7f, 0x05, 0xd2, 0x02, 0x02, 0x08, 0x01]
        );
    }

    #[test]
    fn a_second_query_does_not_evict_the_first() {
        let mut rec = Recorder::default();
        rec.record_query(b"\x08\x01");
        rec.record_query(b"\x08\x02");
        rec.record_response(b"\x10\x02");

        let log = rec.encode_log();
        // Two `Log.entry` records, and the first query survived.
        assert_eq!(log.windows(2).filter(|w| *w == [0xd2, 0x7f]).count(), 2);
        assert!(log.windows(2).any(|w| w == [0x08, 0x01]));
    }
}
