// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The call log: a `bobapp.v1.Log` holding one `Entry` per round trip.
//!
//! ```text
//! message Entry {
//!   google.protobuf.Timestamp at = 1;
//!   string method   = 2;   // last segment only: "SearchText" or "ComputeRoutes"
//!   // Places pair — in embedded set:
//!   SearchTextRequest  places_request  = 4;
//!   SearchTextResponse places_response = 5;
//!   // Routes pair — NOT in embedded set:
//!   ComputeRoutesRequest  routes_request  = 6;
//!   ComputeRoutesResponse routes_response = 7;
//! }
//! message Log { repeated Entry entry = 1; }
//! ```
//!
//! **The embedded set contains only Places FDPs** (spec 0350).  The log
//! envelope (`bobapp.v1.Log`, `bobapp.v1.Entry`) and the Routes types are
//! NOT embedded.  All log encoding uses the *extra* pool (loaded from
//! `BOBAPP_EXTRA_DESCRIPTOR_SET`), which knows every type.  The embedded pool
//! is used only by `request.rs` and `codec.rs` for the live API calls.

use std::{io::Write, time::SystemTime};

use anyhow::{Context, Result};
use prost::encoding::{encode_key, encode_varint, WireType};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, Value};

use crate::request::{message, set};

/// `bobapp.v1.Log`, fully qualified.
pub const LOG_TYPE: &str = "bobapp.v1.Log";
/// `bobapp.v1.Entry`, fully qualified.
pub const ENTRY_TYPE: &str = "bobapp.v1.Entry";

/// Which service pair a log entry belongs to.
#[derive(Debug, Clone, Copy)]
pub enum EntryKind {
    /// A Places/SearchText round trip.  Fields 4/5 in the entry.
    Places,
    /// A Routes/ComputeRoutes round trip.  Fields 6/7 in the entry.
    Routes,
}

/// One call: when it went out, where to, and the bytes in each direction.
#[derive(Debug)]
struct Entry {
    at: SystemTime,
    /// Last path segment of the gRPC method (e.g. "SearchText").
    method: String,
    kind: EntryKind,
    request: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
}

/// Accumulates entries, then renders the whole `Log`.
///
/// All encoding uses `pool`, which is the extra pool (knows all types).
/// Shared between the codec's encoder and its decoder (see [`crate::codec`]),
/// which is why the caller keeps it behind a mutex.
#[derive(Debug)]
pub struct Recorder {
    pool: DescriptorPool,
    done: Vec<Entry>,
    /// A request whose response has not arrived yet.
    open: Option<Entry>,
}

impl Recorder {
    pub fn new(pool: DescriptorPool) -> Self {
        Self {
            pool,
            done: Vec::new(),
            open: None,
        }
    }

    /// Records the bytes of a request **as they are about to leave**.
    ///
    /// `method` is the full gRPC path; the short name (last segment) is
    /// derived here.  `kind` determines which typed fields are written.
    pub fn record_request(&mut self, method: &str, kind: EntryKind, bytes: &[u8]) {
        // Last non-empty segment of the path, e.g. "SearchText".
        let method_name = method
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or(method)
            .to_owned();

        // A second request before the first was answered: close the first out
        // with no response rather than lose it.
        if let Some(open) = self.open.take() {
            self.done.push(open);
        }
        self.open = Some(Entry {
            at: SystemTime::now(),
            method: method_name,
            kind,
            request: Some(bytes.to_vec()),
            response: None,
        });
    }

    /// Records the bytes of a response, pairing it with the open request.
    pub fn record_response(&mut self, bytes: &[u8]) {
        let Some(mut entry) = self.open.take() else {
            return; // A response to nothing is not an entry.
        };
        entry.response = Some(bytes.to_vec());
        self.done.push(entry);
    }

    /// Serializes the accumulated entries as a `bobapp.v1.Log` message.
    ///
    /// A request still awaiting a response is included with its response
    /// absent — a call that failed is exactly the one whose bytes you want.
    ///
    /// Request and response bytes are written verbatim into the typed fields —
    /// no decode/re-encode cycle, so every non-canonical form survives.
    pub fn encode_log(&self) -> Result<Vec<u8>> {
        let pool = &self.pool;
        let mut out = Vec::new();
        for entry in self.done.iter().chain(self.open.iter()) {
            let entry_bytes = entry.encode(pool)?;
            // Log.entry is field 1, length-delimited.
            encode_key(1, WireType::LengthDelimited, &mut out);
            encode_varint(entry_bytes.len() as u64, &mut out);
            out.extend_from_slice(&entry_bytes);
        }
        Ok(out)
    }

    /// True if nothing was ever recorded, so the caller can skip the write.
    pub fn is_empty(&self) -> bool {
        self.done.is_empty() && self.open.is_none()
    }
}

