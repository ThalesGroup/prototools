# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""CLI entry point for reproto-instantiate-schema."""

from __future__ import annotations

import os
from pathlib import Path

import click

from .instantiate import generate_instance, load_pool


@click.command()
@click.version_option()
@click.option(
    '--descriptor',
    'descriptor_path',
    required=False,
    default=None,
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    help=(
        'Path to the .desc FileDescriptorSet to load. '
        'Falls back to $PROTOTEXT_DEFAULT_DESCRIPTOR if not provided.'
    ),
)
@click.option(
    '-O', '--output-root',
    'output_root',
    default=None,
    type=click.Path(file_okay=False, path_type=Path),
    help='Root directory for output files (default: current directory).',
)
@click.option(
    '--seed',
    default=0,
    type=int,
    help='PRNG seed (default: 0).',
)
@click.option(
    '--max-depth',
    default=4,
    type=int,
    help='Maximum recursion depth for nested messages (default: 4).',
)
@click.option(
    '--max-repeated',
    default=3,
    type=int,
    help='Maximum number of elements for repeated fields (default: 3).',
)
@click.option(
    '--quiet', '-q',
    is_flag=True,
    help='Suppress per-file progress messages.',
)
@click.argument('fqdns', nargs=-1, required=True)
def main(
    descriptor_path: Path,
    output_root: Path | None,
    seed: int,
    max_depth: int,
    max_repeated: int,
    quiet: bool,
    fqdns: tuple[str, ...],
) -> None:
    """Generate pseudo-random binary protobuf instances from a .desc FileDescriptorSet.

    FQDNS are fully-qualified message type names.  For each FQDN an output file
    is written to <output-root>/<fqdn-with-dots-as-slashes>.pb.
    """
    root = output_root or Path('.')
    if descriptor_path is None:
        env_val = os.environ.get('PROTOTEXT_DEFAULT_DESCRIPTOR')
        if env_val is None:
            raise click.UsageError(
                'No descriptor specified. Use --descriptor or set PROTOTEXT_DEFAULT_DESCRIPTOR.'
            )
        descriptor_path = Path(env_val)
    pool = load_pool(descriptor_path)
    for fqdn in fqdns:
        try:
            wire = generate_instance(
                fqdn, pool,
                seed=seed,
                max_depth=max_depth,
                max_repeated=max_repeated,
            )
        except KeyError as e:
            raise click.ClickException(f'type not found in descriptor: {e}') from e
        if not wire:
            continue
        out_path = root / Path(*fqdn.split('.'))
        out_path = out_path.with_suffix('.pb')
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_bytes(wire)
        if not quiet:
            click.echo(str(out_path))
