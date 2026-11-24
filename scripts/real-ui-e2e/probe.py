"""PTY probe for the #5145 real UI journeys (P01).

AgentsCommander starts this file as the local coding agent `e2e-probe`. It runs inside the
product PTY, so it has no imports from this folder and only uses the standard library.

The driver writes a 0600 map next to this file (`<this file>.json`):
  {"journal": "<abs path>", "turns": [{"ask": "...", "reply": "...", "emit": true}, ...]}

Behaviour:
1. Append a `start` event with this PID and its start ticks before anything else.
2. Check that stdin and stdout are the same PTY, configure and re-read the termios flags.
3. Print `PROBE READY`.
4. For every line read, append a numbered `read` event first, then answer only when the
   line equals the ASK of that turn and the turn has `emit=true`. Never answer otherwise.
"""

import hashlib
import json
import os
import sys
import termios

READY = "PROBE READY"
MAX_LINE = 4096


def start_ticks(pid):
    """Field 22 of /proc/<pid>/stat (start time in clock ticks)."""
    with open(f"/proc/{pid}/stat", "rb") as handle:
        raw = handle.read().decode("ascii", "replace")
    # The command name sits in parentheses and may hold spaces; split after the last one.
    rest = raw[raw.rindex(")") + 2:].split()
    return int(rest[19])


def tty_dev(fd):
    info = os.fstat(fd)
    return [os.major(info.st_rdev), os.minor(info.st_rdev)]


def replica_key():
    return hashlib.sha256(os.fsencode(os.path.realpath(os.getcwd()))).hexdigest()


class Journal:
    """One JSON record per event, written with a single O_APPEND write."""

    def __init__(self, path):
        self.fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)

    def write(self, record):
        data = (json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n").encode()
        written = os.write(self.fd, data)
        if written != len(data):
            raise OSError("short journal write")


def load_map():
    path = os.path.realpath(__file__) + ".json"
    with open(path, encoding="utf-8") as handle:
        data = json.load(handle)
    turns = data.get("turns")
    if not isinstance(turns, list) or len(turns) != 2:
        raise ValueError("bad probe map")
    return data["journal"], turns


def identity(pid, ticks):
    ident = {"pid": pid, "startTicks": ticks, "ttyDev": None, "replicaKey": None}
    try:
        ident["ttyDev"] = tty_dev(0)
    except OSError:
        pass
    try:
        ident["replicaKey"] = replica_key()
    except OSError:
        pass
    return ident


def configure_tty():
    """Return None when the PTY is ready, else an error code."""
    if not (os.isatty(0) and os.isatty(1)):
        return "not_a_tty"
    if tty_dev(0) != tty_dev(1):
        return "tty_mismatch"
    attrs = termios.tcgetattr(0)
    attrs[0] |= termios.ICRNL
    attrs[0] &= ~(termios.IGNCR | termios.INLCR)
    attrs[3] |= termios.ICANON | termios.ECHO | termios.ECHOE
    attrs[6][termios.VERASE] = b"\x7f"
    termios.tcsetattr(0, termios.TCSANOW, attrs)
    check = termios.tcgetattr(0)
    iflag, lflag, erase = check[0], check[3], check[6][termios.VERASE]
    ok = (
        iflag & termios.ICRNL
        and not iflag & termios.IGNCR
        and not iflag & termios.INLCR
        and lflag & termios.ICANON
        and lflag & termios.ECHO
        and lflag & termios.ECHOE
        and erase in (b"\x7f", 0x7F)
    )
    return None if ok else "termios_mismatch"


def read_line(buffer):
    """Read one LF-terminated line from fd 0. Returns (line or None, buffer)."""
    while b"\n" not in buffer:
        chunk = os.read(0, 1024)
        if not chunk:
            return None, buffer
        buffer += chunk
        if len(buffer) > MAX_LINE:
            return buffer, b""
    line, _, buffer = buffer.partition(b"\n")
    return line, buffer


def serve(journal, turns, ident):
    seq = 0
    buffer = b""
    while True:
        line, buffer = read_line(buffer)
        if line is None:
            return 0
        seq += 1
        text = line.decode("latin-1")
        record = {"event": "read", "seq": seq, "line": text, "hex": line.hex()}
        record.update(ident)
        journal.write(record)
        if seq > len(turns):
            continue
        turn = turns[seq - 1]
        if turn.get("emit") is True and text == turn.get("ask"):
            sys.stdout.write(turn["reply"] + "\n")
            sys.stdout.flush()


def main():
    pid = os.getpid()
    ticks = start_ticks(pid)
    journal_path, turns = load_map()
    journal = Journal(journal_path)
    ident = identity(pid, ticks)
    start = {"event": "start", "argv": sys.argv}
    start.update(ident)
    journal.write(start)
    try:
        error = configure_tty()
    except (OSError, termios.error) as exc:
        error = f"tty_error:{type(exc).__name__}"
    if error is not None:
        journal.write({"event": "error", "error": error, "pid": pid, "startTicks": ticks})
        return 3
    journal.write({"event": "ready", "pid": pid, "startTicks": ticks})
    sys.stdout.write(READY + "\n")
    sys.stdout.flush()
    return serve(journal, turns, ident)


if __name__ == "__main__":
    sys.exit(main())
