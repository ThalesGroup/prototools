// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The call log: a `bobapp.v1.Log` holding one `Entry` per round trip.
//!
//! ```text
//! message Entry {
//!   google.protobuf.Timestamp at = 1;
//!   string method   = 2;
//!   string note     = 3;
//!   bytes  request  = 4;
//!   bytes  response = 5;
//! }
//! message Log { repeated Entry entry = 1; }
//! ```
//!
//! **The envelope is in the schema bobapp embeds, and in no schema anyone
//! else has.**  `bobapp/v1/log.proto` is compiled into the descriptor set
//! `build.rs` bakes into this executable, alongside the 39 googleapis files
//! `routes_service.proto` drags in.  So the schema recovered *from the binary*
//! names `Log` and `Entry`, and a stock `googleapis.desc` — which has never
//! heard of Bob's app — sees the same bytes as an unnamed envelope around
//! payloads it does know.
//!
//! That asymmetry is the point.  It is the difference between the two readings
//! of the same file, and it is repaired by naming the envelope's `bytes` fields
//! with an override rather than by finding a better schema.
//!
//! Because the envelope is now a real message, it is built the same way the
//! request is: a `DynamicMessage` against the pool, by field name (spec 0241
//! S14).  Nothing here writes a varint by hand.

use std::{io::Write, time::SystemTime};

use anyhow::{Context, Result};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, Value};

use crate::request::{message, set};

/// `bobapp.v1.Log`, fully qualified.
pub const LOG_TYPE: &str = "bobapp.v1.Log";
/// `bobapp.v1.Entry`, fully qualified.
pub const ENTRY_TYPE: &str = "bobapp.v1.Entry";

/// One call: when it went out, where to, and the bytes in each direction.
#[derive(Debug)]
struct Entry {
    at: SystemTime,
    method: String,
    /// How the call went, as far as the entry knows when it is closed.
    note: &'static str,
    request: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
}

/// Accumulates entries, then renders the whole `Log`.
///
/// Shared between the codec's encoder and its decoder (see [`crate::codec`]),
/// which is why the caller keeps it behind a mutex.
#[derive(Debug, Default)]
pub struct Recorder {
    done: Vec<Entry>,
    /// A request whose response has not arrived yet.
    open: Option<Entry>,
}

