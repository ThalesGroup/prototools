// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// Generated from Python 3.13: (s, codecs.decode(s, 'unicode-escape')
// .encode('latin1')), a bytes field's default as protoc stores it.
pub(super) const BYTES_DEFAULT: &[(&str, &[u8])] = &[
    ("", &[]),
    ("abc", &[97, 98, 99]),
    ("\\000\\001\\377", &[0, 1, 255]),
    ("\\x41\\x7f", &[65, 127]),
    ("a\\nb\\tc\\\\d", &[97, 10, 98, 9, 99, 92, 100]),
    ("\\'\\\"", &[39, 34]),
    ("\\a\\b\\f\\v\\r", &[7, 8, 12, 11, 13]),
    ("\\101\\102", &[65, 66]),
    ("q\\001r", &[113, 1, 114]),
    ("\\0\\00\\000", &[0, 0, 0]),
    ("\\7", &[7]),
    ("\\8", &[92, 56]),
    ("\\q", &[92, 113]),
    ("\\u0041", &[65]),
    ("\\U00000042", &[66]),
    ("\\\u{a}", &[]),
    ("x\\u00e9y", &[120, 233, 121]),
    ("\u{e9}", &[195, 169]),
    ("a\\x41\u{e9}", &[97, 65, 195, 169]),
];

// The same, for which Python raises (a ValueError: C5 in reproto).
pub(super) const BYTES_DEFAULT_FAILS: &[&str] = &[
    "\\777",
    "\\400",
    "\\x4",
    "\\x",
    "trailing\\",
    "\\u12",
    "\\u0100",
];
