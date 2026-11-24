"""Private desktop for the #5145 real UI journeys (P01).

Owns the session bus, the headless Sway compositor, the portals, focus checks, the virtual
keyboard (wtype), window captures, OCR and cleanup. Nothing here touches the host session:
the child environment is built from an explicit list, inherited DISPLAY/WAYLAND_DISPLAY/
SWAYSOCK/bus values are dropped, and every socket is resolved inside the private runtime.
"""

import io
import json
import os
import signal
import socket
import struct
import subprocess
import time

SYSTEM_PATH = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
SWAY = "/usr/bin/sway"
PORTAL = "/usr/libexec/xdg-desktop-portal"
PORTAL_GTK = "/usr/libexec/xdg-desktop-portal-gtk"
PORTAL_NAME = "org.freedesktop.portal.Desktop"
PORTAL_GTK_NAME = "org.freedesktop.impl.portal.desktop.gtk"
OUTPUT = "HEADLESS-1"
OUTPUT_SIZE = (1600, 1000)


class DesktopError(Exception):
    """A failure with a sanitized error code and the stage it happened in."""

    def __init__(self, code, stage, detail=""):
        super().__init__(f"{stage}:{code}")
        self.code = code
        self.stage = stage
        self.detail = detail


def proc_start_ticks(pid):
    try:
        with open(f"/proc/{pid}/stat", "rb") as handle:
            raw = handle.read().decode("ascii", "replace")
    except OSError:
        return None
    return int(raw[raw.rindex(")") + 2:].split()[19])


def proc_alive(pid, ticks):
    return ticks is not None and proc_start_ticks(pid) == ticks