impl Recorder {
    /// Records the bytes of a request **as they are about to leave**.
    ///
    /// Called from the encoder with the exact buffer handed to tonic, so a
    /// step that rewrites the encoding into a non-canonical form has only to
    /// do so before this call for the log to stay truthful.
    pub fn record_request(&mut self, method: &str, bytes: &[u8]) {
        // A second request before the first was answered: close the first out
        // with no response rather than lose it.
        if let Some(open) = self.open.take() {
            self.done.push(open);
        }
        self.open = Some(Entry {
            at: SystemTime::now(),
            method: method.to_owned(),
            note: "sent",
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
        entry.note = "ok";
        self.done.push(entry);
    }

    /// Serializes the accumulated entries as a `bobapp.v1.Log` message.
    ///
    /// A request still awaiting a response is included with `Entry.response`
    /// absent — a call that failed is exactly the one whose bytes you want to
    /// look at.
    pub fn encode_log(&self, pool: &DescriptorPool) -> Result<Vec<u8>> {
        let mut log = DynamicMessage::new(message(pool, LOG_TYPE)?);
        let entries: Result<Vec<Value>> = self
            .done
            .iter()
            .chain(self.open.iter())
            .map(|entry| Ok(Value::Message(entry.encode(pool)?)))
            .collect();
        set(&mut log, "entry", Value::List(entries?))?;

        let mut out = Vec::with_capacity(log.encoded_len());
        log.encode(&mut out).context("encoding the log")?;
        Ok(out)
    }

    /// True if nothing was ever recorded, so the caller can skip the write.
    pub fn is_empty(&self) -> bool {
        self.done.is_empty() && self.open.is_none()
    }
}

impl Entry {
    fn encode(&self, pool: &DescriptorPool) -> Result<DynamicMessage> {
        let mut entry = DynamicMessage::new(message(pool, ENTRY_TYPE)?);
        set(&mut entry, "at", Value::Message(timestamp(pool, self.at)?))?;
        set(&mut entry, "method", Value::String(self.method.clone()))?;
        set(&mut entry, "note", Value::String(self.note.to_owned()))?;
        if let Some(request) = &self.request {
            set(&mut entry, "request", Value::Bytes(request.clone().into()))?;
        }
        if let Some(response) = &self.response {
            set(
                &mut entry,
                "response",
                Value::Bytes(response.clone().into()),
            )?;
        }
        Ok(entry)
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
    use prost_reflect::ReflectMessage;

    use super::*;

    /// The same descriptor set the binary embeds, so these tests exercise the
    /// real `bobapp.v1.Log` and not a stand-in.
    fn pool() -> DescriptorPool {
        DescriptorPool::decode(crate::DESCRIPTOR_SET).expect("the embedded descriptor set parses")
    }

    /// Reads a `Log` back through the same schema, so an assertion below can
    /// speak in field names rather than in bytes.
    fn decode(pool: &DescriptorPool, bytes: &[u8]) -> DynamicMessage {
        DynamicMessage::decode(message(pool, LOG_TYPE).unwrap(), bytes).expect("a Log decodes")
    }

    fn entries(log: &DynamicMessage) -> Vec<DynamicMessage> {
        log.get_field_by_name("entry")
            .expect("Log.entry")
            .as_list()
            .expect("entry is repeated")
            .iter()
            .map(|v| v.as_message().expect("an Entry").clone())
            .collect()
    }

    fn field_bytes(entry: &DynamicMessage, name: &str) -> Option<Vec<u8>> {
        let field = entry.descriptor().get_field_by_name(name).expect("a field");
        entry
            .has_field(&field)
            .then(|| entry.get_field(&field).as_bytes().expect("bytes").to_vec())
    }

    #[test]
    fn the_envelope_is_in_the_embedded_schema() {
        let pool = pool();
        assert!(
            pool.get_message_by_name(LOG_TYPE).is_some(),
            "the descriptor set bobapp embeds must define {LOG_TYPE}"
        );
        assert!(pool.get_message_by_name(ENTRY_TYPE).is_some());
    }

    #[test]
    fn one_round_trip_is_one_entry_holding_both_payloads() {
        let pool = pool();
        let mut rec = Recorder::default();
        assert!(rec.is_empty());
        rec.record_request("/svc/M", b"\x08\x01");
        rec.record_response(b"\x10\x02");
        assert!(!rec.is_empty());

        let log = decode(&pool, &rec.encode_log(&pool).unwrap());
        let entries = entries(&log);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].get_field_by_name("method").unwrap().as_str(),
            Some("/svc/M")
        );
        assert_eq!(
            field_bytes(&entries[0], "request").as_deref(),
            Some(&b"\x08\x01"[..])
        );
        assert_eq!(
            field_bytes(&entries[0], "response").as_deref(),
            Some(&b"\x10\x02"[..])
        );
    }

    /// The property the log exists for: what went out is what was recorded,
    /// byte for byte, whatever shape the encoder gave it.
    #[test]
    fn an_unanswered_request_is_still_logged_with_its_exact_bytes() {
        let pool = pool();
        let mut rec = Recorder::default();
        // A deliberately non-canonical encoding: a padded varint. Nothing in
        // the recorder may normalize it away.
        let odd = b"\x08\x85\x80\x80\x80\x00";
        rec.record_request("/svc/M", odd);

        let log = decode(&pool, &rec.encode_log(&pool).unwrap());
        let entries = entries(&log);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            field_bytes(&entries[0], "request").as_deref(),
            Some(&odd[..])
        );
        assert_eq!(
            field_bytes(&entries[0], "response"),
            None,
            "a request with no answer is logged without one"
        );
    }

    #[test]
    fn a_second_request_does_not_evict_the_first() {
        let pool = pool();
        let mut rec = Recorder::default();
        rec.record_request("/svc/A", b"\x08\x01");
        rec.record_request("/svc/B", b"\x08\x02");
        rec.record_response(b"\x10\x02");

        let log = decode(&pool, &rec.encode_log(&pool).unwrap());
        let entries = entries(&log);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            field_bytes(&entries[0], "request").as_deref(),
            Some(&b"\x08\x01"[..]),
            "the first request survived, unanswered"
        );
        assert_eq!(field_bytes(&entries[0], "response"), None);
        assert_eq!(
            field_bytes(&entries[1], "response").as_deref(),
            Some(&b"\x10\x02"[..])
        );
    }
}
