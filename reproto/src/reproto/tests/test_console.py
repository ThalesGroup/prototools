# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""reproto's console prints a message as one line (spec 0433 S1).

Rich assumes 80 columns when stderr is not a terminal and word-wrapped
longer messages, splitting a warning across lines.
"""

import subprocess
import sys


def test_a_long_message_is_one_line() -> None:
    message = ' '.join(['word'] * 40)  # 199 characters
    code = ('import sys; from reproto.lib.console import rprint; '
            'rprint(sys.argv[1])')
    r = subprocess.run([sys.executable, '-c', code, message],
                       capture_output=True, text=True, check=True)
    assert r.stderr == message + '\n'
