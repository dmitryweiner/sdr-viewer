#!/usr/bin/env python3
# Runs the release binary in a pseudo-terminal of a given size, presses keys,
# quits with `q`, and reports the exit code and the CPU the app used.
# Fails (exit 1) if the app crashes, does not quit, or exits non-zero.
#
#   scripts/smoke.py                         # generator, 160x45, keys 1-8, ? and Tab
#   scripts/smoke.py --size 80x24 --secs 5
#   scripts/smoke.py --keys '25?' -- capture.cu8 --rate 2.048M
#   scripts/smoke.py --keys 5 --words -- --db /tmp/e.db | grep -E '^[0-9:.]+$'
#   scripts/smoke.py --keys $'5\x1b[Br' --words -- --iq-dir /tmp/iq | grep IQ
#
# An escape sequence in --keys is one key: $'\x1b[B' is the down arrow.
#
# Prints e.g. "ok: exit 0, 3.0 s, cpu 41% of one core (user 1.10 s, sys 0.13 s)".
# Arguments after `--` go to sdr-viewer. Builds with `cargo build --release`
# first unless --no-build or --bin (a binary elsewhere, e.g. on the uConsole:
# `scripts/smoke.py --bin ./sdr-viewer`; needs only Python 3).
# --words also prints every distinct piece of text the app wrote, one per
# line, escape sequences taken as the breaks: not the screen (a terminal
# redraws in pieces), but enough to grep for what must be on it.
import argparse
import fcntl
import json
import os
import pty
import re
import select
import struct
import subprocess
import sys
import termios
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def binary_path():
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=ROOT))
    return os.path.join(meta["target_directory"], "release", "sdr-viewer")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--size", default="160x45", help="COLSxROWS")
    parser.add_argument("--secs", type=float, default=3.0, help="run time before q")
    parser.add_argument("--keys", default="12345678?\t", help="keys pressed one by one")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--bin", help="run this binary instead of building one")
    parser.add_argument("--words", action="store_true",
                        help="print the pieces of text written to the terminal")
    parser.add_argument("app_args", nargs="*")
    args = parser.parse_args()

    if not args.no_build and not args.bin:
        subprocess.check_call(["cargo", "build", "--release", "-q"], cwd=ROOT)
    binary = args.bin or binary_path()
    cols, rows = (int(v) for v in args.size.split("x"))

    pid, fd = pty.fork()
    if pid == 0:
        os.execv(binary, ["sdr-viewer", *args.app_args])
    # Without a window size ratatui sees 0x0 and draws nothing.
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    output = bytearray()

    def drain(seconds):
        end = time.monotonic() + seconds
        while (left := end - time.monotonic()) > 0:
            ready, _, _ = select.select([fd], [], [], left)
            if ready:
                try:
                    output.extend(os.read(fd, 65536))
                except OSError:
                    return

    started = time.monotonic()
    drain(0.5)
    keys = re.findall(r"\x1b\[[0-9;]*[A-Za-z~]|.", args.keys, re.S)
    pause = max(args.secs - 0.5, 0) / max(len(keys), 1)
    try:
        for key in keys:
            os.write(fd, key.encode())
            drain(pause)
        if not keys:
            drain(max(args.secs - 0.5, 0))
        os.write(fd, b"q")
    except OSError:
        pass  # the app has already exited; its status says how

    deadline = time.monotonic() + 3
    status = None
    while time.monotonic() < deadline:
        done, status, usage = os.wait4(pid, os.WNOHANG)
        if done:
            break
        drain(0.05)
    else:
        os.kill(pid, 9)
        print("FAIL: did not quit within 3 s of q")
        return 1
    wall = time.monotonic() - started
    code = os.waitstatus_to_exitcode(status)
    cpu = usage.ru_utime + usage.ru_stime
    report = (f"exit {code}, {wall:.1f} s, cpu {100 * cpu / wall:.0f}% of one core "
              f"(user {usage.ru_utime:.2f} s, sys {usage.ru_stime:.2f} s), "
              f"{len(output) / 1024:.0f} KiB to the terminal")
    if code != 0 or not output:
        print("FAIL:", report)
        sys.stdout.write(output.decode(errors="replace")[-2000:])
        return 1
    print("ok:", report)
    if args.words:
        text = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"\n", bytes(output)).decode(errors="replace")
        for word in sorted({line.strip() for line in text.splitlines()} - {""}):
            print(word)
    return 0


if __name__ == "__main__":
    sys.exit(main())
