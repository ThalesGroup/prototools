# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The graph dump, `--dump-graph FILE` (spec 0428 S1–S6).

What phases 1 to 6 leave, written as text, after which the run stops: the
instrument comparing protoback with reproto.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from reproto.tests.conftest import REPROTO_CMD

_OPTS = """\
syntax = "proto2";

package dg;

import "google/protobuf/descriptor.proto";

extend google.protobuf.MessageOptions {
  optional string tag = 50001;
}
"""

_MAIN = """\
syntax = "proto2";

package dg;

import "opts.proto";
import "gone.proto";

message Holder {
  option (dg.tag) = "t";
  optional Inner inner = 1;
  optional gone.G lost = 2;

  message Inner {
    optional Kind kind = 1;
  }
}

enum Kind {
  KIND_A = 0;
}

service Svc {
  rpc Get(Holder) returns (Holder);
}
"""

_GONE = 'syntax = "proto2";\npackage gone;\nmessage G {}\n'

_SECTIONS = ["[files]", "[pool]", "[context]", "[nodes]", "[seeds]", "[warnings]"]


def _set(src: Path) -> Path:
    """main.proto and opts.proto, without gone.proto: a missing import."""
    for name, content in {"opts.proto": _OPTS, "main.proto": _MAIN,
                          "gone.proto": _GONE}.items():
        (src / name).write_text(content, encoding="utf-8")
    pb = src / "set.pb"
    full = src / "full.pb"
    result = subprocess.run(
        ["protoc", "--include_imports", f"--descriptor_set_out={full}",
         f"-I{src}", str(src / "main.proto")],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    from google.protobuf.descriptor_pb2 import FileDescriptorSet
    fds = FileDescriptorSet()
    fds.ParseFromString(full.read_bytes())
    kept = FileDescriptorSet()
    kept.file.extend(f for f in fds.file if f.name != "gone.proto")
    pb.write_bytes(kept.SerializeToString())
    return pb


def _dump(tmp: Path, pb: Path, seed: str, *args: str) -> tuple[str, Path]:
    out = tmp / f"out-{seed}"
    dump = tmp / f"graph-{seed}.txt"
    env = {**os.environ, "PYTHONHASHSEED": seed}
    env.pop("REPROTO_VARIANT", None)
    result = subprocess.run(
        [*REPROTO_CMD, "--use-variant", "descriptor", f"--proto-out={out}",
         f"--dump-graph={dump}", *args, str(pb)],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    return dump.read_text(encoding="utf-8"), out


def test_every_section_in_order_and_nothing_rendered(tmp_path: Path) -> None:
    src = tmp_path / "src"
    src.mkdir()
    text, out = _dump(tmp_path, _set(src), "1")
    lines = text.splitlines()
    headers = [line for line in lines if line.startswith("[")]
    assert headers == _SECTIONS
    # The run stops after phase 6: nothing rendered.
    assert not out.exists() or not any(out.rglob("*.proto"))
    # The missing import: a ref in [files], stripped from main.proto, warned.
    assert any(line.startswith("gone.proto | ref") for line in lines)
    main = next(line for line in lines if line.startswith("main.proto | file"))
    assert "stripped imports [gone.proto]" in main
    assert "w1 | gone.proto" in text
    # Nodes of each kind, with their flags and links.
    assert "desc:.dg.Holder.Inner" in text
    assert "serv:.dg.Svc" in text


def test_two_hash_seeds_give_the_same_dump(tmp_path: Path) -> None:
    src = tmp_path / "src"
    src.mkdir()
    pb = _set(src)
    first, _ = _dump(tmp_path, pb, "1")
    second, _ = _dump(tmp_path, pb, "2")
    assert first == second


def test_seeds_and_pruning_show(tmp_path: Path) -> None:
    src = tmp_path / "src"
    src.mkdir()
    text, _ = _dump(tmp_path, _set(src), "1", "--seed", "desc:.dg.Holder.Inner",
                    "--prune", "serv:.dg.Svc")
    seeds = text.split("[seeds]\n", 1)[1].split("[warnings]", 1)[0]
    assert seeds.splitlines() == ["desc:.dg.Holder.Inner"]
    # The pattern as reproto keeps it, and the node it pruned.
    context = text.split("[context]\n", 1)[1].split("[nodes]", 1)[0]
    assert "pruned fqdns | [serv:dg/Svc]" in context
    svc = next(line for line in text.splitlines()
               if line.startswith("ReServiceDescriptorProto | serv:.dg.Svc |"))
    assert svc.split(" | ")[3].startswith("P")
