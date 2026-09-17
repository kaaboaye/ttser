"""Read-only context capture against isolated GTK fields on the live X11 desktop."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from common import REPO, active_window, command, find_window, focus, stop_process, wait_for


def contents(case):
    if case == "empty":
        return "", 0, 0
    if case == "password":
        return "must-never-be-captured", 5, 5
    text = "żółw🙂 " * 6000 + "BEFORE_CURSOR" + "AFTER_CURSOR" + " koniec" * 3000
    caret = 36000 + len("BEFORE_CURSOR")
    return text, caret, caret + 8000 if case == "selection" else caret


def fixture(root, case):
    import gi
    gi.require_version("Gtk", "3.0")
    from gi.repository import Gtk, GLib
    text, caret, end = contents(case)
    window = Gtk.Window(title="TTSER context fixture " + root.name)
    window.set_default_size(700, 400)
    if case == "password":
        field = Gtk.Entry()
        field.set_visibility(False)
        field.set_text(text)
        window.add(field)
        window.show_all()
        field.grab_focus()
        field.set_position(caret)
        def state():
            return [field.get_text(), field.get_position(), field.get_selection_bounds()]
    else:
        field = Gtk.TextView()
        scroll = Gtk.ScrolledWindow()
        scroll.add(field)
        window.add(scroll)
        window.show_all()
        field.grab_focus()
        buffer = field.get_buffer()
        buffer.set_text(text)
        buffer.select_range(buffer.get_iter_at_offset(caret), buffer.get_iter_at_offset(end))
        field.scroll_to_mark(buffer.get_insert(), 0, False, 0, 0)
        def state():
            return [buffer.get_text(*buffer.get_bounds(), True), buffer.get_iter_at_mark(buffer.get_insert()).get_offset(), [i.get_offset() for i in buffer.get_selection_bounds()]]
    before = state()
    def inspect():
        if (root / "inspect").exists():
            after = state()
            (root / "fixture-result.json").write_text(json.dumps({"unchanged": before == after, "sha256": hashlib.sha256(after[0].encode()).hexdigest()}))
            return False
        return True
    GLib.timeout_add(30, inspect)
    (root / "ready").touch()
    Gtk.main()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path)
    parser.add_argument("--case")
    args = parser.parse_args()
    if args.fixture:
        fixture(args.fixture, args.case)
        return
    binary = REPO / "target/debug/examples/capture_context"
    original = active_window()
    parent = REPO / "target/desktop-tests"
    parent.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="context-", dir=parent))
    print("Artifacts:", root, flush=True)
    results = []
    try:
        for case in ["large", "selection", "empty", "password"]:
            directory = root / case
            directory.mkdir()
            process = subprocess.Popen([sys.executable, __file__, "--fixture", str(directory), "--case", case], env={**os.environ, "NO_AT_BRIDGE": "0"}, stdout=subprocess.DEVNULL, stderr=(directory / "fixture.log").open("w"))
            try:
                wait_for(lambda: (directory / "ready").exists())
                window = find_window("TTSER context fixture " + directory.name)
                focus(window)
                command(str(binary), str(directory / "reports"))
                snapshot = json.loads(next((directory / "reports").glob("*/context.json")).read_text())
                payload = snapshot["context"]
                assert sum(len(value) for value in payload.values()) <= 3000
                if case == "password":
                    assert snapshot["status"] == "Password field omitted", snapshot
                    assert all(payload[key] == "" for key in ["before_cursor", "after_cursor", "selected_text"])
                else:
                    assert snapshot["status"] == "captured", snapshot
                    text, caret, end = contents(case)
                    assert payload["before_cursor"] == text[max(0, caret - 1500):caret]
                    assert payload["after_cursor"] == text[caret:caret + 500]
                    if case == "selection":
                        assert payload["selected_text"] == text[caret:caret + 250] + "…" + text[end - 249:end]
                (directory / "inspect").touch()
                wait_for(lambda: (directory / "fixture-result.json").exists())
                assert json.loads((directory / "fixture-result.json").read_text())["unchanged"]
                assert active_window() == window
                result = {"case": case, "passed": True, "capture_ms": snapshot["capture_ms"], "context_characters": sum(len(value) for value in payload.values())}
                results.append(result)
                print(json.dumps(result), flush=True)
            finally:
                stop_process(process)
    finally:
        command("xdotool", "windowactivate", "--sync", original)
        (root / "report.json").write_text(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
