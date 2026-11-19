# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The renaming --target protoc derives, written (specs 0440, 0447 S3).

The vendor descriptor's definitions renamed to `google.protobuf`: names are
written as spec 0434 writes them, in the output namespace (0440 S3), in the
importers and in the descriptor itself, whose package is the stock one in
its file alone; two symbols renamed onto one name stop the run (0440 S2).
"""

import os
import subprocess
from pathlib import Path

from .conftest import REPROTO_CMD, vendor_descriptor

USE = '''name: "app/use.proto" package: "app" dependency: "vendor/proto/descriptor.proto"
extension { name: "tag" number: 50201 label: LABEL_OPTIONAL type: TYPE_STRING
  extendee: ".vendor.FieldOptions" }
message_type { name: "M"
  field { name: "mode" number: 1 label: LABEL_OPTIONAL type: TYPE_ENUM
    type_name: ".vendor.FileOptions.OptimizeMode" options { 50201: "t" } } }
message_type { name: "N"
  nested_type { name: "google" }
  field { name: "mode" number: 1 label: LABEL_OPTIONAL type: TYPE_ENUM
    type_name: ".vendor.FileOptions.OptimizeMode" } }
'''
# A file of protoc's own package, beside the vendor descriptor.
EMPTY = '''name: "google/protobuf/empty.proto" package: "google.protobuf"
message_type { name: "Empty" }
'''


def _run(tmp_path: Path, *flags: str, top: str = "",
         files: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    vendor_descriptor(tmp_path, top=top)
    inp = tmp_path / "in"
    for name, text in ({"use.textpb": USE} if files is None else files).items():
        (inp / name).write_text(text, encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    return subprocess.run(
        [*REPROTO_CMD, "--target", "protoc", *flags, f"-I{inp}", "-O", str(tmp_path / "out")],
        capture_output=True, text=True, env=env,
    )


def test_renamed_names_are_written_in_the_output_namespace(tmp_path: Path) -> None:
    result = _run(tmp_path)
    assert result.returncode == 0, result.stderr
    out = tmp_path / "out"
    use = (out / "app" / "use.proto").read_text(encoding="utf-8")
    # From another package: in full, no dot.
    assert "optional google.protobuf.FileOptions.OptimizeMode mode = 1" in use, use
    # Where `google` is captured (N.google): the dot.
    assert "optional .google.protobuf.FileOptions.OptimizeMode mode = 1;" in use, use
    assert "extend google.protobuf.FieldOptions {" in use, use
    # Within the descriptor, its own output package: short.
    desc = (out / "google" / "protobuf" / "descriptor.proto").read_text(encoding="utf-8")
    assert "optional OptimizeMode optimize_for = 9" in desc, desc
    assert "optional FeatureSet features = 50;" in desc, desc
    compiled = subprocess.run(["protoc", f"-I{out}", "-o", os.devnull, "app/use.proto"],
                              capture_output=True, text=True)
    assert compiled.returncode == 0, compiled.stderr


def test_two_symbols_renamed_onto_one_name_stop_the_run(tmp_path: Path) -> None:
    result = _run(tmp_path, top="message Empty { optional int32 marker = 1; }\n",
                  files={"use.textpb": USE, "empty.textpb": EMPTY})
    assert result.returncode == 1, result.stderr
    assert ("'google/protobuf/empty.proto' and 'vendor/proto/descriptor.proto' both define "
            "'google.protobuf.Empty' once rewritten; prune one (-p)") in result.stderr, result.stderr
    # The outermost name only: not its field.
    assert "Empty.marker" not in result.stderr
    assert not (tmp_path / "out" / "app" / "use.proto").exists()
