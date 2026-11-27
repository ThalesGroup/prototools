# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Spec 0406 S6: press Tab in an interactive bash and check the line.

Usage: tab_completion.py BINDIR

For each of prototext, protolens, protoscan and protoback found in BINDIR,
sources the script that `<TOOL>_COMPLETE=bash <tool>` prints into an
interactive bash on a pseudo-terminal, one per tool, types each line of the
spec's table, presses Tab, and compares the command line readline then holds
with the expected one. Exits non-zero if any line differs.
"""

import os
import pty
import select
import shutil
import sys
import tempfile
import time

# (typed, expected), with "{sub}" standing for the tool's subcommand prefix:
# "decode " for prototext, nothing for protolens. Paths refer to the tree
# built by make_tree.
PATH_ROWS = [
    ("{sub}da", "{sub}data/"),
    ("{sub}dir", r"{sub}dir\ with\ space/"),
    ('{sub}"dir w', '{sub}"dir with space"/'),
    ("{sub}'dir w", "{sub}'dir with space'/"),
    (r"{sub}dir\ w", r"{sub}dir\ with\ space/"),
    ("{sub}a:", "{sub}a:b/"),
    ("{sub}a:b/", "{sub}a:b/z.pb "),
    ("{sub}ka:", "{sub}ka:li.pb "),
    ("--descriptor-set=da", "--descriptor-set=data/"),
    (r"--descriptor-set=dir\ w", r"--descriptor-set=dir\ with\ space/"),
    ('--descriptor-set="dir w', '--descriptor-set="dir with space"/'),
    ("--descriptor-set da", "--descriptor-set data/"),
    ("{sub}ét", "{sub}été/"),
    ("{sub}data/x", "{sub}data/x.pb "),
]

# protoscan (spec 0407) takes a positional FILE and --proto-out DIR, so its
# option rows are --proto-out's. "--proto-o" must complete to --proto-out
# alone: the deprecated --proto_out is a hidden alias, never offered.
PROTOSCAN_ROWS = [
    (typed.replace("--descriptor-set", "--proto-out"),
     expected.replace("--descriptor-set", "--proto-out"))
    for typed, expected in PATH_ROWS
] + [("--proto-o", "--proto-out ")]

# protoback (spec 0433 S7, as reproto, spec 0422): DESCRIPTOR_FILES relative
# to the -I roots on the line, which the shell cannot see from where it runs
# ("sub/" with no space), or the members of an -I blob; -O's directories,
# --proto-variant's .yaml files. Without -I, the current directory.
PROTOBACK_ROWS = [
    (typed.replace("--descriptor-set", "--proto-out"),
     expected.replace("--descriptor-set", "--proto-out"))
    for typed, expected in PATH_ROWS
] + [
    ("-I roots/r1 f", "-I roots/r1 f.pb "),
    ("-I roots/r1 su", "-I roots/r1 sub/"),
    ("-I roots/r1 sub/", "-I roots/r1 sub/s.pb "),
    ("--desc-root=roots/r1 f", "--desc-root=roots/r1 f.pb "),
    ("-I roots/r1 no", "-I roots/r1 no"),
    ("-I roots/blob.bin fi", "-I roots/blob.bin first.pb "),
    ("--proto-variant v", "--proto-variant v.yaml "),
    ("--use-variant=desc", "--use-variant=descriptor "),
]

TOOLS = {
    # "decode -I …": bash hands the completer the raw line, whose words
    # args::flag_values reads only once Bash has split them.
    "prototext": PATH_ROWS + [("decode --t", "decode --type "), ("dec", "decode "),
                              ("decode -I roots/r1 f", "decode -I roots/r1 f.pb ")],
    "protolens": PATH_ROWS + [("--ty", "--type ")],
    "protoscan": PROTOSCAN_ROWS,
    "protoback": PROTOBACK_ROWS,
}
SUB = {"prototext": "decode ", "protolens": "", "protoscan": "", "protoback": ""}


def field(number, payload):
    """A length-delimited protobuf field, its length one byte."""
    assert len(payload) < 128
    return bytes([number << 3 | 2, len(payload)]) + payload


def make_tree(root):
    for d in ["data", "dir with space", "a:b", "été", "roots/r1/sub"]:
        os.makedirs(os.path.join(root, d))
    for f in ["data/x.pb", "dir with space/y.pb", "a:b/z.pb", "ka:li.pb",
              "roots/r1/f.pb", "roots/r1/notes.txt", "roots/r1/sub/s.pb", "v.yaml"]:
        open(os.path.join(root, f), "w").close()
    # A blob: a FileDescriptorProto (name, package, a message with a field)
    # among other bytes, as protoscan finds them in a binary.
    message = field(1, b"M") + field(2, field(1, b"x") + b"\x18\x01\x20\x01\x28\x05")
    fdp = field(1, b"first.proto") + field(2, b"p") + field(4, message)
    with open(os.path.join(root, "roots/blob.bin"), "wb") as f:
        f.write(bytes(range(256)) * 4 + fdp + b"\x00\xff" * 64)


# The line readline holds, printed between markers by a key bound to a shell
# command (`bind -x`), which readline runs only once the keys before it,
# Tab's completion included, are handled: no fixed wait, and the line as
# readline holds it, with no terminal escape to strip.
SHOW_LINE = "\x18l"  # C-x l
LINE_START, LINE_END = "@@line<", ">line@@"
READY = "@@ready@@"
# Only a bound on a hang, never waited out on success.
TIMEOUT = 10.0


class Shell:
    """An interactive bash on a pseudo-terminal, one per tool, with the
    tool's completion script sourced."""

    def __init__(self, bindir, root, tool):
        self.tool = tool
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(root)
            env = {
                "PATH": bindir + os.pathsep + os.environ["PATH"],
                "HOME": root,
                "TERM": "dumb",
                "PS1": "$ ",
                "LC_ALL": "C.UTF-8",
            }
            os.execvpe("bash", ["bash", "--norc", "--noprofile", "-i"], env)
        self.pending = b""
        var = tool.upper() + "_COMPLETE"
        show = f'printf "\\n{LINE_START}%s{LINE_END}\\n" "$READLINE_LINE"'
        self.send(f'source <({var}=bash {tool}); bind "set bell-style none"; '
                  f"bind -x '\"\\C-xl\": {show}'; echo {READY[:2]}''{READY[2:]}\n")
        self.until(READY)

    def send(self, text):
        os.write(self.fd, text.encode())

    def until(self, marker):
        """The output up to `marker`, which it consumes."""
        wanted, end = marker.encode(), time.time() + TIMEOUT
        while wanted not in self.pending:
            if time.time() > end:
                raise TimeoutError(f"{self.tool}: no {marker!r} after {TIMEOUT} s: "
                                   f"{self.pending.decode(errors='replace')!r}")
            if select.select([self.fd], [], [], 0.05)[0]:
                self.pending += os.read(self.fd, 4096)
        head, _, self.pending = self.pending.partition(wanted)
        return head.decode(errors="replace")

    def tab(self, typed):
        """The command line after typing `tool typed` and pressing Tab."""
        self.send(f"{self.tool} {typed}\t{SHOW_LINE}")
        self.until(LINE_START)
        line = self.until(LINE_END)
        self.send("\x15")  # C-u: an empty line for the next one
        return line

    def close(self):
        self.send("\x15exit\n")
        os.waitpid(self.pid, 0)


def main():
    bindir = os.path.abspath(sys.argv[1])
    failures = 0
    with tempfile.TemporaryDirectory() as root:
        make_tree(root)
        for tool, rows in TOOLS.items():
            if not shutil.which(tool, path=bindir):
                continue
            shell = Shell(bindir, root, tool)
            for typed, expected in rows:
                typed = typed.format(sub=SUB[tool])
                expected = f"{tool} " + expected.format(sub=SUB[tool])
                got = shell.tab(typed)
                ok = got == expected
                failures += not ok
                print(f"{'ok  ' if ok else 'FAIL'} {tool} {typed!r:42} -> {got!r}"
                      + ("" if ok else f" (expected {expected!r})"))
            shell.close()
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
