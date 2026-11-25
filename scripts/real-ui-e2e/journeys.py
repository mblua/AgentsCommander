"""Real UI journeys J1-J3 for #5145 (P01).

Run as an ordinary user inside a disposable VM, never in a host graphical session:

  /usr/bin/python3 -B scripts/real-ui-e2e/journeys.py --binary <testable ELF> \
    --source-sha <sha> --evidence <new empty dir> [--test-compositor-exit]

J1 creates a project from an empty folder through the GTK portal. J2 opens one local replica
session with the `e2e-probe` agent and ties it to its PTY process. J3 types into xterm and
reads the answer back from a window capture with OCR. On success the evidence folder holds
exactly `result.json`, `ready.png`, `reply.png` and `silent.png`; on failure only
`result.json`. Exit codes: 0 pass, 20 private compositor unavailable, 1 any other failure.
"""

import argparse
import collections
import datetime
import hashlib
import json
import os
import re
import secrets
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import uuid

import desktop as dk

SUITE_S, STARTUP_S, STEP_S, CLOSE_S, OBSERVE_S = 180, 60, 30, 15, 12
EXIT_PASS, EXIT_FAIL, EXIT_COMPOSITOR = 0, 1, 20
DIALOG_TITLE = "Select parent folder for the new agent"
READY = "PROBE READY"
PYTHON = "/usr/bin/python3"
SPAWN_RE = re.compile(r"\[pty\] Spawned session (\S+) with child pid (.*)$")
SPAWN_OK = re.compile(r"^Some\(([1-9][0-9]*)\)$")
LIVE = {"active", "idle", "running"}
WORDS = [
    "ACE", "ARM", "BAY", "BOX", "CAP", "COD", "DEN", "DOT", "ELK", "EMU", "FIG", "FOX",
    "GEM", "GUM", "HAT", "HEN", "INK", "IVY", "JAM", "JET", "KEG", "KIT", "LAB", "LOG",
    "MAP", "MUD", "NET", "NUT", "OAK", "OWL", "PAN", "PEG", "RAG", "RUG", "SAP", "SKY",
    "TAB", "TOP", "URN", "VAN", "WAX", "WEB", "YAK", "ZIP",
]


class Failure(Exception):
    def __init__(self, code, stage):
        super().__init__(f"{stage}:{code}")
        self.code = code
        self.stage = stage


def automation_id_part(value):
    part = re.sub(r"[^a-zA-Z0-9._-]+", "-", value).strip("-")
    return part or "unknown"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def norm(line):
    return " ".join(line.split())


