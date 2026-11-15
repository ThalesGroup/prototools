// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// Generated from reproto's split_fdps._is_fdp (Python).
pub(super) const CASES: &[(&str, &[u8], bool)] = &[
    ("empty", &[], false),
    ("fdp, name only", &[10, 7, 97, 46, 112, 114, 111, 116, 111], true),
    ("fdp, name and package", &[10, 7, 97, 46, 112, 114, 111, 116, 111, 18, 1, 112], true),
    ("set of one", &[10, 9, 10, 7, 97, 46, 112, 114, 111, 116, 111], false),
    ("set of two", &[10, 9, 10, 7, 97, 46, 112, 114, 111, 116, 111, 10, 9, 10, 7, 98, 46, 112, 114, 111, 116, 111], false),
    ("field 1 not a message", &[10, 3, 97, 98, 99], true),
    ("field 1 varint", &[8, 1], false),
    ("truncated varint", &[8, 128], false),
    ("truncated length", &[10, 128], true),
    ("length past the end", &[10, 127, 0], true),
    ("huge tag, low bits field 1 LEN", &[138, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 1, 3, 97, 98, 99], true),
    ("huge length", &[10, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 1], true),
    ("tag past 64 bits only", &[128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 1], true),
    ("wire type 7", &[15, 0], false),
    ("group", &[11, 12], false),
];
