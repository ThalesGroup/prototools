# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Type names as protoc resolves them (spec 0434, invariant 0001).

Each case: compile the original files with protoc, render them, compile
the output again. The output must compile, and every type name, extendee
and method type must come back as the original's: the name written
resolves, under protoc's rule, to the type the FDP names. The positive
controls pin the spelling: googleapis' convention (spec 0434 S4).
"""

import subprocess
from pathlib import Path

import pytest
from google.protobuf import descriptor_pb2

from .conftest import REPROTO_CMD

# Each case: {path: source}, and the file whose output is checked.
CASES: dict[str, dict[str, str]] = {
    # A: the extendee shadowed by a message of the file's own package.
    "a_extendee_own_package": {
        "a/base.proto": """syntax = "proto2";
package alpha;
message Thing { extensions 1000 to max; }
""",
        "a/sub.proto": """syntax = "proto2";
package alpha.beta;
import "a/base.proto";
message Thing { optional int32 id = 1; }
extend .alpha.Thing { optional Thing sub = 1001; }
""",
    },
    # B: a partial name whose first component a nearer package captures.
    "b_partial_name_captured_by_package": {
        "b/one.proto": """syntax = "proto2";
package p.q.inner;
message Source {}
""",
        "b/two.proto": """syntax = "proto2";
package p.q.r.inner;
message Plan {}
""",
        "b/use.proto": """syntax = "proto2";
package p.q.r.use;
import "b/one.proto";
import "b/two.proto";
message Attr {
  optional .p.q.inner.Source source = 1;
  optional .p.q.r.inner.Plan plan = 2;
}
""",
    },
    # C: a full name whose first component a sub-package captures.
    "c_full_name_captured_by_subpackage": {
        "c/kind.proto": """syntax = "proto2";
package top.kinds;
enum Kind { KIND_UNKNOWN = 0; }
""",
        "c/ui.proto": """syntax = "proto2";
package x.y.top;
message Ui {}
""",
        "c/tags.proto": """syntax = "proto2";
package x.y;
import "c/kind.proto";
import "c/ui.proto";
message Tags {
  optional .top.kinds.Kind kind = 1;
  optional .x.y.top.Ui ui = 2;
}
""",
    },
    # D: a method named as its message type.
    "d_method_named_as_its_type": {
        "d/svc.proto": """syntax = "proto2";
package svc.pkg;
message DoIt {}
service S { rpc DoIt(.svc.pkg.DoIt) returns (.svc.pkg.DoIt); }
""",
    },
    # E: as A, the shadowing message in another file of the package.
    "e_extendee_shadowed_from_another_file": {
        "e/req.proto": """syntax = "proto2";
package quota;
message Request { extensions 1000 to max; }
""",
        "e/common.proto": """syntax = "proto2";
package quota.pb;
message Request {}
""",
        "e/ext.proto": """syntax = "proto2";
package quota.pb;
import "e/req.proto";
import "e/common.proto";
extend .quota.Request { optional int32 consistent = 1001; }
""",
    },
    # An extendee is looked up with LOOKUP_ALL: a field of the message
    # the extension is declared in shadows it, as does an enum value.
    "f_extendee_shadowed_by_field_and_enum_value": {
        "f/base.proto": """syntax = "proto2";
package base;
message Target { extensions 1000 to max; }
message Other { extensions 1000 to max; }
""",
        "f/ext.proto": """syntax = "proto2";
package base.ext;
import "f/base.proto";
enum Color { Other = 0; }
message Holder {
  optional int32 Target = 1;
  extend .base.Target { optional int32 by_field = 1001; }
}
extend .base.Other { optional int32 by_value = 1001; }
""",
    },
    # The same in the extendee's own package, where the short name is the
    # convention's: a field of the enclosing message captures it.
    "h_own_package_extendee_shadowed_by_field": {
        "h/ext.proto": """syntax = "proto2";
package own;
message Target { extensions 1000 to max; }
message Holder {
  optional int32 Target = 1;
  extend .own.Target { optional int32 by_field = 1001; }
}
""",
    },
    # A map value's type is resolved from the synthesized entry message.
    "g_map_value_named_as_the_entry": {
        "g/map.proto": """syntax = "proto3";
package maps;
message ThingEntry { int32 x = 1; }
message Holder {
  map<string, .maps.ThingEntry> thing = 1;
}
""",
    },
    # Positive controls: short forms kept where they resolve, other
    # packages' types in full without the dot (googleapis' convention).
    "z_controls": {
        "z/other.proto": """syntax = "proto3";
