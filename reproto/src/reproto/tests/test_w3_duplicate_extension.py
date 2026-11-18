# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""W3: a file reusing an extension number is pruned (spec 0438 S1).

Two files extending one message with the same number each compile alone,
but the descriptor pool refuses the second. upb 36.2 then kept the refused
file's extensions while freeing them: protoback aborted, reproto (through
Python's protobuf) crashed building the schema database. The second file is
now pruned before the pool, as a duplicate symbol is.
"""

import os
import subprocess
from pathlib import Path

from .conftest import REPROTO_CMD

BASE = ('syntax = "proto2";\npackage p;\n'
        'message Config { extensions 100 to max; }\n')
A = ('syntax = "proto2";\npackage a;\nimport "base.proto";\n'
     'message A { extend p.Config { optional A ext = 1000; } }\n')
B = ('syntax = "proto2";\npackage b;\nimport "base.proto";\n'
     'message B { extend p.Config { optional B ext = 1000; } }\n')
# A symbol of a.proto's and an extension number of it both.
C = ('syntax = "proto2";\npackage a;\nimport "base.proto";\n'
     'message A { extend p.Config { optional int32 other = 1000; } }\n')


def _run(tmp_path: Path, sources: dict[str, str], *flags: str
         ) -> subprocess.CompletedProcess[str]:
    src = tmp_path / "src"
    inp = tmp_path / "in"
    src.mkdir(exist_ok=True)
    inp.mkdir(exist_ok=True)
    for name, text in {"base.proto": BASE, **sources}.items():
        (src / name).write_text(text, encoding="utf-8")
        # Each compiled alone: together, protoc refuses them.
        subprocess.run(["protoc", f"-I{src}",
                        f"--descriptor_set_out={inp / name}.pb", name],
                       check=True, capture_output=True)
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    return subprocess.run(
        [*REPROTO_CMD, "--use-variant", "descriptor", *flags, f"-I{inp}",
         "-O", str(tmp_path / "out"),
         f"--schema-db-out={tmp_path / 'db.desc'}"],
        capture_output=True, text=True, env=env,
    )


def test_a_reused_extension_number_prunes_the_second_file(tmp_path: Path) -> None:
    result = _run(tmp_path, {"a.proto": A, "b.proto": B})
    assert result.returncode == 0, result.stderr
    assert ("Warning [W3]: file:b.proto pruned — duplicate extension numbers with:\n"
            "    file:a.proto (p.Config 1000)\n") in result.stderr, result.stderr
    # Pruned before the pool: not refused by it.
    assert "[W19]" not in result.stderr and "[W20]" not in result.stderr, result.stderr
    assert not (tmp_path / "out" / "b.proto").exists()
    assert (tmp_path / "out" / "a.proto").exists()


def test_fail_on_w3_counts_it(tmp_path: Path) -> None:
    result = _run(tmp_path, {"a.proto": A, "b.proto": B}, "--fail-on", "W3")
    assert result.returncode == 3, result.stderr


def test_symbols_and_extension_numbers_in_one_warning(tmp_path: Path) -> None:
    result = _run(tmp_path, {"a.proto": A, "c.proto": C})
    assert result.returncode == 0, result.stderr
    assert ("Warning [W3]: file:c.proto pruned — duplicate symbols with:\n"
            "    file:a.proto (1 symbol)\n"
            "  and duplicate extension numbers with:\n"
            "    file:a.proto (p.Config 1000)\n") in result.stderr, result.stderr
