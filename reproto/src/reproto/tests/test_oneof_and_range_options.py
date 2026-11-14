# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Custom options on oneofs and extension ranges round trip (spec 0424 S1).

Each test compiles a fixture with protoc, renders it with reproto, compiles
reproto's output again, and requires the same descriptor set: a custom
option reproto leaves out is missing from the second one.
"""

from __future__ import annotations

from pathlib import Path

from reproto.tests.test_roundtrip import _run_roundtrip

_HEADER = """\
syntax = "proto2";

package opts;

import "google/protobuf/descriptor.proto";
"""


def _roundtrip(tmp_path: Path, name: str, content: str) -> None:
    orig_dir = tmp_path / "orig"
    new_dir = tmp_path / "new"
    orig_dir.mkdir()
    new_dir.mkdir()
    _run_roundtrip(name, content, orig_dir, new_dir, compare_proto=False)


def test_custom_option_on_a_oneof(tmp_path: Path) -> None:
    _roundtrip(tmp_path, "oneof_option.proto", _HEADER + """
message Target {
  oneof choice {
    option (.opts.oneof_opt) = 7;

    int32 a = 1;
    string b = 2;
  }
}

extend google.protobuf.OneofOptions {
  optional int32 oneof_opt = 50001;
}
""")


def test_custom_option_on_an_extension_range(tmp_path: Path) -> None:
    _roundtrip(tmp_path, "range_option.proto", _HEADER + """
message Target {
  extensions 100 to 199 [(.opts.range_opt) = "r"];
}

extend google.protobuf.ExtensionRangeOptions {
  optional string range_opt = 50002;
}
""")
