// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The four things bobapp does to its own bytes that no library would.
//!
//! bobapp is a little weird.  On the route call it writes one varint padded
//! out to five bytes rather than in the minimal form every encoder produces,
//! and it appends a field its own schema has never heard of.  On the place
//! lookup it writes a singular field twice — the first occurrence being a
//! diagnostic nobody should keep — and then it gets killed before the last
//! record of the log is finished.
//!
//! Every one of those is legal, or nearly so, and every one survives a round
//! trip through a conforming parser, which is exactly why nobody notices until
//! someone opens the bytes.
//!
//! The first three go out through text and back.  `render_as_text` turns the
//! encoded message into enhanced textproto, the edits below are line edits on
//! that text, and `render_as_bytes` puts it back on the wire — carrying the
//! `#@` annotations' instructions with it, including the ones no encoder
//! offers.  That is the whole trick: `val_ohb: 4` is a *request* for a
//! non-minimal varint, a bare field number is a *request* for a field with no
//! name, and a line written twice is written twice.  Nothing here hand-rolls
//! a varint.
//!
//! Encoding needs no descriptor set at all — the annotations are
//! self-describing.  Only the render needs the pool, and only so that the
//! edits below can name a field rather than count one.
//!
//! The fourth is not an encoding at all.  [`cut_short`] drops bytes off the
//! end, which is the one thing a *file* can suffer that a message cannot.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MapKey, MessageDescriptor, Value};
use prototext_core::{
    render_as_bytes, render_as_text, serialize::common::escape::escape_bytes, RenderOpts,
};

use crate::request;

/// The scalar whose varint comes out padded.
const PADDED: &str = "travel_mode";
/// How many bytes that varint occupies once padded.
const PADDING: usize = 4;

/// The field number bobapp stamps its own version into.  Nothing in the
/// embedded schema, and nothing in googleapis, declares it.
const AGENT_FIELD: u32 = 99;
/// What it stamps.
const AGENT: &str = "bobapp/0.9.3-rc2";

/// `SearchTextRequest.text_query`, whose number the lookup rewrite has to
/// write by hand because the extra occurrence has no descriptor to be looked
/// up from.
const QUERY_FIELD: u32 = 1;

/// The googleapis message bobapp's leftover debug trace is shaped like.
///
/// Nothing in the embedded schema declares it — it comes out of the same
/// descriptor set the lookup itself was built against, which is the point:
/// the bytes hiding in `text_query` are a *named* message, and the reader who
/// has that set can name them.  Scored against googleapis, these bytes have
/// exactly one candidate, so a heat cue does not merely say "this is not a
/// string" — it says which message it is.
const TRACE_TYPE: &str = "google.rpc.Status";
/// `Status.code`, 13 = `INTERNAL`.  bobapp reaches for its error-reporting
/// helper to write a trace, which is how the trace ends up shaped like a
/// failure that never happened.
const TRACE_CODE: i32 = 13;
/// `Status.message`.
const TRACE_MESSAGE: &str = "debug: outbound call authenticated";

/// `google.protobuf.Any`, the arm `Status.details` is repeated over.
const ANY_TYPE: &str = "google.protobuf.Any";
/// What goes inside it.
const DETAIL_TYPE: &str = "google.rpc.ErrorInfo";
/// `ErrorInfo.reason`, as bobapp fills it.
const TRACE_REASON: &str = "DEBUG_TRACE";
/// `ErrorInfo.domain`, likewise.
const TRACE_DOMAIN: &str = "bobapp";
/// The `ErrorInfo.metadata` key the key is filed under — the name of the
/// header the call really did travel in.
const KEY_HEADER: &str = "x-goog-api-key";

/// A string shaped exactly like a Google API key and belonging to nobody.
///
/// It is 39 characters beginning `AIza`, because that is what makes the room
/// recognize it in the half-second it is on screen.  It has never been a
/// credential, here or anywhere.
const FAKE_KEY: &str = "AIzaSyB0b5REKn0tAr3aLk3yD0ntB0th3rTry1t";

