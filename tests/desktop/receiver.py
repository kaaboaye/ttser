"""Capture terminal input in raw mode, without ever launching a shell."""
import json
import os
import select
import sys
import termios
import time
import tty
from pathlib import Path

root = Path(sys.argv[1])
fd = sys.stdin.fileno()
before = termios.tcgetattr(fd)
try:
    tty.setraw(fd)
    for case in json.loads((root / "cases.json").read_text()):
        name = case["name"]
        os.write(1, ("\r\nTTSER TEST: " + name + "\r\n").encode())
        os.write(1, b"\x1b[?2004h")
        termios.tcflush(fd, termios.TCIFLUSH)
        (root / (name + ".ready")).touch()
        received = bytearray()
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            available, _, _ = select.select([fd], [], [], 0.4)
            if available:
                received.extend(os.read(fd, 65536))
            elif (root / (name + ".sent")).exists():
                break
        (root / (name + ".received")).write_bytes(received)
    (root / "terminal.done").touch()
    while not (root / "finish").exists():
        time.sleep(0.1)
finally:
    os.write(1, b"\x1b[?2004l")
    termios.tcsetattr(fd, termios.TCSANOW, before)
