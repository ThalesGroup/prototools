# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""The inputs of test_phases.py that its own code builds with Python's
protobuf (4da0f2d6), copied verbatim but for the output paths:
  OUT/self_dep.pb    test_P1_self_dependency_stripped
  OUT/circ_a.pb, OUT/circ_b.pb    test_P5_circular_dependency
  OUT/multi.textpb   test_M2_multi_fdp_text_fds (text_format's printing)
Run with a Python that has protobuf, protoc on PATH, FIXTURES the
directory of the suite's fixtures (protoback/tests/fixtures).

Usage: python3 -I -B test_phases.py FIXTURES OUT
"""
import subprocess
import sys
from pathlib import Path

from google.protobuf import text_format
from google.protobuf.descriptor_pb2 import FileDescriptorProto, FileDescriptorSet

FIXTURES_DIR, out = Path(sys.argv[1]), Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)


def compile_proto(out_dir, *proto_names):
    pb_paths = []
    for name in proto_names:
        pb_path = out_dir / (Path(name).stem + ".pb")
        subprocess.run(["protoc", f"-I{FIXTURES_DIR}", f"--descriptor_set_out={pb_path}",
                        str(FIXTURES_DIR / name)], check=True, capture_output=True)
        pb_paths.append(pb_path)
    return pb_paths


def compile_proto_multi(out_path, *proto_names):
    subprocess.run(["protoc", f"-I{FIXTURES_DIR}", "--include_imports",
                    f"--descriptor_set_out={out_path}",
                    *[str(FIXTURES_DIR / n) for n in proto_names]],
                   check=True, capture_output=True)
    return out_path


def _write_pb(path, fdp):
    fds = FileDescriptorSet()
    fds.file.append(fdp)
    path.write_bytes(fds.SerializeToString())
    return path


# test_P1
work = out / "work"
work.mkdir(exist_ok=True)
(base_pb,) = compile_proto(work, "prune_base.proto")
fds = FileDescriptorSet()
fds.ParseFromString(base_pb.read_bytes())
fdp = fds.file[0]
fdp.dependency.append(fdp.name)   # self-reference
(out / "self_dep.pb").write_bytes(fds.SerializeToString())

# test_P5
fdp_a = FileDescriptorProto()
fdp_a.name = "circ_a.proto"
fdp_a.syntax = "proto3"
fdp_a.dependency.append("circ_b.proto")
msg = fdp_a.message_type.add()
msg.name = "MsgA"
fdp_b = FileDescriptorProto()
fdp_b.name = "circ_b.proto"
fdp_b.syntax = "proto3"
fdp_b.dependency.append("circ_a.proto")
msg = fdp_b.message_type.add()
msg.name = "MsgB"
_write_pb(out / "circ_a.pb", fdp_a)
_write_pb(out / "circ_b.pb", fdp_b)

# test_M2
multi_pb = compile_proto_multi(work / "multi.pb", "address_book.proto")
fds = FileDescriptorSet()
fds.ParseFromString(multi_pb.read_bytes())
(out / "multi.textpb").write_text(text_format.MessageToString(fds))
print("ok")
