// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The schema-blind shape vocabulary (spec 0342).

/// What the renderer decided a record is, when no schema said.
///
/// A shape opens an annotation in the slot a declared type name would
/// otherwise fill — `#@ varint` where a known field says
/// `#@ int64 = 1` — so consumers color it as a type (spec 0341).
///
/// Five of the seven are wire types. Three of them — [`Bytes`],
/// [`String`] and [`Message`] — are the readings wire type 2 admits,
/// listed here in the order the unknown-LEN cascade tries them.
///
/// This enum is the vocabulary. It exists as a type rather than as a
/// list of names because a list can be left behind: before spec 0342
/// the names were literals at eight call sites, and adding one was a
/// new literal that no build could tie to the four downstream copies
/// (spec 0341's defect). Emitting a shape now means naming a variant,
/// and every consumer that enumerates them reads [`Shape::ALL`].
///
/// [`Bytes`]: Shape::Bytes
/// [`String`]: Shape::String
/// [`Message`]: Shape::Message
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// Wire type 0.
    Varint,
    /// Wire type 1.
    Fixed64,
    /// Wire type 5.
    Fixed32,
    /// Wire type 2, payload kept opaque.
    Bytes,
    /// Wire type 2, payload is valid UTF-8.
    String,
    /// Wire type 2, payload parses as a message.
    Message,
    /// Wire types 3 and 4.
    Group,
}

impl Shape {
    /// Every shape, for the consumers that must cover the vocabulary:
    /// protolens's `annotation::vocabulary`, and through it the drift
    /// test that pins `highlights.scm`.
    pub const ALL: [Shape; 7] = [
        Shape::Varint,
        Shape::Fixed64,
        Shape::Fixed32,
        Shape::Bytes,
        Shape::String,
        Shape::Message,
        Shape::Group,
    ];

    /// The token an annotation carries. Lowercase, and identical to the
    /// proto type name where one exists — a `bytes` shape and a
    /// declared `bytes` field are spelled alike on purpose: they fill
    /// the same slot and say the same thing about the payload.
    pub const fn as_str(self) -> &'static str {
        match self {
            Shape::Varint => "varint",
            Shape::Fixed64 => "fixed64",
            Shape::Fixed32 => "fixed32",
            Shape::Bytes => "bytes",
            Shape::String => "string",
            Shape::Message => "message",
            Shape::Group => "group",
        }
    }
}
