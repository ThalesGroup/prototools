// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A `tonic::codec::Codec` over `prost_reflect::DynamicMessage`.
//!
//! This is the whole reflection story (spec 0241 S10): the method is a string,
//! the messages are dynamic, and no googleapis type is known at compile time.
//! It is also where the call log is filled, because this is the last place
//! that sees the request as bytes before tonic frames it.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use bytes::{Buf, BufMut};
use prost::Message;
use prost_reflect::{DynamicMessage, MessageDescriptor, ReflectMessage};
use tonic::{
    codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder},
    Status,
};

use crate::{anomaly, log::Recorder};

/// Shared between the encoder and the decoder of a single call.
pub type SharedRecorder = Arc<Mutex<Recorder>>;

/// Encodes and decodes one method's messages reflectively.
///
/// Only the response descriptor is held: the encoder gets the request's
/// descriptor along with the message it is handed, so it never needs to look
/// one up.
pub struct DynamicCodec {
    /// The full method path, recorded with the request so that a reader of
    /// the log can tell one call from another without matching payload shapes.
    method: String,
    response: MessageDescriptor,
    recorder: SharedRecorder,
}

impl DynamicCodec {
    pub fn new(method: &str, response: MessageDescriptor, recorder: SharedRecorder) -> Self {
        Self {
            method: method.to_owned(),
            response,
            recorder,
        }
    }
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder {
            method: self.method.clone(),
            recorder: Arc::clone(&self.recorder),
        }
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder {
            descriptor: self.response.clone(),
            recorder: Arc::clone(&self.recorder),
        }
    }
}

pub struct DynamicEncoder {
    method: String,
    recorder: SharedRecorder,
}

impl Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = Status;

    /// Serializes `item` and hands the bytes to tonic.
    ///
    /// `dst` is the *message* buffer: tonic writes the five-byte gRPC frame
    /// header (compression flag plus big-endian length) around it afterwards,
    /// so what is recorded here carries no header and needs no stripping
    /// (spec 0241 S15).
    ///
    /// The order of the three steps below is the contract this whole demo
    /// rests on.  The bytes are serialized, then recorded, then written — so
    /// the log holds exactly what egressed, not a re-encoding of the same
    /// message, which may legitimately differ in field order and default
    /// elision (S16).  [`anomaly::rewrite`] sits between `encode` and
    /// `record_request` for exactly that reason: it is the last thing that
    /// touches the bytes, so the log keeps telling the truth without knowing
    /// it is there.
    fn encode(&mut self, item: DynamicMessage, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        let mut bytes = Vec::with_capacity(item.encoded_len());
        item.encode(&mut bytes)
            .map_err(|e| Status::internal(format!("encoding the request: {e}")))?;

        let bytes = anomaly::rewrite_request(&bytes, &item.descriptor())
            .map_err(|e| Status::internal(format!("rewriting the request: {e}")))?;

        // ── the wire is one line away ──────────────────────────────────────
        self.recorder
            .lock()
            .expect("recorder mutex")
            .record_request(&self.method, &bytes);
        dst.put_slice(&bytes);
        Ok(())
    }
}

pub struct DynamicDecoder {
    descriptor: MessageDescriptor,
    recorder: SharedRecorder,
}

impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        let bytes = src.copy_to_bytes(src.remaining());
        self.recorder
            .lock()
            .expect("recorder mutex")
            .record_response(&bytes);

        let message = DynamicMessage::decode(self.descriptor.clone(), bytes)
            .map_err(|e| Status::internal(format!("decoding the response: {e}")))?;
        Ok(Some(message))
    }
}
