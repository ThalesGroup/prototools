# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The textproto tree-sitter grammar, for the Python tree_sitter library.

`language()` returns the grammar as a PyCapsule, to pass to
`tree_sitter.Language` (spec 0408).
"""

from ._binding import language

__all__ = ["language"]
