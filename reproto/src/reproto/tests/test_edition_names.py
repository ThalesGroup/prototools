# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""An edition reproto can't name is a W6, the file not written, never
`edition = "UNKNOWN";` (spec 0467 S1); an older Python protobuf than
reproto's descriptor.proto is said (S2). The first runs on reproto and on
protoback alike (spec 0457 S2)."""

import os
import subprocess
from pathlib import Path

import pytest
from google.protobuf import descriptor_pb2

from .conftest import REPROTO_CMD


def _with_edition(f: descriptor_pb2.FileDescriptorProto, edition: int) -> bytes:
    """`f` with `edition` (field 14, a varint), which the closed enum would
    refuse to hold."""
    out = bytearray(f.SerializeToString())
    out.append(14 << 3)
    while edition >= 0x80:
        out.append((edition & 0x7F) | 0x80)
        edition >>= 7
    out.append(edition)
    return bytes(out)


@pytest.mark.skipif(
    "reproto.tests.via_text" in REPROTO_CMD,
    reason="the text format cannot hold an edition the closed Edition enum lacks: "
           "read back, it is refused (W15), not a W6",
)
def test_an_unnamed_edition_is_a_w6(tmp_path: Path) -> None:
    named = descriptor_pb2.FileDescriptorProto(name="a/ok.proto", package="a",
                                               syntax="editions",
                                               edition=descriptor_pb2.EDITION_2024)
    unnamed = descriptor_pb2.FileDescriptorProto(name="a/b.proto", package="a",
                                                 syntax="editions")
    unnamed.message_type.add(name="M")
    inp = tmp_path / "in"
    inp.mkdir()
    (inp / "ok.pb").write_bytes(named.SerializeToString())
    (inp / "b.pb").write_bytes(_with_edition(unnamed, 1003))
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, f"-I{inp}", "-O", str(tmp_path / "out"), "--detailed-warnings",
         "ok.pb", "b.pb"],
        capture_output=True, text=True, env=env,
    )
    assert "[W6]" in result.stderr and "edition 1003" in result.stderr, result.stderr
    assert not (tmp_path / "out" / "a" / "b.proto").exists()
    assert (tmp_path / "out" / "a" / "ok.proto").exists(), "the other file is written"
    for p in (tmp_path / "out").rglob("*.proto"):
        assert "UNKNOWN" not in p.read_text(encoding="utf-8"), p


def test_an_older_protobuf_is_said(monkeypatch: pytest.MonkeyPatch,
                                   caplog: pytest.LogCaptureFixture) -> None:
    from reproto import protobuf_check
    # Whatever the protobuf imported lacks already (none with reproto's own
    # release; EDITION_2026 with protobuf 34), plus one it can't know.
    before = protobuf_check.missing_editions()
    builtin = protobuf_check._builtin_editions()
    monkeypatch.setattr(protobuf_check, "_builtin_editions",
                        lambda: {**builtin, "EDITION_2027": 1003})
    assert protobuf_check.missing_editions() == sorted([*before, "EDITION_2027"])
    protobuf_check.check()
    said = caplog.text
    assert "[W37]" in said and "EDITION_2027" in said and "PYTHONPATH" in said, said
