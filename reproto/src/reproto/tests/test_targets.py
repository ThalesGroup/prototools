# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Spec 0447 S3 and S4: the targets, and the import rule.

On a vendor fixture (protoc's descriptor as package `vendor`, with a
FieldOptions field only it declares), --target protoc writes the vendor
descriptor as google/protobuf/descriptor.proto, renames its definitions
alone, and imports it where a file sets that field: stock protoc then
compiles each output file on its own. --target keep adds nothing, but the
import under --import-descriptor.
"""

import os
import subprocess
from pathlib import Path

from .conftest import REPROTO_CMD, STOCK_INCLUDE, vendor_descriptor

VENDOR_ONLY = "  optional bool vendor_only = 90;\n"
# A custom option, and a field of a vendor descriptor's type.
CUSTOM = '''name: "c.proto" package: "c" dependency: "vendor/proto/descriptor.proto"
extension { name: "tag" number: 50000 label: LABEL_OPTIONAL type: TYPE_INT32
  extendee: ".vendor.FieldOptions" }
message_type { name: "M"
  field { name: "f" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32
    options { deprecated: true 50000: 7 } }
  field { name: "m" number: 2 label: LABEL_OPTIONAL type: TYPE_ENUM
    type_name: ".vendor.FileOptions.OptimizeMode" } }
'''
# The vendor-only built-in field, without importing the descriptor.
VENDOR_FIELD = '''name: "d.proto" package: "d" message_type { name: "D"
  field { name: "g" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32
    options { vendor_only: true } } }
'''
# A stock built-in field only: no import needed.
STOCK_FIELD = '''name: "e.proto" package: "e" message_type { name: "E"
  field { name: "h" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32
    options { deprecated: true } } }
'''
# Another file of package vendor, a name the descriptor's prefixes.
SIBLING = '''name: "vendor/other.proto" package: "vendor"
message_type { name: "FileOptionsX" field { name: "k" number: 1
  label: LABEL_OPTIONAL type: TYPE_INT32 } }
'''


def _run(tmp_path: Path, *flags: str, files: dict[str, str] | None = None,
         stock_too: bool = False) -> subprocess.CompletedProcess[str]:
    vendor_descriptor(tmp_path, VENDOR_ONLY)
    inp = tmp_path / "in"
    texts = {"c.textpb": CUSTOM, "d.textpb": VENDOR_FIELD, "e.textpb": STOCK_FIELD,
             "other.textpb": SIBLING} if files is None else files
    for name, text in texts.items():
        (inp / name).write_text(text, encoding="utf-8")
    if stock_too:
        subprocess.run(["protoc", f"-I{STOCK_INCLUDE}",
                        f"--descriptor_set_out={inp / 'stock.pb'}",
                        "google/protobuf/descriptor.proto"], check=True, capture_output=True)
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    return subprocess.run([*REPROTO_CMD, *flags, f"-I{inp}", "-O", str(tmp_path / "out")],
                          capture_output=True, text=True, env=env)


def _read(tmp_path: Path, name: str) -> str:
    return (tmp_path / "out" / name).read_text(encoding="utf-8")


def _imports(text: str) -> list[str]:
    return [line for line in text.splitlines() if line.startswith("import ")]


def test_protoc_output_compiles_file_by_file_with_stock_protoc(tmp_path: Path) -> None:
    r = _run(tmp_path, "--target", "protoc")
    assert r.returncode == 0, r.stderr
    out = tmp_path / "out"
    files = sorted(str(p.relative_to(out)) for p in out.rglob("*.proto"))
    assert "google/protobuf/descriptor.proto" in files, files
    assert "vendor/proto/descriptor.proto" not in files, files
    for f in files:
        c = subprocess.run(["protoc", f"-I{out}", "--descriptor_set_out=/dev/null", f],
                           capture_output=True, text=True)
        assert c.returncode == 0, (f, c.stderr)


def test_protoc_renames_the_descriptor_and_its_references(tmp_path: Path) -> None:
    r = _run(tmp_path, "--target", "protoc")
    assert r.returncode == 0, r.stderr
    assert "package google.protobuf;" in _read(tmp_path, "google/protobuf/descriptor.proto")
    c = _read(tmp_path, "c.proto")
    assert 'import "google/protobuf/descriptor.proto";' in c, c
    assert "extend google.protobuf.FieldOptions" in c, c
    assert "google.protobuf.FileOptions.OptimizeMode m = 2" in c, c


def test_protoc_renames_the_descriptor_definitions_alone(tmp_path: Path) -> None:
    r = _run(tmp_path, "--target", "protoc")
    assert r.returncode == 0, r.stderr
    other = _read(tmp_path, "vendor/other.proto")
    assert "package vendor;" in other and "message FileOptionsX" in other, other


def test_import_rule_only_where_needed(tmp_path: Path) -> None:
    r = _run(tmp_path, "--target", "protoc")
    assert r.returncode == 0, r.stderr
    assert _imports(_read(tmp_path, "d.proto")) == [
        'import "google/protobuf/descriptor.proto";']
    assert _imports(_read(tmp_path, "e.proto")) == []


def test_keep_adds_nothing(tmp_path: Path) -> None:
    r = _run(tmp_path)
    assert r.returncode == 0, r.stderr
    assert _imports(_read(tmp_path, "d.proto")) == []
    assert "package vendor;" in _read(tmp_path, "vendor/proto/descriptor.proto")
    assert "extend vendor.FieldOptions" in _read(tmp_path, "c.proto")


def test_keep_imports_the_descriptor_on_request(tmp_path: Path) -> None:
    r = _run(tmp_path, "--import-descriptor")
    assert r.returncode == 0, r.stderr
    assert _imports(_read(tmp_path, "d.proto")) == ['import "vendor/proto/descriptor.proto";']
    assert _imports(_read(tmp_path, "e.proto")) == []


def test_the_input_stock_descriptor_is_dropped_with_w27(tmp_path: Path) -> None:
    r = _run(tmp_path, "--target", "protoc", stock_too=True)
    assert r.returncode == 0, r.stderr
    assert ("Warning [W27]: 'google/protobuf/descriptor.proto' dropped: "
            "'vendor/proto/descriptor.proto', the descriptor in use, is written as "
            "'google/protobuf/descriptor.proto' (--target protoc)") in r.stderr, r.stderr
    assert "package google.protobuf;" in _read(tmp_path, "google/protobuf/descriptor.proto")
    assert "vendor_only" in _read(tmp_path, "google/protobuf/descriptor.proto")


def test_protoc_with_a_variant_is_a_usage_error(tmp_path: Path) -> None:
    r = _run(tmp_path, "--target", "protoc", "--keep-descriptor-path")
    assert r.returncode == 2, r.stderr
    assert "--target protoc derives its renaming from the descriptor" in r.stderr, r.stderr
