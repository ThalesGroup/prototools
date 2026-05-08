# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

import click
from pathlib import Path
from google.protobuf import descriptor_pb2

from fdp_scan_lib import scan


@click.command()
@click.argument("file", type=click.Path(exists=True, dir_okay=False))
@click.option("--proto_out", type=click.Path(file_okay=False), default=None,
              help="Directory to write extracted .pb files")
def main(
    file: Path,
    proto_out: Path | None,
) -> None:
    """
    Scan file for embedded FileDescriptorProto (.proto) blobs and optionally
    write them under PROTO_OUT directory.
    """
    file = Path(file)
    if proto_out is not None:
        proto_out = Path(proto_out)

    with open(file, "rb") as f:
        data = f.read()

    # Wrap the bytes in a memoryview for zero-copy slicing
    mv = memoryview(data)
    candidates = scan(data)

    for start, end in candidates:
        blob = mv[start:end]
        fdp = descriptor_pb2.FileDescriptorProto()
        try:
            fdp.ParseFromString(blob)  # type: ignore[arg-type]  # types-protobuf stubs declare bytes but protobuf accepts any buffer-protocol object at runtime
            print(fdp.name)
            if proto_out:
                out_path = proto_out / fdp.name
                if out_path.suffix == ".proto":
                    out_path = out_path.with_suffix(".pb")
                out_path.parent.mkdir(parents=True, exist_ok=True)
                out_path.write_bytes(blob)
        except Exception:
            continue


if __name__ == "__main__":
    main()
