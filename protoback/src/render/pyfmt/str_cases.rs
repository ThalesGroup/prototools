// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// Generated from Python 3.13: (s, repr(s)).
pub(super) const STR_REPR: &[(&str, &str)] = &[
    ("", "''"),
    ("a", "'a'"),
    ("it's", "\"it's\""),
    ("say \"hi\"", "'say \"hi\"'"),
    ("both ' and \"", "'both \\' and \"'"),
    ("back\\slash", "'back\\\\slash'"),
    ("tab\u{9}here", "'tab\\there'"),
    ("nl\u{a}x", "'nl\\nx'"),
    ("cr\u{d}", "'cr\\r'"),
    ("\u{0}\u{1}\u{1f}\u{7f}", "'\\x00\\x01\\x1f\\x7f'"),
    ("\u{e9}", "'\u{e9}'"),
    ("\u{65e5}\u{672c}\u{8a9e}", "'\u{65e5}\u{672c}\u{8a9e}'"),
    ("\u{1f642}", "'\u{1f642}'"),
    ("\u{80}\u{9f}\u{a0}\u{ad}", "'\\x80\\x9f\\xa0\\xad'"),
    ("\u{200b}\u{2028}\u{2029}\u{feff}", "'\\u200b\\u2028\\u2029\\ufeff'"),
    ("\u{1f600}\u{e0001}", "'\u{1f600}\\U000e0001'"),
    ("x", "'x'"),
    ("\u{fc}n\u{ef}c\u{f6}d\u{e9}", "'\u{fc}n\u{ef}c\u{f6}d\u{e9}'"),
    ("\u{378}", "'\\u0378'"),
    ("\u{10ffff}", "'\\U0010ffff'"),
    ("mixed 'quotes' and \"more\"", "'mixed \\'quotes\\' and \"more\"'"),
    ("\u{a0}nbsp", "'\\xa0nbsp'"),
    ("\u{3000}ideo", "'\\u3000ideo'"),
];