/// Returns `bytes` re-encoded the way bobapp puts a request on the wire.
///
/// `request` is only the descriptor the bytes were encoded against, used to
/// render them with field names — and, for the lookup, to reach the pool the
/// trace message is built from.
///
/// Each edit below is written in terms of one message, so a request of any
/// other type goes out as encoded.  That is also what the log shows: two
/// pairs of entries, odd in two different ways.
pub fn rewrite_request(bytes: &[u8], request: &MessageDescriptor) -> Result<Vec<u8>> {
    match request.full_name() {
        request::REQUEST_TYPE => round_trip(bytes, request, patch_request),
        request::LOOKUP_TYPE => {
            let trace = escape_bytes(&debug_trace(request.parent_pool())?);
            round_trip(bytes, request, |text| patch_lookup(text, &trace))
        }
        _ => Ok(bytes.to_vec()),
    }
}

/// Drops the last [`CUT`] bytes, leaving the final record's length header
/// promising more than the file holds.
///
/// A short file is left alone: there is no interesting way to truncate
/// something that has not been written yet.
pub fn cut_short(bytes: &[u8]) -> &[u8] {
    /// Enough to land well inside the last payload rather than on its edge.
    const CUT: usize = 1024;
    match bytes.len().checked_sub(CUT) {
        Some(keep) if keep > 0 => &bytes[..keep],
        _ => bytes,
    }
}

/// Text out, edit, bytes back.
fn round_trip(
    bytes: &[u8],
    descriptor: &MessageDescriptor,
    edit: impl FnOnce(&str) -> Result<String>,
) -> Result<Vec<u8>> {
    let text = render_as_text(
        bytes,
        Some(descriptor),
        RenderOpts {
            assume_binary: true,
            include_annotations: true,
            ..RenderOpts::default()
        },
    )
    .context("rendering as text")?;
    let text = String::from_utf8(text).context("the rendered text is not UTF-8")?;

    let patched = edit(&text)?;

    let bytes =
        render_as_bytes(patched.as_bytes(), RenderOpts::default()).context("re-encoding")?;
    Ok(bytes.into_owned())
}

/// A padded varint, and a field nobody declared.
fn patch_request(text: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len() + 64);
    let mut padded = false;

    for line in text.lines() {
        out.push_str(line);
        // A top-level scalar, so no leading indent to skip.
        if let Some(annotated) = line.strip_prefix(&format!("{PADDED}: ")) {
            if !annotated.contains("#@ ") {
                bail!("`{PADDED}` was rendered without an annotation to extend");
            }
            out.push_str(&format!("; val_ohb: {PADDING}"));
            padded = true;
        }
        out.push('\n');
    }

    if !padded {
        bail!("the rendered request has no top-level `{PADDED}` line");
    }
    out.push_str(&format!(
        "{AGENT_FIELD}: \"{AGENT}\"  #@ string = {AGENT_FIELD}\n"
    ));
    Ok(out)
}

/// The debug trace bobapp leaves in the query it is about to send.
///
/// A `google.rpc.Status` carrying an `Any` carrying a `google.rpc.ErrorInfo`
/// that files the key under the name of the header it travelled in — the
/// shape a "log the credential we used" line takes when somebody reaches for
/// the nearest structured type instead of a string.  The `Any` spells
/// `google.rpc.ErrorInfo` out in its `type_url`, so a reader who expands it
/// is told what is inside before decoding a byte of it.
///
/// Every byte of the result is ASCII, so the whole message is valid UTF-8 and
/// passes for the `string` it is about to be written into.  That is what makes
/// it invisible: a conforming reader — the Places server included — sees a
/// long, ugly query and no error.
fn debug_trace(pool: &DescriptorPool) -> Result<Vec<u8>> {
    let mut info = DynamicMessage::new(request::message(pool, DETAIL_TYPE)?);
    request::set(&mut info, "reason", Value::String(TRACE_REASON.to_owned()))?;
    request::set(&mut info, "domain", Value::String(TRACE_DOMAIN.to_owned()))?;
    // One entry, so there is no map iteration order to depend on.
    let mut metadata = HashMap::new();
    metadata.insert(
        MapKey::String(KEY_HEADER.to_owned()),
        Value::String(FAKE_KEY.to_owned()),
    );
    request::set(&mut info, "metadata", Value::Map(metadata))?;

    let mut detail = DynamicMessage::new(request::message(pool, ANY_TYPE)?);
    request::set(
        &mut detail,
        "type_url",
        Value::String(format!("type.googleapis.com/{DETAIL_TYPE}")),
    )?;
    request::set(&mut detail, "value", Value::Bytes(encode(&info)?.into()))?;

    let mut status = DynamicMessage::new(request::message(pool, TRACE_TYPE)?);
    request::set(&mut status, "code", Value::I32(TRACE_CODE))?;
    request::set(
        &mut status,
        "message",
        Value::String(TRACE_MESSAGE.to_owned()),
    )?;
    request::set(
        &mut status,
        "details",
        Value::List(vec![Value::Message(detail)]),
    )?;

    let bytes = encode(&status)?;
    // The trace is about to be written into a `string`, and a proto3 string
    // has to be valid UTF-8 or the server rejects the call.  ASCII is the
    // strong form of that, and it holds only while every length varint inside
    // stays below 128 — which is to say, while the constants above stay short.
    // Growing one of them past that is a silent break, so it is checked here.
    if !bytes.is_ascii() {
        bail!(
            "the trace is {} bytes and not ASCII; shorten it until every \
             length prefix fits in one byte",
            bytes.len()
        );
    }
    Ok(bytes)
}

