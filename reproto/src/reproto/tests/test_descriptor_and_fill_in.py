# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Spec 0447 S1 and S2: the descriptor the input was compiled against,
inferred; built-in copies filling in what the input lacks.

A stock input needs no option: a missing descriptor.proto or well-known
file is filled in, silently. A vendor input's descriptor is inferred from
what its custom options extend; extendees of two vendors are an error,
which --descriptor settles.
"""

import os
import subprocess
from pathlib import Path

from .conftest import REPROTO_CMD, needs_release_protobuf, vendor_descriptor

USER = '''name: "b.proto" package: "b" dependency: "google/protobuf/timestamp.proto"
message_type { name: "B" field { name: "t" number: 1 label: LABEL_OPTIONAL
  type: TYPE_MESSAGE type_name: ".google.protobuf.Timestamp" } }
'''
CUSTOM = '''name: "c.proto" package: "c" dependency: "vendor/proto/descriptor.proto"
extension { name: "tag" number: 50000 label: LABEL_OPTIONAL type: TYPE_INT32
  extendee: ".vendor.FieldOptions" }
message_type { name: "M" field { name: "f" number: 1 label: LABEL_OPTIONAL
  type: TYPE_INT32 options { 50000: 7 } } }
'''
OTHER = '''name: "d.proto" package: "d" extension { name: "x" number: 50001
  label: LABEL_OPTIONAL type: TYPE_INT32 extendee: ".other.FileOptions" }
'''


def _run(tmp_path: Path, files: dict[str, str], *flags: str) -> subprocess.CompletedProcess[str]:
    inp = tmp_path / "in"
    inp.mkdir(exist_ok=True)
    for name, text in files.items():
        (inp / name).write_text(text, encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    return subprocess.run([*REPROTO_CMD, *flags, f"-I{inp}", "-O", str(tmp_path / "out")],
                          capture_output=True, text=True, env=env)


@needs_release_protobuf
def test_a_stock_input_needs_no_option(tmp_path: Path) -> None:
    r = _run(tmp_path, {"b.textpb": USER})
    assert r.returncode == 0, r.stderr
    assert "Warning" not in r.stderr and "fallback" not in r.stderr, r.stderr
    b = (tmp_path / "out" / "b.proto").read_text(encoding="utf-8")
    assert "google.protobuf.Timestamp t = 1" in b, b
    assert (tmp_path / "out" / "google/protobuf/timestamp.proto").exists()


def test_no_builtin_fills_in_nothing(tmp_path: Path) -> None:
    r = _run(tmp_path, {"b.textpb": USER}, "--no-builtin")
    assert r.returncode == 1, r.stderr
    assert "descriptor.proto not found in the input set" in r.stderr, r.stderr


def test_use_variant_is_deprecated(tmp_path: Path) -> None:
    r = _run(tmp_path, {"b.textpb": USER}, "--use-variant", "descriptor")
    assert "Warning [W18]: --use-variant is deprecated; use --builtin" in r.stderr, r.stderr


def test_a_vendor_descriptor_is_inferred(tmp_path: Path) -> None:
    vendor_descriptor(tmp_path)
    r = _run(tmp_path, {"c.textpb": CUSTOM})
    assert r.returncode == 0, r.stderr
    c = (tmp_path / "out" / "c.proto").read_text(encoding="utf-8")
    assert "[(c.tag) = 7]" in c, c
    assert "W22" not in r.stderr, r.stderr


def test_descriptor_overrides_the_inference(tmp_path: Path) -> None:
    vendor_descriptor(tmp_path)
    r = _run(tmp_path, {"c.textpb": CUSTOM}, "--descriptor", "google/protobuf/descriptor.proto")
    assert r.returncode == 0, r.stderr
    # Read against protoc's descriptor, the vendor's option is an orphan.
    assert "W22" in r.stderr, r.stderr


def test_two_vendors_are_an_error(tmp_path: Path) -> None:
    vendor_descriptor(tmp_path)
    r = _run(tmp_path, {"c.textpb": CUSTOM, "d.textpb": OTHER})
    assert r.returncode == 1, r.stderr
    assert ("extend the options of several descriptors: other (as in d.proto), "
            "vendor (as in c.proto); name the one to use with --descriptor") in r.stderr, r.stderr
