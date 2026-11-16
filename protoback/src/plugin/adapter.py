# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""protoback's adapter for Python plugins (spec 0431 S3).

Run by protoback as `python3 -c ADAPTER FILE LEGACY`: it loads the
plugin FILE as reproto does (`compile`, then `exec`), checks its entry
point with reproto's messages, then serves the plugin protocol (S2) on
its stdin and stdout: each frame read is a FileDescriptorProto, each
frame written a status byte (0, the changed descriptor; 1, an error
message) and its payload. The plugin's own output goes to stderr, where
it cannot break a frame.
"""

import os
import sys


def main() -> None:
    out = os.fdopen(os.dup(sys.stdout.fileno()), 'wb')
    sys.stdout = sys.stderr
    inp = sys.stdin.buffer

    def write(status: int, payload: bytes) -> None:
        body = bytes([status]) + payload
        n = len(body)
        varint = bytearray()
        while n >= 0x80:
            varint.append((n & 0x7F) | 0x80)
            n >>= 7
        varint.append(n)
        out.write(bytes(varint) + body)
        out.flush()

    def read() -> bytes | None:
        n, shift = 0, 0
        while True:
            b = inp.read(1)
            if not b:
                return None
            n |= (b[0] & 0x7F) << shift
            if not b[0] & 0x80:
                break
            shift += 7
        data = inp.read(n)
        if len(data) != n:
            return None
        return data

    path, legacy = sys.argv[1], sys.argv[2] == '1'
    entry = 'phase2_plugin' if legacy else 'fdp_plugin'
    try:
        with open(path, encoding='utf-8') as f:
            code = compile(f.read(), path, 'exec')
        scope: dict = {}
        exec(code, scope)
    except Exception as e:  # reproto's traceback, as one line
        write(1, f'{type(e).__name__}: {e}'.encode())
        return
    fn = scope.get(entry)
    if fn is None:
        write(1, f"plugin '{path}' does not define {entry}()".encode())
        return
    if not callable(fn):
        write(1, (f"plugin '{path}' defines {entry} as "
                  f"{type(fn).__name__}, which is not callable").encode())
        return

    from google.protobuf.descriptor_pb2 import FileDescriptorProto
    write(0, b'')
    while (data := read()) is not None:
        fdp = FileDescriptorProto()
        fdp.ParseFromString(data)
        try:
            if legacy:
                fn(None, fdp)
            else:
                fn(fdp)
        except Exception as e:
            write(1, f"plugin raised {type(e).__name__} on '{fdp.name}': {e}".encode())
            continue
        write(0, fdp.SerializeToString())


main()
