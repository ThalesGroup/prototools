// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The default comparator: text, line by line (design.md).

use memchr::{memchr, memmem};

use crate::{Comparator, Containment};

/// A version is inside another when each of its lines matches a line of
/// the other, in order, a line matching when its characters are a
/// subsequence of the other line's. So B is later than A when it can be
/// made from A by inserting lines and inserting characters within lines;
/// no match crosses a newline.
///
/// Lines are split on `\n` alone; a final `\n` ends the last line and adds
/// no empty one. Characters are UTF-8 sequences, matched whole; a byte that
/// starts no complete sequence is matched as far as the needle goes.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinesChars;

impl Comparator for LinesChars {
    fn compare(&self, a: &[u8], b: &[u8]) -> Containment {
        if a == b {
            return Containment::Equal;
        }
        match (lines_inside(a, b), lines_inside(b, a)) {
            // Only possible for different bytes when each line of one
            // matches a line of the other both ways: read as equal.
            (true, true) => Containment::Equal,
            (true, false) => Containment::FirstInSecond,
            (false, true) => Containment::SecondInFirst,
            (false, false) => Containment::Incomparable,
        }
    }
}

/// The line at `pos`, and where the next one starts.
fn next_line(buf: &[u8], pos: usize) -> (&[u8], usize) {
    match memchr(b'\n', &buf[pos..]) {
        Some(i) => (&buf[pos..pos + i], pos + i + 1),
        None => (&buf[pos..], buf.len()),
    }
}

/// Whether every line of `needle` matches a line of `haystack`, in order.
/// Both cursors only move forward.
fn lines_inside(needle: &[u8], haystack: &[u8]) -> bool {
    let (mut ni, mut hi) = (0, 0);
    while ni < needle.len() {
        let (line, next) = next_line(needle, ni);
        loop {
            if hi >= haystack.len() {
                return false;
            }
            let (candidate, after) = next_line(haystack, hi);
            hi = after;
            if chars_inside(line, candidate) {
                break;
            }
        }
        ni = next;
    }
    true
}

/// Whether `needle`'s characters are a subsequence of `haystack`'s: each
/// matched at its earliest place after the previous one.
fn chars_inside(needle: &[u8], haystack: &[u8]) -> bool {
    if haystack.len() < needle.len() {
        return false;
    }
    if needle.len() == haystack.len() {
        return needle == haystack;
    }
    let (mut ni, mut hi) = (0, 0);
    while ni < needle.len() {
        let lead = needle[ni];
        let len = match lead {
            0x00..=0x7F => 1,
            0x80..=0xDF => 2,
            0xE0..=0xEF => 3,
            _ => 4,
        }
        .min(needle.len() - ni);
        let found = if len == 1 {
            memchr(lead, &haystack[hi..])
        } else {
            memmem::find(&haystack[hi..], &needle[ni..ni + len])
        };
        match found {
            Some(i) => {
                hi += i + len;
                ni += len;
            }
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use Containment::*;

    fn cmp(a: &str, b: &str) -> Containment {
        LinesChars.compare(a.as_bytes(), b.as_bytes())
    }

    #[test]
    fn the_four_outcomes() {
        assert_eq!(cmp("a\nb", "a\nb"), Equal);
        assert_eq!(cmp("a\nc", "a\nb\nc"), FirstInSecond);
        assert_eq!(cmp("a\nb\nc", "a\nc"), SecondInFirst);
        assert_eq!(cmp("a\nb", "a\nc"), Incomparable);
        assert_eq!(cmp("", "x"), FirstInSecond);
    }

    #[test]
    fn characters_inserted_within_a_line() {
        assert_eq!(
            cmp("message M {}", "message Msg { int32 x = 1; }"),
            FirstInSecond
        );
        assert_eq!(cmp("ab\ncd", "aXb\ncYd\ne"), FirstInSecond);
    }

    #[test]
    fn no_match_across_a_newline() {
        // "ab" is a subsequence of "a\nb" as bytes, not as lines.
        assert_eq!(cmp("ab", "a\nb"), Incomparable);
    }

    #[test]
    fn a_final_newline_adds_no_line() {
        assert_eq!(cmp("a\n", "a\nb"), FirstInSecond);
        assert_eq!(cmp("a", "a\n"), Equal);
    }

    #[test]
    fn characters_are_matched_whole() {
        // é is C3 A9; "e" + A9 elsewhere must not assemble it.
        assert_eq!(cmp("é", "xéy"), FirstInSecond);
        assert_eq!(
            LinesChars.compare("é".as_bytes(), b"\xC3x\xA9"),
            Incomparable
        );
        // A truncated sequence in the needle doesn't panic.
        assert_eq!(LinesChars.compare(b"\xC3", b"a\xC3b"), FirstInSecond);
    }
}