/// Serializes a dynamic message.
fn encode(message: &DynamicMessage) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(message.encoded_len());
    message.encode(&mut bytes).context("encoding the trace")?;
    Ok(bytes)
}

/// The query, written once with the trace and once with what was asked for.
///
/// The trace is inserted ahead of the real query, so the wire order is the
/// order the app would have written them, and last-one-wins hands every
/// ordinary reader — including the server — the harmless one.
///
/// `trace` is already escaped, so this writes a string literal and the
/// re-encode puts the trace's own bytes back exactly.
fn patch_lookup(text: &str, trace: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len() + trace.len() + 64);
    let mut doubled = false;

    for line in text.lines() {
        // A top-level scalar, so no leading indent to skip.
        if line.starts_with("text_query: ") {
            out.push_str(&format!(
                "text_query: \"{trace}\"  #@ string = {QUERY_FIELD}\n"
            ));
            doubled = true;
        }
        out.push_str(line);
        out.push('\n');
    }

    if !doubled {
        bail!("the rendered lookup has no top-level `text_query` line to write twice");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the renderer produces for a request built by [`crate::request`],
    /// trimmed to the lines the patch cares about.
    const RENDERED: &str = "\
#@ prototext: protoc
origin {  #@ Waypoint = 1
 address: \"Grenoble, France\"  #@ string = 7
}
travel_mode: DRIVE  #@ RouteTravelMode(1) = 4
language_code: \"en-US\"  #@ string = 10
";

    /// A lookup request, as the renderer gives it back with `text_query` set
    /// once.
    const RENDERED_LOOKUP: &str = "\
#@ prototext: protoc
text_query: \"boulangerie\"  #@ string = 1
language_code: \"en-US\"  #@ string = 6
max_result_count: 5  #@ int32 = 4
";

    /// The trace, built against the descriptor set the demo ships.
    ///
    /// bobapp embeds no Places service and no `google.rpc`, so this reads the
    /// same set off disk that `--extra-descriptor-set` points at.
    fn trace() -> Vec<u8> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../grpconf/stage/googleapis.desc"
        );
        let bytes = std::fs::read(path).expect("the staged descriptor set");
        let pool = DescriptorPool::decode(&bytes[..]).expect("it parses");
        debug_trace(&pool).expect("the trace encodes")
    }

    #[test]
    fn the_trace_is_the_first_of_two_queries_and_the_real_one_wins() {
        let trace = trace();
        let text = patch_lookup(RENDERED_LOOKUP, &escape_bytes(&trace)).unwrap();
        let queries: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with("text_query: "))
            .collect();
        assert_eq!(queries.len(), 2, "the singular field must occur twice");
        assert!(queries[0].contains(FAKE_KEY), "the trace is written first");
        assert!(
            queries[1].contains("\"boulangerie\""),
            "and overwritten second"
        );

        // Both are really on the wire, in that order, and the first one's
        // bytes are the trace message itself — the anomaly is not a rendering
        // artifact.
        let bytes = render_as_bytes(text.as_bytes(), RenderOpts::default()).unwrap();
        let trace_at = find(&bytes, &trace).expect("the trace is on the wire, whole");
        let real_at = find(&bytes, b"\x0a\x0bboulangerie").expect("so is the real query");
        assert!(
            trace_at < real_at,
            "last one wins, so the trace must come first"
        );
    }

    /// What hides the trace is that it passes for a string: a `string` field
    /// holding bytes that are not UTF-8 is a request the server rejects, and
    /// an anomaly nobody gets to see.
    ///
    /// [`debug_trace`] refuses to return non-ASCII bytes, so this asserts the
    /// two things that make the guard meaningful — that the payload really is
    /// the leak, and that it really is text.
    #[test]
    fn the_trace_passes_for_a_string() {
        let trace = trace();
        let text = std::str::from_utf8(&trace).expect("valid UTF-8");
        assert!(text.contains(KEY_HEADER));
        assert!(text.contains(FAKE_KEY));
        assert!(text.contains(DETAIL_TYPE), "the Any names what it holds");
        assert!(text.is_ascii(), "ASCII, so no encoder has an opinion");
    }

    #[test]
    fn a_lookup_without_the_target_line_is_an_error() {
        let err = patch_lookup(
            "#@ prototext: protoc\nmax_result_count: 5  #@ int32 = 4\n",
            "x",
        )
        .expect_err("the patch must not silently do nothing");
        assert!(err.to_string().contains("text_query"), "{err}");
    }

    #[test]
    fn the_key_is_shaped_like_a_google_api_key() {
        assert_eq!(FAKE_KEY.len(), 39, "a Google API key is 39 characters");
        assert!(FAKE_KEY.starts_with("AIza"));
    }

    #[test]
    fn the_tail_is_cut_inside_the_last_record() {
        let whole = vec![0u8; 4096];
        assert_eq!(cut_short(&whole).len(), 4096 - 1024);
        // Nothing to cut into yet.
        let stub = vec![0u8; 512];
        assert_eq!(cut_short(&stub).len(), 512);
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn the_padded_varint_and_the_undeclared_field_reach_the_wire() {
        let text = patch_request(RENDERED).unwrap();
        let bytes = render_as_bytes(text.as_bytes(), RenderOpts::default()).unwrap();

        // Field 4, varint, value 1 (DRIVE) — spelled in five bytes where one
        // would do.  This is the anomaly, and it must be exact.
        let padded = b"\x20\x81\x80\x80\x80\x00";
        assert!(
            bytes.windows(padded.len()).any(|w| w == padded),
            "the padded varint is not on the wire: {bytes:02x?}"
        );

        // Field 99, LEN.  99 << 3 | 2 = 794 = 0x31a, so the tag is `9a 06`.
        let agent = b"\x9a\x06\x10bobapp/0.9.3-rc2";
        assert!(
            bytes.windows(agent.len()).any(|w| w == agent),
            "the undeclared field is not on the wire: {bytes:02x?}"
        );
    }

    /// The edits are additive: nothing bobapp meant to send is disturbed.
    #[test]
    fn the_rest_of_the_request_is_untouched() {
        let patched = render_as_bytes(
            patch_request(RENDERED).unwrap().as_bytes(),
            RenderOpts::default(),
        )
        .unwrap()
        .into_owned();
        let plain = render_as_bytes(RENDERED.as_bytes(), RenderOpts::default())
            .unwrap()
            .into_owned();

        // `origin` and `language_code` are byte-identical, at the same offsets;
        // only the two anomalies grew the message.
        assert_eq!(&patched[..2], &plain[..2], "origin's tag and length");
        assert!(patched.len() > plain.len());
        assert!(patched.ends_with(b"bobapp/0.9.3-rc2"));
    }

    #[test]
    fn a_request_without_the_target_line_is_an_error() {
        let err = patch_request("#@ prototext: protoc\nlanguage_code: \"en-US\"  #@ string = 10\n")
            .expect_err("the patch must not silently do nothing");
        assert!(err.to_string().contains(PADDED), "{err}");
    }
}