def peer_cred(path):
    """(pid, uid, gid) of the process listening on a Unix socket."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.settimeout(2)
        sock.connect(path)
        raw = sock.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i"))
        return struct.unpack("3i", raw)
    finally:
        sock.close()


class Owned:
    """A child process started by this run, remembered with its start ticks."""

    def __init__(self, name, popen, log_path):
        self.name = name
        self.popen = popen
        self.pid = popen.pid
        self.ticks = proc_start_ticks(popen.pid)
        self.log_path = log_path

    def alive(self):
        return self.popen.poll() is None and proc_alive(self.pid, self.ticks)


class Desktop:
    """The private desktop. Call the steps in order; `close()` always runs at the end."""

    def __init__(self, root, compositor_args=None):
        self.root = root
        self.uid = os.getuid()
        self.compositor_args = compositor_args or [SWAY, "--config", self.path("sway.conf")]
        self.children = []
        self.env = None
        self.bus_address = None
        self.wayland = None
        self.swaysock = None
        self.sway = None
        self.portal_pids = {}
        self.key_calls = 0
        self.snapshot_calls = 0
        self.sway_exit = None
        self.wayland_seen = False

    # ----- environment -----------------------------------------------------------------

    def path(self, *parts):
        return os.path.join(self.root, *parts)

    def prepare(self):
        dirs = {
            "HOME": "home",
            "XDG_DATA_HOME": "xdg/data",
            "XDG_CONFIG_HOME": "xdg/config",
            "XDG_CACHE_HOME": "xdg/cache",
            "XDG_STATE_HOME": "xdg/state",
            "XDG_RUNTIME_DIR": "run",
            "AGENTSCOMMANDER_CONFIG_DIR": "profile",
        }
        env = {"PATH": SYSTEM_PATH, "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8"}
        for key, rel in dirs.items():
            full = self.path(rel)
            os.makedirs(full, mode=0o700, exist_ok=True)
            env[key] = full
        runtime = env["XDG_RUNTIME_DIR"]
        os.chmod(runtime, 0o700)
        info = os.stat(runtime)
        if info.st_uid != self.uid or info.st_mode & 0o777 != 0o700:
            raise DesktopError("runtime_dir_invalid", "environment")
        env["XDG_DATA_DIRS"] = "/usr/local/share:/usr/share"
        env["XDG_CONFIG_DIRS"] = "/etc/xdg"
        env["XDG_SESSION_TYPE"] = "wayland"
        env["XDG_CURRENT_DESKTOP"] = "sway"
        self.env = env
        self.write_configs()
        return env

    def write_configs(self):
        width, height = OUTPUT_SIZE
        sway_conf = "\n".join([
            "xwayland disable",
            f"output {OUTPUT} resolution {width}x{height} scale 1 position 0 0",
            "default_border none",
            "focus_on_window_activation focus",
            "",
        ])
        with open(self.path("sway.conf"), "w", encoding="utf-8") as handle:
            handle.write(sway_conf)
        portal_dir = os.path.join(self.env["XDG_CONFIG_HOME"], "xdg-desktop-portal")
        os.makedirs(portal_dir, mode=0o700, exist_ok=True)
        conf = "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=gtk\n"
        with open(os.path.join(portal_dir, "portals.conf"), "w", encoding="utf-8") as handle:
            handle.write(conf)

    def child_env(self, gui=False):
        """Explicit environment for children; only the private destinations are published."""
        env = dict(self.env)
        if self.bus_address:
            env["DBUS_SESSION_BUS_ADDRESS"] = self.bus_address
        if self.wayland:
            env["WAYLAND_DISPLAY"] = self.wayland
        if self.swaysock:
            env["SWAYSOCK"] = self.swaysock
        if gui:
            env.update({
                "GDK_BACKEND": "wayland",
                "GDK_SCALE": "1",
                "GDK_DPI_SCALE": "1",
                "LIBGL_ALWAYS_SOFTWARE": "1",
                "WEBKIT_DISABLE_DMABUF_RENDERER": "1",
                "WEBKIT_DISABLE_COMPOSITING_MODE": "1",
                "GTK_USE_PORTAL": "1",
                "RUST_LOG": "info",
            })
        return env

    def spawn(self, name, argv, env, log_name=None):
        log_path = self.path("logs", log_name or f"{name}.log")
        os.makedirs(os.path.dirname(log_path), mode=0o700, exist_ok=True)
        log = open(log_path, "wb")  # noqa: SIM115 - closed with the run
        try:
            popen = subprocess.Popen(
                argv, env=env, cwd=self.root, stdin=subprocess.DEVNULL, stdout=log,
                stderr=subprocess.STDOUT, start_new_session=True, close_fds=True,
            )
        finally:
            log.close()
        child = Owned(name, popen, log_path)
        self.children.append(child)
        return child

    def run(self, argv, gui=False, timeout=30, check=True):
        result = subprocess.run(
            argv, env=self.child_env(gui), cwd=self.root, stdin=subprocess.DEVNULL,
            capture_output=True, timeout=timeout, check=False,
        )
        if check and result.returncode != 0:
            raise DesktopError("command_failed", "desktop", os.path.basename(argv[0]))
        return result

    # ----- bus -------------------------------------------------------------------------

    def start_bus(self, deadline):
        bus_path = self.path("run", "bus")
        address = f"unix:path={bus_path}"
        self.spawn("dbus", ["/usr/bin/dbus-daemon", "--session", "--nofork",
                            f"--address={address}"], self.child_env())
        wait_until(deadline, lambda: os.path.exists(bus_path), "bus_unavailable", "bus")
        pid, uid, _ = peer_cred(bus_path)
        bus = self.children[-1]
        if uid != self.uid or pid != bus.pid or not bus.alive():
            raise DesktopError("bus_identity_mismatch", "bus")
        self.bus_address = address

    def bus_call(self, method, arg):
        result = self.run([
            "/usr/bin/dbus-send", "--session", "--print-reply=literal",
            "--dest=org.freedesktop.DBus", "/org/freedesktop/DBus",
            f"org.freedesktop.DBus.{method}", f"string:{arg}",
        ], check=False, timeout=5)
        if result.returncode != 0:
            return None
        return result.stdout.decode("ascii", "replace").split()[-1]

    def name_owner_pid(self, name):
        owner = self.bus_call("GetNameOwner", name)
        if owner is None:
            return None
        pid = self.bus_call("GetConnectionUnixProcessID", owner)
        return int(pid) if pid and pid.isdigit() else None

    # ----- compositor ------------------------------------------------------------------

    def start_compositor(self, deadline):
        env = self.child_env()
        env.update({
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_LIBINPUT_NO_DEVICES": "1",
            "WLR_RENDERER": "pixman",
        })
        try:
            self.sway = self.spawn("sway", list(self.compositor_args), env)
        except OSError as exc:
            raise DesktopError("private_compositor_unavailable", "compositor") from exc
        runtime = self.env["XDG_RUNTIME_DIR"]

        def ready():
            if self.sway.popen.poll() is not None:
                return True
            return self.find_sockets(runtime) is not None

        wait_until(deadline, ready, "private_compositor_unavailable", "compositor")
        found = self.find_sockets(runtime)
        if self.sway.popen.poll() is not None or found is None:
            self.sway_exit = self.sway.popen.wait()
            self.wayland_seen = found is not None or self.any_wayland(runtime)
            raise DesktopError("private_compositor_unavailable", "compositor")
        self.wayland_seen = True
        self.wayland, self.swaysock = found
        self.verify_compositor()

    @staticmethod
    def any_wayland(runtime):
        return any(n.startswith("wayland-") and not n.endswith(".lock")
                   for n in os.listdir(runtime))

    def find_sockets(self, runtime):
        names = os.listdir(runtime)
        waylands = [n for n in names if n.startswith("wayland-") and not n.endswith(".lock")]
        ipc = [n for n in names if n.startswith("sway-ipc.") and n.endswith(".sock")]
        if len(waylands) != 1 or len(ipc) != 1:
            return None
        return waylands[0], os.path.join(runtime, ipc[0])

    def verify_compositor(self):
        """UID, live Sway PID, socket owners and the single active output."""
        if not self.sway.alive():
            raise DesktopError("compositor_identity_mismatch", "compositor")
        runtime = self.env["XDG_RUNTIME_DIR"]
        for sock in (os.path.join(runtime, self.wayland), self.swaysock):
            if os.path.dirname(os.path.realpath(sock)) != os.path.realpath(runtime):
                raise DesktopError("compositor_socket_outside_runtime", "compositor")
            pid, uid, _ = peer_cred(sock)
            if pid != self.sway.pid or uid != self.uid:
                raise DesktopError("compositor_identity_mismatch", "compositor")
        outputs = self.swaymsg("get_outputs")
        active = [o for o in outputs if o.get("active")]
        if len(active) != 1 or active[0].get("name") != OUTPUT:
            raise DesktopError("compositor_output_mismatch", "compositor")
        mode = active[0].get("current_mode") or {}
        size = (mode.get("width"), mode.get("height"))
        if size != OUTPUT_SIZE or active[0].get("scale") != 1:
            raise DesktopError("compositor_output_mismatch", "compositor")

    def swaymsg(self, kind):
        result = self.run(["/usr/bin/swaymsg", "-s", self.swaysock, "-r", "-t", kind],
                          timeout=5, check=False)
        if result.returncode != 0:
            raise DesktopError("compositor_ipc_failed", "compositor")
        return json.loads(result.stdout)

    # ----- portals ---------------------------------------------------------------------

    def start_portals(self, deadline):
        env = self.child_env()
        gtk = self.spawn("portal-gtk", [PORTAL_GTK, "--replace"], env)
        portal = self.spawn("portal", [PORTAL, "--replace"], env)
        expected = {PORTAL_NAME: portal, PORTAL_GTK_NAME: gtk}

        def owned():
            return all(self.name_owner_pid(n) == c.pid for n, c in expected.items())

        wait_until(deadline, owned, "portal_unavailable", "portals")
        for name, child in expected.items():
            if not child.alive():
                raise DesktopError("portal_identity_mismatch", "portals")
            self.portal_pids[name] = child.pid

    def verify_portals(self):
        for name, pid in self.portal_pids.items():
            if self.name_owner_pid(name) != pid:
                raise DesktopError("portal_identity_mismatch", "portals")

    @property
    def gtk_portal_pid(self):
        return self.portal_pids.get(PORTAL_GTK_NAME)

    # ----- windows and focus -----------------------------------------------------------

    def windows(self):
        """Flat list of Sway view nodes: {conId, pid, name, focused}."""
        found = []

        def walk(node):
            if node.get("pid") is not None and node.get("type") in ("con", "floating_con"):
                found.append({"conId": node["id"], "pid": node["pid"],
                              "name": node.get("name") or "", "focused": node.get("focused")})
            for child in node.get("nodes", []) + node.get("floating_nodes", []):
                walk(child)

        walk(self.swaymsg("get_tree"))
        return found

    def focused(self):
        hits = [w for w in self.windows() if w["focused"]]
        return hits[0] if len(hits) == 1 else None

    def require_focus(self, con_id, pid, stage):
        """Re-check environment, live processes and the focused destination."""
        self.verify_compositor()
        self.verify_portals()
        focus = self.focused()
        if focus is None or focus["conId"] != con_id or focus["pid"] != pid:
            raise DesktopError("focus_lost", stage)
        return focus

    # ----- sensitive edges -------------------------------------------------------------

    def key(self, args, con_id, pid, stage):
        """One wtype action after a focus check. Counted right before dispatch."""
        self.require_focus(con_id, pid, stage)
        self.key_calls += 1
        result = self.run(["/usr/bin/wtype", *args], timeout=10, check=False)
        if result.returncode != 0:
            raise DesktopError("key_failed", stage)

    def snapshot(self, binary, frame, con_id, pid, stage):
        """window.snapshot into a new private file; returns the full PNG bytes."""
        self.require_focus(con_id, pid, stage)
        if os.path.exists(frame):
            raise DesktopError("snapshot_path_exists", stage)
        self.snapshot_calls += 1
        result = self.run([binary, "ui-backend", "--selector", "window.snapshot",
                           "--value", frame], gui=True, timeout=30, check=False)
        try:
            reply = json.loads(result.stdout)
            diag = reply.get("diagnostics") or {}
            ok = (result.returncode == 0 and reply.get("ok") is True
                  and diag.get("window") == "main" and diag.get("source") == "webkitgtk"
                  and diag.get("path") == frame)
            with open(frame, "rb") as handle:
                png = handle.read()
            ok = ok and diag.get("bytes") == len(png)
            size = (diag.get("width"), diag.get("height"))
        except (OSError, ValueError, AttributeError):
            ok, png, size = False, b"", (None, None)
        finally:
            remove_quiet(frame)
        if not ok:
            raise DesktopError("snapshot_failed", stage)
        return png, size

    # ----- capture and OCR -------------------------------------------------------------

    @staticmethod
    def crop(png, rect, size):
        """Crop the CSS rect, scale 2x and encode once. Returns the PNG bytes."""
        from PIL import Image  # noqa: PLC0415 - only needed once the GUI runs

        image = Image.open(io.BytesIO(png))
        image.load()
        if image.size != tuple(size):
            raise DesktopError("snapshot_size_mismatch", "capture")
        left, top = int(rect["x"]), int(rect["y"])
        right, bottom = int(rect["x"] + rect["width"]), int(rect["y"] + rect["height"])
        if left < 0 or top < 0 or right > image.size[0] or bottom > image.size[1]:
            raise DesktopError("crop_outside_viewport", "capture")
        part = image.convert("RGB").crop((left, top, right, bottom))
        part = part.resize((part.size[0] * 2, part.size[1] * 2), Image.Resampling.LANCZOS)
        out = io.BytesIO()
        part.save(out, format="PNG")
        return out.getvalue()

    def ocr(self, crop_png, scratch):
        """tesseract on exactly these bytes; returns normalized non-empty lines."""
        with open(scratch, "wb") as handle:
            handle.write(crop_png)
        try:
            result = self.run(["/usr/bin/tesseract", scratch, "stdout", "-l", "eng",
                               "--psm", "6"], timeout=30, check=False)
        finally:
            remove_quiet(scratch)
        if result.returncode != 0:
            raise DesktopError("ocr_failed", "capture")
        text = result.stdout.decode("utf-8", "replace")
        return [" ".join(line.split()) for line in text.splitlines() if line.strip()]

    # ----- cleanup ---------------------------------------------------------------------

    def close(self, timeout=15):
        """Stop own children (newest first) whose identity still matches."""
        ok = True
        deadline = time.monotonic() + timeout
        for child in reversed(self.children):
            if child.popen.poll() is not None:
                continue
            if not proc_alive(child.pid, child.ticks):
                ok = False
                continue
            ok = stop_group(child, deadline) and ok
        if self.sway is not None and self.sway_exit is None:
            self.sway_exit = self.sway.popen.poll()
        return ok


def stop_group(child, deadline):
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            if proc_alive(child.pid, child.ticks):
                os.killpg(child.pid, sig)
        except ProcessLookupError:
            return True
        except PermissionError:
            return False
        remaining = max(0.5, deadline - time.monotonic()) if sig == signal.SIGTERM else 5
        try:
            child.popen.wait(timeout=remaining)
            return True
        except subprocess.TimeoutExpired:
            continue
    return False


def wait_until(deadline, predicate, code, stage, interval=0.1):
    while True:
        try:
            if predicate():
                return
        except (OSError, ValueError, DesktopError):
            pass
        if time.monotonic() >= deadline:
            raise DesktopError(code, stage)
        time.sleep(interval)


def remove_quiet(path):
    try:
        os.remove(path)
    except FileNotFoundError:
        pass
