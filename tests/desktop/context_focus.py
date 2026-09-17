"""Context regressions: cold Chromium accessibility and X11 keyboard grabs."""
import ctypes
from contextlib import contextmanager
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request

from common import REPO, active_window, command, find_window, focus, stop_process, wait_for


@contextmanager
def keyboard_grab():
    x = ctypes.CDLL("libX11.so.6")
    x.XOpenDisplay.restype = ctypes.c_void_p
    x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x.XDefaultRootWindow.restype = ctypes.c_ulong
    x.XGrabKeyboard.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
    x.XUngrabKeyboard.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    x.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
    x.XCloseDisplay.argtypes = [ctypes.c_void_p]
    display = x.XOpenDisplay(None)
    assert display
    try:
        assert x.XGrabKeyboard(display, x.XDefaultRootWindow(display), 0, 1, 1, 0) == 0
        x.XSync(display, 0)
        time.sleep(0.1)
        yield
    finally:
        x.XUngrabKeyboard(display, 0)
        x.XSync(display, 0)
        x.XCloseDisplay(display)


class Probe:
    def __init__(self, root):
        self.root = root
        self.seen = set()
        self.process = subprocess.Popen([str(REPO / "target/debug/examples/capture_context"), str(root), "--listen"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=(root.parent / "probe.log").open("w"), text=True)
        assert self.process.stdout.readline().strip() == "ready"

    def capture(self):
        self.process.stdin.write("capture\n")
        self.process.stdin.flush()
        def report():
            if self.process.poll() is not None:
                raise RuntimeError("Context probe exited")
            paths = set(self.root.glob("*/context.json")) - self.seen
            return next(iter(paths)) if paths else None
        path = wait_for(report, timeout=8)
        self.seen.add(path)
        return json.loads(path.read_text())

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.write("quit\n")
            self.process.stdin.flush()
            self.process.wait(timeout=5)


def main():
    parent = REPO / "target/desktop-tests"
    parent.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="context-focus-", dir=parent))
    print("Artifacts:", root, flush=True)
    original = active_window()
    original_mouse = dict(line.split("=") for line in command("xdotool", "getmouselocation", "--shell").decode().splitlines())
    cleanup = []
    results = []
    probe = Probe(root / "reports")
    env = {**os.environ, "NO_AT_BRIDGE": "0"}
    def check(name, snapshot):
        assert snapshot["status"] == "captured", snapshot
        assert sum(len(value) for value in snapshot["context"].values()) <= 3000
        result = {"case": name, "passed": True, "role": snapshot["role"], "focus_source": snapshot["focus_source"], "capture_ms": snapshot["capture_ms"]}
        results.append(result)
        print(json.dumps(result), flush=True)
    try:
        terminal = subprocess.Popen(["xfce4-terminal", "--disable-server", "--title=TTSER context grab", "--execute", "python3", "-c", "import time; print('KNOWN_TERMINAL_CONTEXT', flush=True); time.sleep(120)"], env=env, stderr=(root / "terminal.log").open("w"))
        cleanup.append(lambda: stop_process(terminal))
        window = find_window("TTSER context grab")
        focus(window)
        normal = probe.capture()
        check("terminal-normal", normal)
        assert "KNOWN_TERMINAL_CONTEXT" in normal["context"]["before_cursor"]
        with keyboard_grab():
            grabbed = probe.capture()
            check("terminal-grab", grabbed)
            assert grabbed["focus_source"] == "remembered_before_grab", grabbed
            assert grabbed["context"] == normal["context"]
        released = probe.capture()
        check("terminal-released", released)
        assert released["context"] == normal["context"]

        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        driver = subprocess.Popen(["chromedriver", "--port=" + str(port)], env=env, stdout=(root / "chromedriver.log").open("w"), stderr=subprocess.STDOUT)
        cleanup.append(lambda: stop_process(driver))
        def request(method, path, data=None):
            body = json.dumps(data).encode() if data is not None else None
            req = urllib.request.Request(f"http://127.0.0.1:{port}" + path, data=body, method=method, headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=15) as response:
                return json.load(response)["value"]
        def ready():
            try: return request("GET", "/status")
            except OSError: return False
        wait_for(ready)
        session = request("POST", "/session", {"capabilities": {"alwaysMatch": {"browserName": "chrome", "goog:chromeOptions": {"binary": "/usr/bin/chromium", "args": ["--user-data-dir=" + str(root / "browser-profile"), "--no-first-run", "--no-default-browser-check", "--disable-background-networking"]}}}})["sessionId"]
        route = "/session/" + session
        cleanup.append(lambda: request("DELETE", route))
        def script(code):
            return request("POST", route + "/execute/sync", {"script": code, "args": []})
        title = "TTSER context browser " + root.name
        page = root / "page.html"
        page.write_text(f'<!doctype html><meta charset="utf-8"><title>{title}</title><textarea id="edit" rows="10" cols="70"></textarea><div id="rich" contenteditable="true" style="border:1px solid;padding:20px"></div>')
        request("POST", route + "/url", {"url": page.as_uri()})
        focus(find_window(title))
        element = request("POST", route + "/element", {"using": "css selector", "value": "#edit"})["element-6066-11e4-a52e-4f735466cecf"]
        request("POST", route + "/element/" + element + "/click", {})
        script("const e=document.getElementById('edit');e.value='a'.repeat(20000)+'BEFORE_AFTER'+'b'.repeat(20000);e.focus();e.setSelectionRange(20007,20007)")
        assert script("return document.hasFocus() && document.activeElement.id === 'edit'")
        first = probe.capture()
        check("chromium-cold-textarea", first)
        assert first["context"]["before_cursor"] == "a" * 1493 + "BEFORE_"
        assert first["context"]["after_cursor"] == "AFTER" + "b" * 495
        with keyboard_grab():
            grabbed = probe.capture()
            check("chromium-grab", grabbed)
            assert grabbed["context"] == first["context"]
        script("const e=document.getElementById('rich');e.textContent='OTHER_FIELD_BEFORE_AFTER';e.focus();const r=document.createRange();r.setStart(e.firstChild,19);r.collapse(true);const s=getSelection();s.removeAllRanges();s.addRange(r)")
        # Let focus notifications arrive before emulating a hotkey's keyboard grab.
        time.sleep(0.1)
        with keyboard_grab():
            changed = probe.capture()
        check("chromium-switch-field", changed)
        assert changed["context"]["before_cursor"] == "OTHER_FIELD_BEFORE_", changed
        assert changed["context"]["after_cursor"] == "AFTER", changed
        assert script("return document.getElementById('edit').value.length") == 40012
        assert script("return document.getElementById('rich').textContent") == "OTHER_FIELD_BEFORE_AFTER"
        assert script("return getSelection().anchorOffset") == 19
        script("const e=document.getElementById('rich');e.setAttribute('role','textbox');e.innerHTML='<p>FIRST_PARAGRAPH</p><p>SECOND_BEFORE_AFTER</p><p>LAST_PARAGRAPH</p>';e.focus();const r=document.createRange();r.setStart(e.children[1].firstChild,14);r.collapse(true);const s=getSelection();s.removeAllRanges();s.addRange(r)")
        time.sleep(0.1)
        nested = probe.capture()
        check("chromium-nested-paragraph", nested)
        assert nested["text_depth"] > 0, nested
        assert nested["context"]["before_cursor"] == "SECOND_BEFORE_", nested
        assert nested["context"]["after_cursor"].rstrip("\n") == "AFTER", nested
        assert "\ufffc" not in str(nested["context"])
        assert script("return getSelection().anchorOffset") == 14

    finally:
        probe.close()
        for action in reversed(cleanup): action()
        command("xdotool", "mousemove", original_mouse["X"], original_mouse["Y"])
        command("xdotool", "windowactivate", "--sync", original)
        (root / "report.json").write_text(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