package google.type.demo;
message Date { int32 day = 1; }
""",
        "z/main.proto": """syntax = "proto3";
package google.api.demo;
import "google/protobuf/timestamp.proto";
import "z/other.proto";
message Outer {
  message Inner { int32 x = 1; }
  Inner inner = 1;
  Sibling sibling = 2;
  google.protobuf.Timestamp at = 3;
  google.type.demo.Date day = 4;
  int32 Sibling = 5;
}
message Sibling { Outer.Inner inner = 1; }
service S { rpc Get(google.protobuf.Timestamp) returns (Sibling); }
""",
    },
}


def _write(root: Path, files: dict[str, str]) -> None:
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


def _compile(root: Path, files: list[str], out: Path) -> descriptor_pb2.FileDescriptorSet:
    result = subprocess.run(
        ["protoc", f"-I{root}", "--include_imports", f"--descriptor_set_out={out}", *files],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(out.read_bytes())
    return fds


def _references(fds: descriptor_pb2.FileDescriptorSet, names: set[str]) -> dict[str, str]:
    """Every type name, extendee and method type of the files `names`,
    by where it is."""
    refs: dict[str, str] = {}

    def field(where: str, f: descriptor_pb2.FieldDescriptorProto) -> None:
        if f.type_name:
            refs[f"{where}.{f.name}:type"] = f.type_name
        if f.extendee:
            refs[f"{where}.{f.name}:extendee"] = f.extendee

    def message(where: str, m: descriptor_pb2.DescriptorProto) -> None:
        here = f"{where}.{m.name}"
        for f in [*m.field, *m.extension]:
            field(here, f)
        for n in m.nested_type:
            message(here, n)

    for fdp in fds.file:
        if fdp.name not in names:
            continue
        for m in fdp.message_type:
            message(fdp.package, m)
        for f in fdp.extension:
            field(fdp.package, f)
        for s in fdp.service:
            for meth in s.method:
                refs[f"{fdp.package}.{s.name}.{meth.name}:in"] = meth.input_type
                refs[f"{fdp.package}.{s.name}.{meth.name}:out"] = meth.output_type
    return refs


def _roundtrip(tmp: Path, files: dict[str, str]) -> Path:
    src = tmp / "src"
    _write(src, files)
    original = _compile(src, list(files), tmp / "original.pb")
    out = tmp / "out"
    result = subprocess.run(
        [*REPROTO_CMD, "-q", f"-I{tmp / 'original.pb'}", "-O", str(out),
         "--use-variant", "descriptor", "."],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    again = _compile(out, list(files), tmp / "again.pb")
    assert _references(again, set(files)) == _references(original, set(files))
    return out


@pytest.mark.parametrize("case", sorted(CASES))
def test_every_name_written_resolves_to_its_type(case: str, tmp_path: Path) -> None:
    _roundtrip(tmp_path, CASES[case])


def test_names_are_written_as_googleapis_writes_them(tmp_path: Path) -> None:
    out = _roundtrip(tmp_path, CASES["z_controls"])
    text = (out / "z/main.proto").read_text(encoding="utf-8")
    # The file's own package: short, relative to the scope; a field named
    # as a type does not shadow it for a field (LOOKUP_TYPES).
    assert "Inner inner = 1;" in text
    assert "Sibling sibling = 2;" in text
    assert "Outer.Inner inner = 1;" in text
    # Other packages, well-known types included: in full, without the dot,
    # from a google.* package too.
    assert "google.protobuf.Timestamp at = 3;" in text
    assert "google.type.demo.Date day = 4;" in text
    assert "rpc Get(google.protobuf.Timestamp) returns (Sibling);" in text


def test_a_captured_name_gets_its_leading_dot(tmp_path: Path) -> None:
    out = _roundtrip(tmp_path, CASES["c_full_name_captured_by_subpackage"])
    text = (out / "c/tags.proto").read_text(encoding="utf-8")
    assert ".top.kinds.Kind kind = 1;" in text
    assert "x.y.top.Ui ui = 2;" in text


def test_an_own_package_name_captured_gets_its_leading_dot(tmp_path: Path) -> None:
    out = _roundtrip(tmp_path, CASES["h_own_package_extendee_shadowed_by_field"])
    text = (out / "h/ext.proto").read_text(encoding="utf-8")
    assert "extend .own.Target {" in text