class Run:
    """One run of the suite. Every step records its stage before it starts."""

    def __init__(self, args):
        self.args = args
        self.t0 = time.monotonic()
        self.suite_deadline = self.t0 + SUITE_S
        self.stage = "setup"
        self.root = None
        self.desk = None
        self.app = None
        self.bin = None
        self.probe = None
        self.journal = None
        self.turns = None
        self.main_con = None
        self.session = None
        self.child = None
        self.rect = None
        self.viewport = None
        self.captures = {}
        self.result = {
            "sourceSha": args.source_sha,
            "startedAtUtc": datetime.datetime.now(datetime.timezone.utc)
            .strftime("%Y-%m-%dT%H:%M:%S.%fZ"),
            "journeys": {"J1": "skipped", "J2": "skipped", "J3": "skipped"},
            "cleanup": {"ok": False},
        }

    # ----- helpers ---------------------------------------------------------------------

    def deadline(self, seconds):
        return min(time.monotonic() + seconds, self.suite_deadline)

    def at(self, stage):
        self.stage = stage
        if time.monotonic() >= self.suite_deadline:
            raise Failure("suite_timeout", stage)

    def cli(self, *argv, timeout=STEP_S, check=True):
        """Run the copied product binary; returns (rc, parsed stdout JSON or None)."""
        try:
            result = self.desk.run([self.bin, *argv], timeout=timeout, check=False)
        except subprocess.TimeoutExpired as exc:
            raise Failure("cli_timeout", self.stage) from exc
        try:
            data = json.loads(result.stdout) if result.stdout.strip() else None
        except ValueError:
            data = None
        if check and (result.returncode != 0 or data is None):
            raise Failure(f"cli_failed_{argv[0]}", self.stage)
        return result.returncode, data

    def ui(self, verb, selector, *extra, window="main", check=True, timeout=STEP_S):
        _, reply = self.cli(verb, "--window", window, "--selector", selector, *extra,
                            check=False, timeout=timeout + 5)
        if not isinstance(reply, dict):
            raise Failure("bridge_unreadable", self.stage)
        if check and reply.get("ok") is not True:
            raise Failure(f"bridge_{reply.get('error') or 'error'}", self.stage)
        return reply

    def require_target(self, reply, selector):
        target = reply.get("target") or {}
        if (target.get("testId") != selector or target.get("visible") is not True
                or target.get("disabled") is True):
            raise Failure("target_not_usable", self.stage)
        return target

    def click(self, selector):
        self.require_main_focus()
        self.require_target(self.ui("ui-query", selector), selector)
        # The bridge snapshots the target after the click, so an element the click
        # closes (a menu item) is no longer visible. Usability is checked by the
        # query above; here only the clicked element's identity.
        clicked = self.ui("ui-click", selector).get("target") or {}
        if clicked.get("testId") != selector:
            raise Failure("target_not_usable", self.stage)

    def wait_selector(self, selector, seconds=STEP_S):
        reply = self.ui("ui-wait", selector, "--timeout-ms", str(int(seconds * 1000)),
                        timeout=seconds)
        return self.require_target(reply, selector)

    def poll(self, seconds, predicate, code):
        deadline = self.deadline(seconds)
        while True:
            value = predicate()
            if value:
                return value
            if time.monotonic() >= deadline:
                raise Failure(code, self.stage)
            time.sleep(0.2)

    def require_main_focus(self):
        try:
            self.desk.require_focus(self.main_con, self.app.pid, self.stage)
        except dk.DesktopError as exc:
            raise Failure(exc.code, self.stage) from exc
        if not self.app.alive():
            raise Failure("app_exited", self.stage)

    # ----- setup -----------------------------------------------------------------------

    def setup(self):
        self.at("setup")
        evidence = self.args.evidence
        if not os.path.isdir(evidence) or os.listdir(evidence):
            raise Failure("evidence_not_empty", self.stage)
        self.root = os.path.realpath(tempfile.mkdtemp(prefix="ac5145-", dir="/tmp"))
        if not self.root.isascii() or ".ac" in self.root.split(os.sep):
            raise Failure("root_invalid", self.stage)
        os.makedirs(os.path.join(self.root, "bin"), mode=0o700)
        self.bin = os.path.join(self.root, "bin", "agentscommander_testeable.exe")
        shutil.copy2(self.args.binary, self.bin)
        with open(self.args.binary, "rb") as a, open(self.bin, "rb") as b:
            source, copy = a.read(), b.read()
        if source != copy or not os.access(self.bin, os.X_OK):
            raise Failure("binary_copy_mismatch", self.stage)
        self.result["binary"] = {"sha256": sha256(copy), "bytes": len(copy)}
        probe_dir = os.path.join(self.root, "probe")
        os.makedirs(probe_dir, mode=0o700)
        self.probe = os.path.join(probe_dir, "probe.py")
        here = os.path.dirname(os.path.realpath(__file__))
        shutil.copyfile(os.path.join(here, "probe.py"), self.probe)
        if " " in self.probe:
            raise Failure("probe_path_invalid", self.stage)
        self.journal = os.path.join(probe_dir, "journal.jsonl")
        self.turns = make_turns()
        probe_map = {"turns": self.turns}
        fd = os.open(self.probe + ".json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(probe_map, handle)
        args = [dk.SWAY, "--version"] if self.args.test_compositor_exit else None
        self.desk = dk.Desktop(self.root, compositor_args=args)
        self.desk.prepare()
        self.profile = self.desk.env["AGENTSCOMMANDER_CONFIG_DIR"]

    def start_desktop(self):
        try:
            self.at("bus")
            self.desk.start_bus(self.deadline(STARTUP_S))
            self.at("compositor")
            self.desk.start_compositor(self.deadline(STARTUP_S))
            self.at("portals")
            self.desk.start_portals(self.deadline(STARTUP_S))
        except dk.DesktopError as exc:
            raise Failure(exc.code, exc.stage) from exc

    def report_agent_list(self, agents, listing):
        """Diagnostics only: print the ids and commands `coding-agent list` returned."""
        if isinstance(agents, list):
            rows = [{"id": a.get("id"), "baseCommand": a.get("baseCommand")} if isinstance(a, dict)
                    else {"type": type(a).__name__} for a in agents]
        else:
            rows = {"listingType": type(listing).__name__}
        expected = {"id": "e2e-probe", "baseCommand": f"{PYTHON} {self.probe}"}
        print(json.dumps({"agentList": rows, "expected": expected}), file=sys.stderr)

    def register_agent(self):
        self.at("agent")
        self.cli("coding-agent", "add", "--id", "e2e-probe", "--label", "E2EProbe",
                 "--command", f"{PYTHON} {self.probe}", "--backend", "local")
        _, listing = self.cli("coding-agent", "list")
        agents = listing.get("agents") if isinstance(listing, dict) else listing
        if not isinstance(agents, list) or len(agents) != 1:
            self.report_agent_list(agents, listing)
            raise Failure("agent_list_mismatch", self.stage)
        agent = agents[0]
        if agent.get("id") != "e2e-probe" or agent.get("baseCommand") != f"{PYTHON} {self.probe}":
            self.report_agent_list(agents, listing)
            raise Failure("agent_list_mismatch", self.stage)

    def launch_app(self):
        self.at("app")
        if self.spawn_records():
            raise Failure("spawn_log_not_empty", self.stage)
        self.app = self.desk.spawn("app", [self.bin, "--app", "--ui-automation"],
                                   self.desk.child_env(gui=True), log_name="gui.stderr")
        descriptor = os.path.join(self.profile, "ui-automation", "session.json")

        def ready():
            try:
                with open(descriptor, encoding="utf-8") as handle:
                    data = json.load(handle)
            except (OSError, ValueError):
                return False
            return (data.get("pid") == self.app.pid
                    and os.path.realpath(data.get("exePath", "")) == self.bin
                    and os.path.realpath(data.get("configDir", "")) == self.profile
                    and "main" in (data.get("readyWindowLabels") or []))

        self.poll(STARTUP_S, ready, "app_not_ready")
        self.ui("ui-wait", "main.root", "--timeout-ms", str(STARTUP_S * 1000),
                timeout=STARTUP_S)

        def main_window():
            hits = [w for w in self.desk.windows() if w["pid"] == self.app.pid]
            return hits[0] if len(hits) == 1 and hits[0]["focused"] else None

        self.main_con = self.poll(STEP_S, main_window, "main_window_missing")["conId"]
        self.result["appPid"] = self.app.pid
        self.result["window"] = {"label": "main", "conId": self.main_con}

    # ----- J1 --------------------------------------------------------------------------

    def project_paths(self):
        try:
            with open(os.path.join(self.profile, "project-paths.json"), encoding="utf-8") as h:
                data = json.load(h)
        except FileNotFoundError:
            return []
        except ValueError as exc:
            raise Failure("project_paths_unreadable", self.stage) from exc
        if not isinstance(data, dict):
            raise Failure("project_paths_unreadable", self.stage)
        # Same reading as the app (project_lists_from_projection): the file only
        # holds the keys it has, so a missing or null projectPaths falls back to
        # projectPath.
        paths = data.get("projectPaths")
        if paths is None:
            head = data.get("projectPath")
            paths = [head] if isinstance(head, str) else []
        if not isinstance(paths, list):
            raise Failure("project_paths_unreadable", self.stage)
        return [os.path.realpath(p if isinstance(p, str) else p.get("path", "")) for p in paths]

    def journey_one(self):
        self.at("J1")
        self.project_name = "p5145" + secrets.token_hex(4)
        self.project = os.path.join(self.root, "projects", self.project_name)
        os.makedirs(self.project, mode=0o700)
        canonical = os.path.realpath(self.project)
        if os.listdir(self.project) or canonical in self.project_paths():
            raise Failure("project_not_fresh", self.stage)
        self.p_id = automation_id_part(canonical)
        header = f"project.header.{self.p_id}"
        if self.ui("ui-query", header, check=False).get("error") != "missing_selector":
            raise Failure("project_header_present", self.stage)
        self.click("actionBar.newOpen")
        self.click("actionBar.menu.newProject")
        dialog = self.wait_dialog()
        # A key sent before the chooser takes keyboard input is lost: ctrl+l then never
        # opens the location bar and the typed path lands in the search box instead.
        # A leading "/" also opens that bar, but it takes focus a moment later: keys
        # typed before then still go to the search box. So "/" goes alone, and the rest
        # of the path follows after a pause, one key every 20 ms.
        time.sleep(1.0)
        steps = [(["-M", "ctrl", "l", "-m", "ctrl"], 0.3), (["/"], 1.0),
                 (["-d", "20", canonical.lstrip("/")], 0.3), (["-k", "Return"], 0.3)]
        for step, pause in steps:
            self.dialog_key(step, dialog)
            time.sleep(pause)
        # The chooser may only enter the folder on Return; then the accept button
        # confirms it. Its mnemonic depends on the portal label (_Select or _Open).
        accepts = [("return", ["-k", "Return"]), ("alt_s", ["-M", "alt", "s", "-m", "alt"]),
                   ("alt_o", ["-M", "alt", "o", "-m", "alt"])]
        for name, keys in accepts:
            time.sleep(0.5)
            focus = self.desk.focused()
            if not (focus and focus["conId"] == dialog["conId"]):
                break
            print(f"J1 dialog still open, sending {name}", file=sys.stderr)
            self.dialog_key(keys, dialog)

        def back_in_main():
            focus = self.desk.focused()
            return focus and focus["conId"] == self.main_con and focus["pid"] == self.app.pid

        try:
            self.poll(STEP_S, back_in_main, "main_focus_not_restored")
        except Failure:
            self.dump_views("main_focus_not_restored", self.desk.windows())
            self.dump_screen("j1-dialog")
            raise
        self.wait_selector(header)
        self.poll(STEP_S, lambda: os.path.isdir(os.path.join(canonical, ".ac")),
                  "project_ac_missing")
        if self.project_paths().count(canonical) != 1:
            raise Failure("project_paths_mismatch", self.stage)
        self.result["journeys"]["J1"] = "pass"

    def wait_dialog(self):
        gtk = self.desk.gtk_portal_pid

        seen = []

        def dialog():
            views = self.desk.windows()
            seen[:] = views
            if any(w["pid"] not in (self.app.pid, gtk) for w in views):
                raise Failure("unexpected_dialog_owner", self.stage)
            hits = [w for w in views if w["conId"] != self.main_con]
            if len(hits) != 1:
                return None
            hit = hits[0]
            if hit["focused"] and hit["name"] == DIALOG_TITLE and hit["pid"] == gtk:
                return hit
            return None

        try:
            return self.poll(STEP_S, dialog, "portal_dialog_missing")
        except Failure:
            self.dump_views("portal_dialog_missing", seen)
            raise

    def dump_views(self, code, views):
        # Diagnostic only (job log): which views existed when a wait ended.
        gtk = self.desk.gtk_portal_pid
        roles = {self.app.pid: "app", gtk: "portal"}
        shown = [{"owner": roles.get(w["pid"], "other"), "name": w["name"],
                  "focused": w["focused"], "main": w["conId"] == self.main_con}
                 for w in views]
        print(f"{code}: portalPid={'set' if gtk else 'none'} "
              f"views={json.dumps(shown)}", file=sys.stderr)
        logs = self.desk.path("logs")
        for name in sorted(os.listdir(logs)) if os.path.isdir(logs) else []:
            with open(os.path.join(logs, name), "rb") as h:
                tail = h.read().decode("utf-8", "replace").splitlines()[-40:]
            print(f"--- {name} (last {len(tail)} lines)", file=sys.stderr)
            print("\n".join(tail), file=sys.stderr)

    def dump_screen(self, name):
        # Diagnostic only, on failure: the private compositor's screen and its OCR text,
        # kept under RUNNER_TEMP/real-ui-debug, outside the verified evidence.
        out = os.path.join(os.environ.get("RUNNER_TEMP") or self.root, "real-ui-debug")
        os.makedirs(out, exist_ok=True)
        png = os.path.join(out, name + ".png")
        shot = self.desk.run(["/usr/bin/grim", png], check=False)
        if shot.returncode != 0:
            print(f"{name}: grim failed: {shot.stderr.decode('utf-8', 'replace')}", file=sys.stderr)
            return
        text = self.desk.run(["/usr/bin/tesseract", png, "stdout", "-l", "eng"], check=False)
        print(f"--- {name} OCR\n{text.stdout.decode('utf-8', 'replace')}", file=sys.stderr)

    def dialog_key(self, args, dialog):
        # Each wtype run is a new virtual keyboard; the chooser drops keys sent before
        # it has taken that keyboard's keymap, so every run waits 300 ms first.
        try:
            self.desk.key(["-s", "300", *args], dialog["conId"], self.desk.gtk_portal_pid,
                          self.stage)
        except dk.DesktopError as exc:
            raise Failure(exc.code, self.stage) from exc
        time.sleep(0.2)

    # ----- J2 --------------------------------------------------------------------------

    def session_rows(self):
        _, rows = self.cli("list-sessions")
        if not isinstance(rows, list):
            raise Failure("list_sessions_unreadable", self.stage)
        return rows

    def require_empty_profile(self):
        if self.session_rows():
            raise Failure("profile_not_empty", self.stage)
        path = os.path.join(self.profile, "sessions.json")
        if os.path.exists(path):
            try:
                with open(path, encoding="utf-8") as handle:
                    data = json.load(handle)
            except ValueError as exc:
                raise Failure("sessions_file_invalid", self.stage) from exc
            if data != []:
                raise Failure("sessions_file_not_empty", self.stage)

    def prepare_replica(self):
        name = self.project_name
        _, matrix = self.cli("create-agent-matrix", "--project", name, "--name", "probe",
                             "--description", "PTY probe")
        if matrix.get("launched") is not False or matrix.get("launchStatus") != "skipped":
            raise Failure("matrix_launched", self.stage)
        self.cli("team", "create", "--project", name, "--team", "sample",
                 "--coordinator", "probe")
        _, room = self.cli("room", "add", "--project", name, "--team", "sample",
                           "--title", "PTY check")
        room_path = os.path.realpath(room.get("path", ""))
        if os.path.commonpath([room_path, os.path.realpath(self.project)]) != \
                os.path.realpath(self.project) or room.get("cloneErrors"):
            raise Failure("room_invalid", self.stage)
        replica = os.path.join(room_path, "__agent_probe")
        if not os.path.isdir(replica) or os.path.islink(replica):
            raise Failure("replica_missing", self.stage)
        self.replica = os.path.realpath(replica)
        self.replica_key = sha256(os.fsencode(self.replica))

    def journey_two(self):
        self.at("J2")
        self.require_empty_profile()
        self.prepare_replica()
        self.require_empty_profile()
        self.result["profile"] = {"before": []}
        toggle = f"project.header.{self.p_id}.toggle"
        if self.wait_selector(toggle).get("expanded") is False:
            self.click(toggle)
        row = f"replica.row.quick.{automation_id_part(self.replica)}"
        self.wait_selector(row)
        self.click(row)
        self.wait_selector("agentPicker.modal")
        self.click("agentPicker.provider.e2e-probe")
        self.require_empty_profile()
        if self.spawn_records():
            raise Failure("spawn_before_apply", self.stage)
        self.click("agentPicker.apply")
        self.session = self.poll(STEP_S, self.single_session, "session_not_created")
        self.child = self.poll(STEP_S, self.single_spawn, "spawn_record_missing")
        self.verify_child()
        self.poll(STEP_S, lambda: self.probe_events("start"), "probe_not_started")
        self.verify_identity()
        self.terminal_state()
        self.ready_capture()
        self.result["journeys"]["J2"] = "pass"

    def single_session(self):
        rows = self.session_rows()
        if not rows:
            return None
        ids = [r.get("id") for r in rows]
        if collections.Counter(ids) != collections.Counter(ids[:1]) or len(rows) != 1:
            raise Failure("session_delta_mismatch", self.stage)
        row = rows[0]
        try:
            valid = str(uuid.UUID(row["id"])) == row["id"]
        except (KeyError, ValueError, TypeError):
            valid = False
        status = row.get("status")
        tag = status if isinstance(status, str) else next(iter(status or {}), "")
        if (not valid or os.path.realpath(row.get("workingDirectory", "")) != self.replica
                or str(tag).lower() not in LIVE):
            raise Failure("session_row_invalid", self.stage)
        return row["id"]

    def spawn_records(self):
        path = os.path.join(self.root, "logs", "gui.stderr")
        if not os.path.exists(path):
            return []
        with open(path, "rb") as handle:
            text = handle.read().decode("utf-8", "replace")
        records = []
        for line in text.splitlines():
            if "[pty] Spawned session" not in line:
                continue
            match = SPAWN_RE.search(line)
            if not match or "pty::local_backend" not in line:
                raise Failure("spawn_record_malformed", self.stage)
            records.append(match.groups())
        return records

    def single_spawn(self):
        records = self.spawn_records()
        if not records:
            return None
        if len(records) != 1 or records[0][0] != self.session:
            raise Failure("spawn_count_mismatch", self.stage)
        match = SPAWN_OK.match(records[0][1].strip())
        if not match:
            raise Failure("spawn_pid_invalid", self.stage)
        return int(match.group(1))

    def verify_child(self):
        pid = self.child
        try:
            exe = os.path.realpath(f"/proc/{pid}/exe")
            with open(f"/proc/{pid}/cmdline", "rb") as handle:
                argv = handle.read().split(b"\0")[:-1]
            with open(f"/proc/{pid}/status", encoding="ascii") as handle:
                ppid = int(next(r for r in handle if r.startswith("PPid:")).split()[1])
            cwd = os.path.realpath(f"/proc/{pid}/cwd")
            devs = [os.stat(f"/proc/{pid}/fd/{fd}") for fd in (0, 1)]
        except (OSError, StopIteration, ValueError) as exc:
            raise Failure("child_unreadable", self.stage) from exc
        python = os.path.realpath(PYTHON)
        if exe != python or argv != [PYTHON.encode(), self.probe.encode()]:
            raise Failure("child_argv_mismatch", self.stage)
        if ppid != self.app.pid or cwd != self.replica:
            raise Failure("child_parent_mismatch", self.stage)
        if not all(stat.S_ISCHR(d.st_mode) for d in devs) \
                or devs[0].st_rdev != devs[1].st_rdev:
            raise Failure("child_tty_mismatch", self.stage)
        self.ticks = dk.proc_start_ticks(pid)
        rdev = devs[0].st_rdev
        self.tty = [os.major(rdev), os.minor(rdev)]
        if self.live_fixture_pids() != {pid}:
            raise Failure("fixture_process_set_mismatch", self.stage)

    def live_fixture_pids(self):
        want = PYTHON.encode() + b"\0" + self.probe.encode() + b"\0"
        found = set()
        for name in os.listdir("/proc"):
            if not name.isdigit():
                continue
            try:
                if os.stat(f"/proc/{name}").st_uid != os.getuid():
                    continue
                with open(f"/proc/{name}/cmdline", "rb") as handle:
                    if handle.read() == want:
                        found.add(int(name))
            except OSError:
                continue
        return found

    def probe_events(self, kind=None):
        try:
            with open(self.journal, encoding="utf-8") as handle:
                events = [json.loads(line) for line in handle if line.strip()]
        except FileNotFoundError:
            return []
        except ValueError as exc:
            raise Failure("journal_unreadable", self.stage) from exc
        return [e for e in events if kind is None or e.get("event") == kind]

    def same_identity(self, event):
        return (event.get("pid") == self.child and event.get("startTicks") == self.ticks
                and event.get("ttyDev") == self.tty
                and event.get("replicaKey") == self.replica_key)

    def verify_identity(self):
        """Session, spawn, process set and journal still describe one probe."""
        if self.single_session() != self.session or self.single_spawn() != self.child:
            raise Failure("session_identity_changed", self.stage)
        if dk.proc_start_ticks(self.child) != self.ticks:
            raise Failure("child_identity_changed", self.stage)
        if self.live_fixture_pids() != {self.child}:
            raise Failure("fixture_process_set_mismatch", self.stage)
        starts = self.probe_events("start")
        if len(starts) != 1 or not self.same_identity(starts[0]):
            raise Failure("probe_start_mismatch", self.stage)
        if self.probe_events("error"):
            raise Failure("probe_tty_error", self.stage)

    def terminal_state(self):
        _, reply = self.cli("ui-terminal", "--window", "main", "--session-id", self.session,
                            "query", check=False)
        target = (reply or {}).get("target") or {}
        if (not isinstance(reply, dict) or reply.get("ok") is not True
                or target.get("sessionId") != self.session
                or not target.get("cols") or not target.get("rows")
                or target.get("atBottom") is not True):
            raise Failure("terminal_not_ready", self.stage)
        selector = f"terminal.session.{self.session}"
        query = self.ui("ui-query", selector)
        element = self.require_target(query, selector)
        diag = query.get("diagnostics") or {}
        state = {"rect": element.get("rect"), "viewport": diag.get("viewport"),
                 "dpr": diag.get("devicePixelRatio"), "rows": target["rows"]}
        if state["dpr"] != 1 or not state["rect"] or not state["viewport"]:
            raise Failure("terminal_geometry_invalid", self.stage)
        if self.rect is None:
            if target["rows"] < 6:
                raise Failure("terminal_too_small", self.stage)
            self.rect, self.viewport = state["rect"], state["viewport"]
        elif state["rect"] != self.rect or state["viewport"] != self.viewport:
            raise Failure("terminal_geometry_changed", self.stage)
        return target

    # ----- capture and OCR -------------------------------------------------------------

    def sample(self):
        """One fresh, verified capture of the terminal: (crop bytes, OCR lines)."""
        self.terminal_state()
        self.require_main_focus()
        frame = os.path.join(self.root, f"frame-{secrets.token_hex(6)}.png")
        try:
            png, size = self.desk.snapshot(self.bin, frame, self.main_con, self.app.pid,
                                           self.stage)
            if [size[0], size[1]] != [self.viewport["width"], self.viewport["height"]]:
                raise Failure("snapshot_viewport_mismatch", self.stage)
            crop = self.desk.crop(png, self.rect, size)
            lines = self.desk.ocr(crop, os.path.join(self.root, "ocr-input.png"))
        except dk.DesktopError as exc:
            raise Failure(exc.code, self.stage) from exc
        self.terminal_state()
        self.require_main_focus()
        return crop, lines

    def known(self):
        return [READY] + [t[k] for t in self.turns for k in ("ask", "reply")]

    def classify(self, lines):
        """Map OCR lines to known fixture texts; any unknown line invalidates the read."""
        texts = self.known()
        seen = []
        for line in lines:
            if line not in texts:
                raise Failure("ocr_unrecognized_line", self.stage)
            seen.append(line)
        return seen

    def record(self, role, crop):
        digest = sha256(crop)
        self.captures[role] = crop
        return {"sessionId": self.session, "childPid": self.child, "conId": self.main_con,
                "file": f"{role}.png", "sha256": digest}

    def ready_capture(self):
        def greeting():
            crop, lines = self.sample()
            return (crop, lines) if READY in lines else None

        crop, lines = self.poll(STEP_S, greeting, "greeting_not_visible")
        if self.classify(lines) != [READY]:
            raise Failure("ready_profile_mismatch", self.stage)
        self.result["ready"] = self.record("ready", crop)

    # ----- J3 --------------------------------------------------------------------------

    def journey_three(self):
        self.at("J3")
        if self.ui("ui-query", "agentPicker.modal", check=False).get("error") \
                != "missing_selector":
            raise Failure("picker_still_open", self.stage)
        observations = []
        for index, case in enumerate(("reply", "silent")):
            observations.append(self.challenge(index, case))
        self.result["observations"] = observations
        self.verify_identity()
        if len(self.probe_events("read")) != 2:
            raise Failure("probe_read_count_mismatch", self.stage)
        self.result["control"] = {"samePty": True, "observedError": observations[1]["outcome"]}
        self.result["journeys"]["J3"] = "pass"

    def challenge(self, index, case):
        self.stage = f"J3.{case}"
        turn = self.turns[index]
        ask, want = turn["ask"], turn["reply"]
        self.verify_identity()
        _, lines = self.sample()
        if READY not in lines or want in lines or ask in lines:
            raise Failure("precondition_capture_mismatch", self.stage)
        self.require_main_focus()
        try:
            self.desk.key(["z", "-P", "BackSpace", "-p", "BackSpace", ask,
                           "-P", "Return", "-p", "Return"],
                          self.main_con, self.app.pid, self.stage)
        except dk.DesktopError as exc:
            raise Failure(exc.code, self.stage) from exc
        deadline = time.monotonic() + OBSERVE_S
        receipt = self.wait_receipt(index + 1, ask, deadline)
        if case == "reply":
            crop = self.observe_reply(ask, want, deadline)
            outcome = "visible_reply"
        else:
            crop = self.observe_silent(ask, want, deadline)
            outcome = "missing_visible_reply"
        self.verify_identity()
        capture = self.record(case, crop)
        return {"case": case, "ask": ask, "want": want, "outcome": outcome,
                "receipt": receipt, "capture": capture}

    def wait_receipt(self, seq, ask, deadline):
        while True:
            reads = self.probe_events("read")
            if len(reads) > seq:
                raise Failure("unexpected_receipt", self.stage)
            if len(reads) == seq:
                event = reads[-1]
                if event.get("seq") != seq or not self.same_identity(event):
                    raise Failure("receipt_identity_mismatch", self.stage)
                if event.get("line") != ask:
                    raise Failure("input_mismatch", self.stage)
                return {"seq": seq, "line": event["line"], "pid": event["pid"],
                        "startTicks": event["startTicks"], "ttyDev": event["ttyDev"],
                        "replicaKey": event["replicaKey"]}
            if time.monotonic() >= deadline:
                raise Failure("receipt_timeout", self.stage)
            time.sleep(0.1)

    def observe_reply(self, ask, want, deadline):
        expected = [READY, ask, want]
        while True:
            crop, lines = self.sample()
            seen = self.classify(lines)
            if seen == expected:
                return crop
            if seen != expected[:len(seen)]:
                raise Failure("reply_profile_mismatch", self.stage)
            if time.monotonic() >= deadline:
                raise Failure("reply_timeout", self.stage)

    def observe_silent(self, ask, want, deadline):
        first = self.turns[0]
        expected = [READY, first["ask"], first["reply"], ask]
        last = None
        while True:
            crop, lines = self.sample()
            seen = self.classify(lines)
            if want in seen:
                raise Failure("silent_reply_visible", self.stage)
            if seen != expected[:len(seen)]:
                raise Failure("silent_profile_mismatch", self.stage)
            last = (crop, seen)
            if time.monotonic() >= deadline:
                break
            time.sleep(0.5)
        if last[1] != expected:
            raise Failure("silent_final_unreadable", self.stage)
        return last[0]

    # ----- cleanup and result ----------------------------------------------------------

    def stop_probe(self):
        if not self.child or not dk.proc_alive(self.child, self.ticks):
            return True
        try:
            os.kill(self.child, signal.SIGTERM)
        except ProcessLookupError:
            return True
        end = time.monotonic() + 5
        while time.monotonic() < end:
            if not dk.proc_alive(self.child, self.ticks):
                return True
            time.sleep(0.1)
        if dk.proc_alive(self.child, self.ticks):
            os.kill(self.child, signal.SIGKILL)
        return not dk.proc_alive(self.child, self.ticks)

    def cleanup(self, failed):
        ok = True
        try:
            final = self.final_counts() if self.child else None
            ok = self.stop_probe() and ok
            if self.desk is not None:
                ok = self.desk.close(CLOSE_S) and ok
                pngs = self.count_pngs()
                self.result["isolation"] = {
                    "swayExit": self.desk.sway_exit,
                    "waylandSocketSeen": self.desk.wayland_seen,
                    "keyCalls": self.desk.key_calls,
                    "snapshotCalls": self.desk.snapshot_calls,
                    "pngFiles": pngs,
                }
            if final is not None and final != (1, 1):
                ok = False
                self.result["cleanup"]["countMismatch"] = True
        except (OSError, Failure, dk.DesktopError):
            ok = False
        finally:
            if self.root:
                shutil.rmtree(self.root, ignore_errors=True)
                ok = ok and not os.path.exists(self.root)
        self.result["cleanup"]["ok"] = ok
        return ok

    def final_counts(self):
        try:
            spawns = len(self.spawn_records())
        except Failure:
            spawns = -1
        starts = len(self.probe_events("start"))
        return spawns, starts

    def count_pngs(self):
        count = 0
        for base in (self.root, self.args.evidence):
            for _, _, files in os.walk(base or "/nonexistent"):
                count += sum(1 for f in files if f.lower().endswith(".png"))
        return count

    def finish_launch(self):
        self.result["profile"]["after"] = [self.single_session()]
        self.result["sessionId"] = self.session
        self.result["launch"] = {
            "sessionId": self.session, "childPid": self.child, "startTicks": self.ticks,
            "ttyDev": self.tty, "replicaKey": self.replica_key,
            "spawnCount": len(self.spawn_records()),
            "probeStarts": len(self.probe_events("start")),
        }

    def write_evidence(self, passed):
        self.result["durationMs"] = int((time.monotonic() - self.t0) * 1000)
        evidence = self.args.evidence
        if passed:
            for role, crop in self.captures.items():
                with open(os.path.join(evidence, f"{role}.png"), "wb") as handle:
                    handle.write(crop)
        with open(os.path.join(evidence, "result.json"), "w", encoding="utf-8") as handle:
            json.dump(self.result, handle, indent=2, sort_keys=True)
            handle.write("\n")


def make_turns():
    words = secrets.SystemRandom().sample(WORDS, 14)
    ask1, ask2 = " ".join(words[0:3]), " ".join(words[3:6])
    reply1, reply2 = " ".join(words[6:10]), " ".join(words[10:14])
    return [{"ask": f"ASK {ask1}", "reply": f"REPLY {reply1}", "emit": True},
            {"ask": f"ASK {ask2}", "reply": f"REPLY {reply2}", "emit": False}]


def execute(run):
    """Run the stages; returns the exit code. Never builds a result for skipped work."""
    run.setup()
    run.start_desktop()
    run.register_agent()
    run.launch_app()
    current = "J1"
    try:
        run.journey_one()
        current = "J2"
        run.journey_two()
        current = "J3"
        run.journey_three()
        run.finish_launch()
    except Failure:
        run.result["journeys"][current] = "fail"
        raise
    return EXIT_PASS


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--evidence", required=True)
    parser.add_argument("--test-compositor-exit", action="store_true")
    args = parser.parse_args(argv)
    os.umask(0o077)
    run = Run(args)

    def on_signal(signum, _frame):
        raise Failure(f"signal_{signal.Signals(signum).name.lower()}", run.stage)

    signal.signal(signal.SIGTERM, on_signal)
    signal.signal(signal.SIGINT, on_signal)
    code = EXIT_FAIL
    failed = True
    try:
        code = execute(run)
        failed = False
    except Failure as exc:
        run.result["error"], run.result["stage"] = exc.code, exc.stage
        if exc.code == "private_compositor_unavailable":
            code = EXIT_COMPOSITOR
    except Exception as exc:  # noqa: BLE001 - recorded as a sanitized code
        run.result["error"] = f"internal_{type(exc).__name__}"
        run.result["stage"] = run.stage
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        if not run.cleanup(failed) and code == EXIT_PASS:
            code = EXIT_FAIL
            failed = True
            run.result["error"], run.result["stage"] = "cleanup_failed", "cleanup"
        run.write_evidence(not failed)
    return code


if __name__ == "__main__":
    sys.exit(main())
