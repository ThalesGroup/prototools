# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Message-valued custom options round trip (spec 0424 S1).

A custom option whose type is a message is printed as an aggregate,
`option (.pkg.opt) = { ... };`. Each test compiles a fixture with protoc,
renders it with reproto, compiles reproto's output again, and requires the
same descriptor set: the aggregate must say what the options bytes say.
"""

from __future__ import annotations

from pathlib import Path

from reproto.tests.test_roundtrip import _run_roundtrip

_HEADER = """\
syntax = "proto2";

package agg;

import "google/protobuf/descriptor.proto";
"""


def _roundtrip(tmp_path: Path, name: str, content: str) -> None:
    orig_dir = tmp_path / "orig"
    new_dir = tmp_path / "new"
    orig_dir.mkdir()
    new_dir.mkdir()
    _run_roundtrip(name, content, orig_dir, new_dir, compare_proto=False)


def test_aggregate_with_a_map_field(tmp_path: Path) -> None:
    """A map field inside the value: one entry per key, `key` and `value`."""
    _roundtrip(tmp_path, "aggregate_map.proto", _HEADER + """
message Target {
  option (.agg.opt) = { counts { key: "b" value: 2 } counts { key: "a" value: 1 } counts { key: "c" value: 3 } };
}

message Agg {
  map<string, int32> counts = 1;
}

extend google.protobuf.MessageOptions {
  optional Agg opt = 50001;
}
""")


def test_aggregate_with_an_extension(tmp_path: Path) -> None:
    """An extension inside the value: named `[pkg.ext]`."""
    _roundtrip(tmp_path, "aggregate_extension.proto", _HEADER + """
message Target {
  option (.agg.opt) = { n: 2 [agg.tag]: "t" };
}

message Agg {
  optional int32 n = 1;

  extensions 100 to 199;
}

extend Agg {
  optional string tag = 100;
}

extend google.protobuf.MessageOptions {
  optional Agg opt = 50001;
}
""")


def test_aggregate_with_a_group(tmp_path: Path) -> None:
    """A group inside the value: named by its type, its fields inside."""
    _roundtrip(tmp_path, "aggregate_group.proto", _HEADER + """
message Target {
  option (.agg.opt) = { G { x: 5 } };
}

message Agg {
  optional group G = 1 {
    optional int32 x = 2;
  }
}

extend google.protobuf.MessageOptions {
  optional Agg opt = 50001;
}
""")
