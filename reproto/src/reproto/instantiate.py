# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Instance generator for reproto instantiate-schema (spec 0063).

Generates pseudo-random binary protobuf instances from a FileDescriptorSet,
using google.protobuf which correctly handles proto3 packing defaults.
"""

from __future__ import annotations

import hashlib
import random
from pathlib import Path
from typing import Any

from google.protobuf import descriptor_pb2, descriptor_pool, message_factory
from google.protobuf.descriptor import FieldDescriptor


# ── PRNG seeding ──────────────────────────────────────────────────────────────

def _make_rng(seed: int, fqdn: str) -> random.Random:
    """Derive a per-type PRNG from seed and fqdn.

    Algorithm matches the Rust implementation:
      seed_input = f"{seed}:{fqdn_with_leading_dot}"
      hash_bytes = SHA256(seed_input)
    """
    dotted = fqdn if fqdn.startswith('.') else f'.{fqdn}'
    seed_input = f'{seed}:{dotted}'.encode()
    hash_bytes = hashlib.sha256(seed_input).digest()
    int_seed = int.from_bytes(hash_bytes, 'big')
    rng = random.Random()
    rng.seed(int_seed)
    return rng


# ── Value generators ──────────────────────────────────────────────────────────

def _p_at_depth(depth: int, max_depth: int) -> float:
    """Inclusion probability for optional/repeated fields at `depth`.

    p = (max_depth - depth) / max_depth
    - depth 0  -> 1.0  (always include)
    - depth max_depth -> 0.0 (never include; recursion terminates)
    """
    if max_depth == 0:
        return 0.0
    return (max_depth - depth) / max_depth


def _random_value(
    field: Any,
    rng: random.Random,
    depth: int,
    max_depth: int,
    max_repeated: int,
    pool: descriptor_pool.DescriptorPool,
) -> Any:
    """Generate a random scalar or message value for a single field occurrence."""
    ft = field.type

    if ft in (FieldDescriptor.TYPE_MESSAGE, FieldDescriptor.TYPE_GROUP):
        if depth >= max_depth:
            return None
        nested_desc = field.message_type
        return _generate_message(
            nested_desc, rng, depth + 1, max_depth, max_repeated, pool
        )

    if ft == FieldDescriptor.TYPE_ENUM:
        values = [v.number for v in field.enum_type.values]
        # Prefer non-zero values (proto3 zero enum = default/"unset"); fall back
        # to the full set only when every value is zero.
        non_zero = [v for v in values if v != 0]
        return rng.choice(non_zero if non_zero else values)

    if ft == FieldDescriptor.TYPE_BOOL:
        # False encodes as nothing in proto3; always emit True.
        return True

    if ft == FieldDescriptor.TYPE_STRING:
        return f's{rng.randint(1, 9999)}'

    if ft == FieldDescriptor.TYPE_BYTES:
        length = rng.randint(1, 8)
        return bytes(rng.randint(0, 255) for _ in range(length))

    if ft == FieldDescriptor.TYPE_FLOAT:
        return rng.uniform(1.0, 1000.0)

    if ft == FieldDescriptor.TYPE_DOUBLE:
        return rng.uniform(1.0, 1000.0)

    # All integer types — start from 1 so proto3 default suppression never
    # silently drops the field.
    if ft in (
        FieldDescriptor.TYPE_INT32,
        FieldDescriptor.TYPE_SINT32,
        FieldDescriptor.TYPE_SFIXED32,
    ):
        return rng.randint(1, 1000)

    if ft in (
        FieldDescriptor.TYPE_INT64,
        FieldDescriptor.TYPE_SINT64,
        FieldDescriptor.TYPE_SFIXED64,
    ):
        return rng.randint(1, 1000)

    if ft in (FieldDescriptor.TYPE_UINT32, FieldDescriptor.TYPE_FIXED32):
        return rng.randint(1, 1000)

    if ft in (FieldDescriptor.TYPE_UINT64, FieldDescriptor.TYPE_FIXED64):
        return rng.randint(1, 1000)

    return None


def _generate_message(
    desc: Any,
    rng: random.Random,
    depth: int,
    max_depth: int,
    max_repeated: int,
    pool: descriptor_pool.DescriptorPool,
) -> Any:
    """Recursively generate a message instance."""
    cls = message_factory.GetMessageClass(desc)
    msg = cls()
    p = _p_at_depth(depth, max_depth)

    # Collect fields covered by real oneofs (not synthetic proto3 optional oneofs).
    # The C extension of google.protobuf does not expose is_synthetic; detect
    # synthetic oneofs by convention: proto3 optional fields get a single-field
    # oneof whose name is '_' + field_name.
    oneof_field_numbers: set[int] = set()
    for oneof in desc.oneofs:
        fields_list = list(oneof.fields)
        is_synthetic = (
            len(fields_list) == 1
            and oneof.name == f"_{fields_list[0].name}"
        )
        if is_synthetic:
            continue
        for f in oneof.fields:
            oneof_field_numbers.add(f.number)
        # Decide whether to populate this oneof at all.
        if rng.random() >= p:
            continue
        chosen = rng.choice(list(oneof.fields))
        val = _random_value(chosen, rng, depth, max_depth, max_repeated, pool)
        if val is not None:
            getattr(msg, chosen.name)  # touch to ensure field exists
            try:
                if chosen.type == FieldDescriptor.TYPE_MESSAGE:
                    getattr(msg, chosen.name).CopyFrom(val)
                else:
                    setattr(msg, chosen.name, val)
            except (AttributeError, ValueError):
                pass

    for field in desc.fields:
        if field.number in oneof_field_numbers:
            continue

        label = field.label

        if label == FieldDescriptor.LABEL_REPEATED:
            max_count = max(1, round(p * max_repeated))
            min_count = 1 if depth == 0 else 0
            count = rng.randint(min_count, max_count)
            if count == 0:
                continue
            is_map = (
                field.type == FieldDescriptor.TYPE_MESSAGE
                and field.message_type.GetOptions().map_entry
            )
            repeated = getattr(msg, field.name)
            for _ in range(count):
                val = _random_value(field, rng, depth, max_depth, max_repeated, pool)
                if val is None:
                    continue
                try:
                    if is_map:
                        # Map fields: val is the entry message; assign via key.
                        map_val_desc = field.message_type.fields_by_name['value']
                        if map_val_desc.type == FieldDescriptor.TYPE_MESSAGE:
                            repeated[val.key].CopyFrom(val.value)
                        else:
                            repeated[val.key] = val.value
                    elif field.type == FieldDescriptor.TYPE_MESSAGE:
                        repeated.add().CopyFrom(val)
                    else:
                        repeated.append(val)
                except (AttributeError, ValueError):
                    pass

        elif label == FieldDescriptor.LABEL_REQUIRED:
            val = _random_value(field, rng, depth, max_depth, max_repeated, pool)
            if val is not None:
                try:
                    if field.type == FieldDescriptor.TYPE_MESSAGE:
                        getattr(msg, field.name).CopyFrom(val)
                    else:
                        setattr(msg, field.name, val)
                except (AttributeError, ValueError):
                    pass

        else:  # LABEL_OPTIONAL
            if rng.random() < p:
                val = _random_value(field, rng, depth, max_depth, max_repeated, pool)
                if val is not None:
                    try:
                        if field.type == FieldDescriptor.TYPE_MESSAGE:
                            getattr(msg, field.name).CopyFrom(val)
                        else:
                            setattr(msg, field.name, val)
                    except (AttributeError, ValueError):
                        pass

    return msg


# ── Public entry point ────────────────────────────────────────────────────────

def generate_instance(
    fqdn: str,
    pool: descriptor_pool.DescriptorPool,
    *,
    seed: int = 0,
    max_depth: int = 4,
    max_repeated: int = 3,
) -> bytes:
    """Generate a pseudo-random binary protobuf instance for `fqdn`.

    Inclusion probability for optional/repeated fields decreases linearly with
    nesting depth: p = (max_depth - depth) / max_depth.  Top-level fields are
    always included (p=1); fields at max_depth are never included (p=0),
    guaranteeing termination without a separate recursion guard for scalars.

    Returns the raw wire bytes (SerializeToString output).
    """
    desc = pool.FindMessageTypeByName(fqdn)
    rng = _make_rng(seed, fqdn)
    msg = _generate_message(desc, rng, 0, max_depth, max_repeated, pool)
    return msg.SerializeToString()


def load_pool(desc_path: Path) -> descriptor_pool.DescriptorPool:
    """Load a FileDescriptorSet from `desc_path` into a fresh descriptor pool."""
    data = desc_path.read_bytes()
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(data)

    pool = descriptor_pool.Default()
    for f in fds.file:
        try:
            pool.Add(f)
        except TypeError:
            # File already registered (e.g. well-known types).
            pass

    return pool
