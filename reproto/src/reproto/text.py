# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

from dataclasses import dataclass, field
from enum import IntEnum, auto
from typing import Final, overload

from .context import Context

# --- Text manipulation --------------------------------------------------------

TAB_SIZE: Final[int] = 2
MAX_LINE_LENGTH: Final[int] = 79


class LineType(IntEnum):
    CODE = 0          # normal active code
    COMMENT = auto()  # actual comment line
    ORPHAN = auto()   # code commented out because it references missing dependencies

# Optional convenient aliases
CODE = LineType.CODE
COMMENT = LineType.COMMENT
ORPHAN = LineType.ORPHAN

@dataclass
class BlockLine:
    """Represents a single line within a Block."""
    text: str              # the line content
    level: int = 0         # indentation level (number of spaces or tabs)
    type: LineType = CODE  # the line kind (default: active code)

    def abandon(self) -> None:
        self.type = ORPHAN

    def prepend(self, s: str) -> None:
        self.text = s + self.text

    def postpend(self, s: str) -> None:
        self.text += s


@dataclass
class Block:
    """Represents a sequence of BlockLines forming a structured text block."""
    lines: list[BlockLine] =  field(default_factory=list)

    def __iter__(self):
        """Allow iterating directly over the BlockLines."""
        return iter(self.lines)

    def __len__(self):
        """Return the number of lines in the block."""
        return len(self.lines)

    @overload
    def __getitem__(self, index: int) -> BlockLine: ...
    @overload
    def __getitem__(self, index: slice) -> list[BlockLine]: ...
    def __getitem__(
            self, index: int | slice
    ) -> BlockLine | list[BlockLine]:
        """Enable indexing like block[0] or slicing like block[1:3]."""
        return self.lines[index]

    def abandon(self) -> None:
        for line in self.lines:
            line.abandon()

    def append(self, line: BlockLine) -> None:
        """Add a new BlockLine to the block."""
        self.lines.append(line)

    def extend(self, block: Block) -> None:
        """Extend a block."""
        self.lines.extend(block.lines)

    def insert(self, index: int, line: BlockLine) -> None:
        """Insert a new BlockLine at the beginning of the block."""
        self.lines.insert(index, line)
        
    def pop(self, index: int = -1) -> "BlockLine":
        """
        Remove and return the BlockLine at the given index.
        Default is -1 (the last line), just like list.pop().
        """
        return self.lines.pop(index)


    def append_div_maybe(self, depth: int = 0) -> None:
        if self and self[-1].text:
            self.append(BlockLine('', depth))

    def prepend(self, s: str) -> None:
        self.lines[0].prepend(s)

    def postpend(self, s: str) -> None:
        self.lines[-1].postpend(s)

    def flush(self, ctx: Context, continuing: bool = False) -> str:
        out = ''

        for index, line in enumerate(self):
            # Line indentation
            indent_spaces = line.level * TAB_SIZE

            if line.type == COMMENT:
                if not ctx.redact_comments:
                    # prepend '// ' (indented)
                    out += " " * indent_spaces + "// " + line.text + '\n'

            elif line.type == ORPHAN:
                if not ctx.redact_orphans:
                    # prepend '///' and preserve apparent indentation
                    # by removing up to 3 spaces from the indent to make room for '///'
                    spaces_to_remove = min(3, indent_spaces)
                    adjusted_indent = indent_spaces - spaces_to_remove
                    out += "///" + " " * adjusted_indent + line.text + '\n'

            else:  # CODE
                out += " " * indent_spaces + line.text + '\n'

        return out

    def max_index(self) -> int:
        return len(self.lines) - 1
    
    def set_type(self, typ: LineType) -> None:
        for line in self.lines:
            line.type = typ

        

def render_bytes(value: bytes) -> str:
    """
    Render bytes as a valid .proto string literal using C-style escapes.
    """
    parts = ['"']
    for b in value:
        # Printable ASCII except \ and "
        if 32 <= b <= 126 and b not in (34, 92):
            parts.append(chr(b))
        else:
            parts.append(f"\\x{b:02x}")
    parts.append('"')
    return "".join(parts)
