# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The textproto grammar's C extension (spec 0408 S4).

Built from the committed parser (src/parser.c) and the binding (binding.c),
with a C compiler alone: no tree-sitter CLI. The rest of the package is
declared in pyproject.toml.
"""

from setuptools import Extension, setup

setup(
    ext_modules=[
        Extension(
            "tree_sitter_textproto._binding",
            sources=["binding.c", "src/parser.c"],
            include_dirs=["src"],
        )
    ]
)
