"""Helpers for opt-in tests that temporarily use a live X11 desktop."""
import json
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SELECTIONS = ("clipboard", "primary")


def command(*args, **kwargs):
    return subprocess.run(args, check=True, capture_output=True, timeout=30, **kwargs).stdout


def active_window():
    return command("xdotool", "getactivewindow").decode().strip()


def selection(name, target="UTF8_STRING"):
    result = subprocess.run(
        ["xclip", "-selection", name, "-o", "-t", target], capture_output=True, timeout=3
    )
    return result.stdout if result.returncode == 0 else b""


def set_selection(name, content, target="UTF8_STRING"):
    subprocess.run(
        ["xclip", "-selection", name, "-t", target], input=content,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True, timeout=3,
    )


def wait_for(predicate, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise TimeoutError("Condition did not become true")


def find_window(title):
    def find():
        result = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--name", title], capture_output=True
        )
        return result.stdout.decode().splitlines()[-1] if result.returncode == 0 else None
    return wait_for(find)


def focus(window):
    command("xdotool", "windowactivate", "--sync", window)
    time.sleep(0.2)
    assert active_window() == window, "Could not focus the isolated test window"


def stop_process(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


class DesktopSession:
    def __init__(self, name):
        for tool in ["xdotool", "xclip", "xmodmap", "xfce4-terminal"]:
            if not shutil.which(tool):
                raise RuntimeError(f"Missing smoke-test dependency: {tool}")
        # The harness preserves text only; never replace a user's rich clipboard.
        text_targets = {
            "TARGETS", "TIMESTAMP", "SAVE_TARGETS", "MULTIPLE", "UTF8_STRING",
            "STRING", "TEXT", "COMPOUND_TEXT", "text/plain",
            "text/plain;charset=utf-8", "text/plain;charset=UTF-8",
        }
        for name_ in SELECTIONS:
            targets = set(selection(name_, "TARGETS").decode().splitlines())
            if targets - text_targets:
                raise RuntimeError(f"Smoke test requires a text-only {name_}; found {targets - text_targets}")
        self.original_window = active_window()
        self.original_mouse = dict(line.split("=") for line in command("xdotool", "getmouselocation", "--shell").decode().splitlines())
        self.original = {name_: selection(name_) for name_ in SELECTIONS}
        self.keymap = command("xmodmap", "-pke")
        parent = REPO / "target" / "desktop-tests"
        parent.mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(prefix=name + "-", dir=parent))
        self.report = {"checks": []}
        self.cleanup = []
        print(f"Artifacts: {self.root}", flush=True)

    def __enter__(self):
        return self

    def __exit__(self, kind, error, traceback):
        (self.root / "finish").touch()
        errors = []
        for action in reversed(self.cleanup):
            try:
                action()
            except Exception as exc:
                errors.append(str(exc))
        for name_, content in self.original.items():
            set_selection(name_, content)
        command("xdotool", "mousemove", self.original_mouse["X"], self.original_mouse["Y"])
        command("xdotool", "windowactivate", "--sync", self.original_window)
        self.report["cleanup"] = {
            "clipboards_restored": all(selection(n) == v for n, v in self.original.items()),
            "keymap_unchanged": command("xmodmap", "-pke") == self.keymap,
            "focus_restored": active_window() == self.original_window,
            "errors": errors,
        }
        if error:
            self.report["failure"] = str(error)
        (self.root / "report.json").write_text(json.dumps(self.report, ensure_ascii=False, indent=2))
        print(json.dumps(self.report["cleanup"]), flush=True)
        if kind is None:
            assert all(v for k, v in self.report["cleanup"].items() if k != "errors")
            assert not errors, errors

    def terminal(self, cases):
        (self.root / "cases.json").write_text(json.dumps(cases))
        title = "TTSER isolated test " + self.root.name
        process = subprocess.Popen(
            ["xfce4-terminal", "--disable-server", "--title=" + title, "--geometry=80x8",
             "--execute", "python3", str(Path(__file__).with_name("receiver.py")), str(self.root)],
            stdout=subprocess.DEVNULL, stderr=(self.root / "terminal.log").open("w"),
        )
        self.cleanup.append(lambda: stop_process(process))
        window = find_window(title)
        focus(window)
        return window

    def check(self, name, expected, actual, **metadata):
        result = {"name": name, "passed": expected == actual, **metadata}
        if expected != actual:
            result.update(expected=expected, actual=actual)
        self.report["checks"].append(result)
        print(json.dumps(result, ensure_ascii=False), flush=True)
        assert result["passed"], result
