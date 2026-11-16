# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Shared Rich console, progress bars, and spinner (spec 0075).

All reproto output to the terminal is routed through `rprint` so that the
Rich Live context used by `progress` can redraw the progress bar below any
log lines without interleaving artefacts.
"""

from __future__ import annotations

from collections.abc import Callable, Generator
from contextlib import contextmanager
from typing import Protocol

from rich.console import Console
from rich.progress import BarColumn, MofNCompleteColumn, Progress, TextColumn, TimeElapsedColumn

# Single console instance for the whole process.
# highlight=False: prevents Rich from auto-highlighting numbers/strings in
# log messages, which would alter their appearance unexpectedly.
# soft_wrap=True: a message is one line, whatever its length; Rich would
# otherwise word-wrap it at the console's width, 80 columns when stderr is
# not a terminal, splitting log lines (spec 0433 S1).
console = Console(stderr=True, highlight=False, soft_wrap=True)


class Advance(Protocol):
    """What `progress` and `progress_lazy` yield to move their bar."""

    def __call__(self, n: int = 1) -> None: ...


def _no_advance(n: int = 1) -> None:
    """No bar to move."""


def _no_total(n: int) -> None:
    """No bar to size."""


def rprint(message: str, *, style: str | None = None) -> None:
    """Print a message to stderr via the shared Rich console.

    markup=False ensures square brackets (e.g. WARNING[downconvert]) are
    treated as literals rather than Rich markup.
    """
    console.print(message, style=style, markup=False)


@contextmanager
def progress(label: str, total: int, quiet: bool = False) -> Generator[Advance, None, None]:
    """Context manager that shows a progress bar for a countable step.

    Yields an advance(n=1) callable.  Calling advance() moves the bar forward
    by 1 step; calling advance(n) moves it forward by n steps.

    The bar is suppressed when:
    - stderr is not a TTY (piped/redirected output), or
    - total == 0 (caller passes 0 to opt out, e.g. --quiet mode).
    Label is suppressed when quiet=True.
    """
    if not console.is_terminal or total == 0:
        yield _no_advance
        if not quiet:
            rprint(label)
        return
    with Progress(
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        MofNCompleteColumn(),
        TimeElapsedColumn(),
        console=console,
        transient=True,
    ) as prog:
        task = prog.add_task(label, total=total)

        def advance(n: int = 1) -> None:
            prog.advance(task, advance=n)

        yield advance
    rprint(label)


@contextmanager
def progress_lazy(
    label: str, quiet: bool = False
) -> Generator[tuple[Advance, Callable[[int], None]], None, None]:
    """Context manager for a progress bar whose total is not known upfront.

    Yields (advance, set_total) where:
    - set_total(n) must be called once before any advance() calls to set the
      bar total; until then the bar is indeterminate.
    - advance(n=1) moves the bar forward by n steps.

    Suppressed (no-op) when stderr is not a TTY or quiet=True.
    Prints label on completion.
    """
    if quiet or not console.is_terminal:
        yield (_no_advance, _no_total)
        if not quiet and console.is_terminal:
            rprint(label)
        return
    with Progress(
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        MofNCompleteColumn(),
        TimeElapsedColumn(),
        console=console,
        transient=True,
    ) as prog:
        task = prog.add_task(label, total=None)

        def set_total(n: int) -> None:
            prog.update(task, total=n)

        def advance(n: int = 1) -> None:
            prog.advance(task, advance=n)

        yield (advance, set_total)
    rprint(label)


@contextmanager
def spinning(label: str, quiet: bool = False) -> Generator[None, None, None]:
    """Context manager that shows a spinner for an indeterminate blocking step.

    Suppressed (no spinner, no label) when stderr is not a TTY or quiet=True.
    Prints label on completion so the phase name persists on the terminal.
    """
    if quiet:
        yield
        return
    if not console.is_terminal:
        yield
        rprint(label)
        return
    with console.status(label, spinner="dots"):
        yield
    rprint(label)
