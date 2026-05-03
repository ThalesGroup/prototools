# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Mixin providing source_code_info comment extraction for descriptor nodes."""

from __future__ import annotations

from typing import Any

from .text import COMMENT, Block, BlockLine


class SourceCodeInfoMixin:
    """Mixin providing render_message_comments and _calculate_source_code_info_path.

    Requires the host class to provide: _parent, name, nested_type, parent.
    """

    def render_message_comments(self, depth: int = 0) -> Block:
        """Extract and render message-level comments from source_code_info."""
        from .re_file import ReFileDescriptorProto

        out = Block()

        # Find the root file to access source_code_info
        current: Any = self
        while current.parent is not None:
            parent = current.parent
            if isinstance(parent, ReFileDescriptorProto):
                file = parent
                break
            current = parent
        else:
            return out

        if not file.source_code_info:
            return out

        path = self._calculate_source_code_info_path()
        if not path:
            return out

        for location in file.source_code_info.location:
            if list(location.path) == path:
                if location.leading_comments:
                    for line in location.leading_comments.strip().split('\n'):
                        out.append(BlockLine(line, depth, COMMENT))
                for detached in location.leading_detached_comments:
                    for line in detached.strip().split('\n'):
                        out.append(BlockLine(line, depth, COMMENT))
                if location.trailing_comments:
                    for line in location.trailing_comments.strip().split('\n'):
                        out.append(BlockLine(line, depth, COMMENT))
                break

        return out

    def _calculate_source_code_info_path(self) -> list[int] | None:
        """Calculate the source_code_info path for this message."""
        from .re_descriptor import ReDescriptorProto
        from .re_file import ReFileDescriptorProto

        path_segments = []
        current: Any = self

        while current.parent is not None:
            parent = current.parent

            if isinstance(parent, ReFileDescriptorProto):
                try:
                    index = next(i for i, m in enumerate(parent.message_type)
                                 if m.name == current.name)
                    path_segments.insert(0, [4, index])
                except StopIteration:
                    return None
                break
            elif isinstance(parent, ReDescriptorProto):
                try:
                    index = next(i for i, m in enumerate(parent.nested_type)
                                 if m.name == current.name)
                    path_segments.insert(0, [3, index])
                except StopIteration:
                    return None

            current = parent

        flat_path: list[int] = []
        for segment in path_segments:
            flat_path.extend(segment)

        return flat_path if flat_path else None