impl Entry {
    /// Encodes the entry as raw proto bytes.
    ///
    /// Fields 1–2 (timestamp, method) are built via `DynamicMessage`.
    /// Fields 4–7 (typed request/response payloads) are appended verbatim as
    /// length-delimited records — no decode/re-encode, so every non-canonical
    /// form written by `anomaly::rewrite_request` survives into the log.
    fn encode(&self, pool: &DescriptorPool) -> Result<Vec<u8>> {
        // Fields 1–2 via DynamicMessage (they have no anomalies to preserve).
        let mut header = DynamicMessage::new(message(pool, ENTRY_TYPE)?);
        set(&mut header, "at", Value::Message(timestamp(pool, self.at)?))?;
        set(&mut header, "method", Value::String(self.method.clone()))?;
        let mut out = Vec::with_capacity(header.encoded_len() + 256);
        header.encode(&mut out).context("encoding entry header")?;

        // Fields 4–7: write raw bytes as length-delimited records.
        let (req_field, resp_field) = match self.kind {
            EntryKind::Places => (4u32, 5u32),
            EntryKind::Routes => (6u32, 7u32),
        };
        if let Some(req) = &self.request {
            encode_key(req_field, WireType::LengthDelimited, &mut out);
            encode_varint(req.len() as u64, &mut out);
            out.extend_from_slice(req);
        }
        if let Some(resp) = &self.response {
            encode_key(resp_field, WireType::LengthDelimited, &mut out);
            encode_varint(resp.len() as u64, &mut out);
            out.extend_from_slice(resp);
        }
        Ok(out)
    }
}

/// A `google.protobuf.Timestamp` for `at`.
fn timestamp(pool: &DescriptorPool, at: SystemTime) -> Result<DynamicMessage> {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .context("the system clock is before the epoch")?;
    let mut timestamp = DynamicMessage::new(message(pool, "google.protobuf.Timestamp")?);
    set(
        &mut timestamp,
        "seconds",
        Value::I64(since_epoch.as_secs() as i64),
    )?;
    set(
        &mut timestamp,
        "nanos",
        Value::I32(since_epoch.subsec_nanos() as i32),
    )?;
    Ok(timestamp)
}

/// Writes `bytes` to `path`, creating the parent directory.
pub fn write_to(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
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

    fn dummy_pool() -> DescriptorPool {
        DescriptorPool::decode(&include_bytes!(concat!(env!("OUT_DIR"), "/bobapp.desc"))[..])
            .expect("embedded descriptor set")
    }

    #[test]
    fn recorder_is_empty_until_first_request() {
        let mut rec = Recorder::new(dummy_pool());
        assert!(rec.is_empty());
        rec.record_request("/svc/M", EntryKind::Places, b"\x08\x01");
        assert!(!rec.is_empty());
    }

    #[test]
    fn unanswered_request_is_not_evicted_by_a_second() {
        // Two requests in a row: the first closes without a response, the
        // second stays open.  Both must be present.
        let mut rec = Recorder::new(dummy_pool());
        rec.record_request("/svc/A", EntryKind::Places, b"\x08\x01");
        rec.record_request("/svc/B", EntryKind::Routes, b"\x08\x02");
        // One entry in `done` (the closed first), one in `open`.
        assert_eq!(rec.done.len(), 1);
        assert!(rec.open.is_some());
        assert_eq!(rec.done[0].request.as_deref(), Some(&b"\x08\x01"[..]));
        assert!(rec.done[0].response.is_none());
    }

    #[test]
    fn response_pairs_with_open_request() {
        let mut rec = Recorder::new(dummy_pool());
        rec.record_request("/svc/M", EntryKind::Places, b"\x08\x01");
        rec.record_response(b"\x10\x02");
        // Closed into `done`; `open` is now empty.
        assert_eq!(rec.done.len(), 1);
        assert!(rec.open.is_none());
        assert_eq!(rec.done[0].request.as_deref(), Some(&b"\x08\x01"[..]));
        assert_eq!(rec.done[0].response.as_deref(), Some(&b"\x10\x02"[..]));
    }

    #[test]
    fn non_canonical_bytes_are_preserved_verbatim() {
        // A padded varint — nothing in the recorder may normalize it.
        let odd = b"\x08\x85\x80\x80\x80\x00";
        let mut rec = Recorder::new(dummy_pool());
        rec.record_request("/svc/M", EntryKind::Routes, odd);
        assert_eq!(
            rec.open.as_ref().unwrap().request.as_deref(),
            Some(&odd[..])
        );
    }

    #[test]
    fn method_is_shortened_to_last_segment() {
        let mut rec = Recorder::new(dummy_pool());
        rec.record_request(
            "/google.maps.places.v1.Places/SearchText",
            EntryKind::Places,
            b"",
        );
        assert_eq!(rec.open.as_ref().unwrap().method, "SearchText");

        let mut rec2 = Recorder::new(dummy_pool());
        rec2.record_request(
            "/google.maps.routing.v2.Routes/ComputeRoutes",
            EntryKind::Routes,
            b"",
        );
        assert_eq!(rec2.open.as_ref().unwrap().method, "ComputeRoutes");
    }
}
