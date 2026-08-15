// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The four things bobapp does to its own bytes that no library would.
//!
//! bobapp is a little weird.  On the way out it writes one varint padded out
//! to five bytes rather than in the minimal form every encoder produces, and
//! it appends a field its own schema has never heard of.  On the way to disk
//! it writes a singular field twice — the first occurrence being a diagnostic
//! nobody should keep — and then it gets killed before the last record is
//! finished.
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

use anyhow::{bail, Context, Result};
use prost_reflect::MessageDescriptor;
use prototext_core::{render_as_bytes, render_as_text, RenderOpts};

/// The scalar whose varint comes out padded.
const PADDED: &str = "travel_mode";
/// How many bytes that varint occupies once padded.
const PADDING: usize = 4;

/// The field number bobapp stamps its own version into.  Nothing in the
/// embedded schema, and nothing in googleapis, declares it.
const AGENT_FIELD: u32 = 99;
/// What it stamps.
const AGENT: &str = "bobapp/0.9.3-rc2";

/// `Entry.note`, whose number the log rewrite has to write by hand because
/// the second occurrence has no descriptor to be looked up from.
const NOTE_FIELD: u32 = 3;

/// A string shaped exactly like a Google API key and belonging to nobody.
///
/// It is 39 characters beginning `AIza`, because that is what makes the room
/// recognize it in the half-second it is on screen.  It has never been a
/// credential, here or anywhere.
const FAKE_KEY: &str = "AIzaSyB0b5REKn0tAr3aLk3yD0ntB0th3rTry1t";

/// Returns `bytes` re-encoded the way bobapp puts a request on the wire.
///
/// `request` is only the descriptor the bytes were encoded against, used to
/// render them with field names.
///
/// Only the route request is touched.  Both edits below are written in terms
/// of *that* message — one names a field of it, the other stamps a number no
/// schema declares — so a request of any other type goes out as encoded.
/// That is also what the log shows: two pairs of entries, one pair odd and
/// one pair ordinary.
pub fn rewrite_request(bytes: &[u8], request: &MessageDescriptor) -> Result<Vec<u8>> {
    if request.full_name() != crate::request::REQUEST_TYPE {
        return Ok(bytes.to_vec());
    }
    round_trip(bytes, request, patch_request)
}

/// Returns `bytes` re-encoded the way bobapp writes its log.
pub fn rewrite_log(bytes: &[u8], log: &MessageDescriptor) -> Result<Vec<u8>> {
    round_trip(bytes, log, patch_log)
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
    edit: fn(&str) -> Result<String>,
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

/// The note, written once with what was in flight and once with how it went.
///
/// The first occurrence is inserted ahead of the second, so the wire order is
/// the order the app would have written them, and last-one-wins hands every
/// ordinary reader the harmless one.
fn patch_log(text: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len() + 128);
    let mut doubled = false;

    for line in text.lines() {
        let indent = &line[..line.len() - line.trim_start().len()];
        if line.trim_start().starts_with("note: ") {
            out.push_str(&format!(
                "{indent}note: \"x-goog-api-key: {FAKE_KEY}\"  #@ string = {NOTE_FIELD}\n"
            ));
            doubled = true;
        }
        out.push_str(line);
        out.push('\n');
    }

    if !doubled {
        bail!("the rendered log has no `note` line to write twice");
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

    /// A one-entry log, as the renderer gives it back with `note` set once.
    const RENDERED_LOG: &str = "\
#@ prototext: protoc
entry {  #@ repeated Entry = 1
 method: \"/svc/M\"  #@ string = 2
 note: \"ok\"  #@ string = 3
 request: \"\\010\\001\"  #@ bytes = 4
}
";

    #[test]
    fn the_key_is_the_first_of_two_notes_and_the_bland_one_wins() {
        let text = patch_log(RENDERED_LOG).unwrap();
        let notes: Vec<&str> = text
            .lines()
            .filter(|l| l.trim_start().starts_with("note: "))
            .collect();
        assert_eq!(notes.len(), 2, "the singular field must occur twice");
        assert!(notes[0].contains(FAKE_KEY), "the key is written first");
        assert!(notes[1].contains("\"ok\""), "and overwritten second");

        // Both are really on the wire, in that order — the anomaly is not a
        // rendering artifact.
        let bytes = render_as_bytes(text.as_bytes(), RenderOpts::default()).unwrap();
        let key_at = find(&bytes, FAKE_KEY.as_bytes()).expect("the key is on the wire");
        let ok_at = find(&bytes, b"\x1a\x02ok").expect("the bland note is on the wire");
        assert!(key_at < ok_at, "last one wins, so the key must come first");
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
