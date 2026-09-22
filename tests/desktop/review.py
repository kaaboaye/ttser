"""Headless i3/GTK integration: run with xvfb-run -a python3 tests/desktop/review.py."""
import json
import os
import selectors
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from common import REPO, command, find_window, focus, selection, set_selection, stop_process, wait_for


def receiver(path):
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("Gdk", "3.0")
    from gi.repository import Gdk, Gtk
    window = Gtk.Window(title="TTSER review test destination")
    editor = Gtk.TextView()
    window.add(editor)
    events = {"text": "", "enters": 0}
    def save(*_):
        events["text"] = editor.get_buffer().get_text(*editor.get_buffer().get_bounds(), True)
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps(events))
        temporary.replace(path)
    def pressed(_, event):
        if event.keyval == Gdk.KEY_Insert and event.state & Gdk.ModifierType.SHIFT_MASK:
            if path.with_suffix(".delay").exists():
                time.sleep(0.9)
        if event.keyval in (Gdk.KEY_Return, Gdk.KEY_KP_Enter):
            events["enters"] += 1
            save()
        return False
    editor.get_buffer().connect("changed", save)
    editor.connect("key-press-event", pressed)
    window.show_all()
    editor.grab_focus()
    save()
    Gtk.main()


def clipboard_observer(root):
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("Gdk", "3.0")
    from gi.repository import Gdk, Gtk
    clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
    def received(clipboard, text, data):
        with (root / "observer.reads").open("a") as log:
            log.write(str(len(text) if text else 0) + "\n")
    def changed(clipboard, event):
        if (root / "observer.enabled").exists():
            # Concurrent GTK reads use a temporary window destroyed after receipt, as in Remmina.
            clipboard.request_text(received, None)
            clipboard.request_text(received, None)
    clipboard.connect("owner-change", changed)
    (root / "observer.ready").touch()
    Gtk.main()


def main():
    # Refuse a user's desktop; this test intentionally owns the entire X server.
    assert os.environ.get("TTSER_ISOLATED_X11") == "1", "Set TTSER_ISOLATED_X11=1 only under xvfb-run"
    parent = REPO / "target/desktop-tests"
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="review-", dir=parent) as directory:
        root = Path(directory)
        os.environ["I3SOCK"] = str(root / "i3.sock")
        os.environ["GDK_BACKEND"] = "x11"
        config = root / "i3.config"
        config.write_text("font pango:monospace 10\nfocus_follows_mouse no\n")
        processes = []
        logs = (root / "processes.log").open("w+")
        def spawn(args, **kwargs):
            process = subprocess.Popen(args, stderr=logs, **kwargs)
            processes.append(process)
            return process
        try:
            spawn(["i3", "-c", str(config)], stdout=logs)
            wait_for(lambda: subprocess.run(["i3-msg", "-t", "get_version"], capture_output=True).returncode == 0)
            data = root / "received.json"
            spawn([sys.executable, __file__, "--receiver", str(data)], stdout=logs)
            destination = find_window("^TTSER review test destination$")
            wait_for(data.exists)
            spawn([sys.executable, __file__, "--clipboard-observer", str(root)], stdout=logs)
            wait_for((root / "observer.ready").exists)
            helper = None
            for case in ["unchanged", "background-readers", "slow-with-background-readers", "after-background-readers", "delayed-paste", "multiline", "reverted", "button", "keypad", "copy", "copy-cancel", "cancel", "close", "shutdown", "empty", "long", "destination-closed"]:
                (root / "observer.enabled").unlink(missing_ok=True)
                focus(destination)
                delay = data.with_suffix(".delay")
                if case in ("delayed-paste", "slow-with-background-readers"):
                    delay.touch()
                else:
                    delay.unlink(missing_ok=True)
                command("xdotool", "key", "ctrl+a", "BackSpace")
                wait_for(lambda: json.loads(data.read_text())["text"] == "")
                for name in ("clipboard", "primary"):
                    set_selection(name, ("original " + name).encode())
                if case in ("background-readers", "slow-with-background-readers"):
                    (root / "observer.reads").unlink(missing_ok=True)
                    (root / "observer.enabled").touch()
                original = "Zażółć gęślą 🐢" if case != "long" else "Żółw 🐢 " * 12000
                if helper is None:
                    # The product must work without an interpreter or helper executable on PATH.
                    helper = spawn(
                        [str(REPO / "target/debug/examples/review_text")],
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        env={**os.environ, "PATH": str(root / "no-executables"), "G_DEBUG": "fatal-criticals"},
                    )
                payload = original.encode()
                helper.stdin.write(len(payload).to_bytes(8, "big") + payload)
                helper.stdin.flush()
                dialog = find_window("^ttser — popraw transkrypcję$")
                wait_for(lambda: command("xdotool", "getactivewindow").decode().strip() == dialog)
                tree = json.loads(command("i3-msg", "-t", "get_tree"))
                def floating(node, inside=False):
                    if node.get("window") == int(dialog):
                        return inside
                    return any(floating(child, True) for child in node.get("floating_nodes", [])) or any(floating(child, inside) for child in node.get("nodes", []))
                assert floating(tree), "Review window must float in i3"
                expected = original
                if case == "multiline":
                    command("xdotool", "key", "ctrl+End", "shift+Return")
                    command("xdotool", "type", "second line")
                    expected += "\nsecond line"
                if case == "reverted":
                    command("xdotool", "key", "ctrl+End")
                    command("xdotool", "type", "x")
                    command("xdotool", "key", "BackSpace")
                if case == "empty":
                    command("xdotool", "key", "ctrl+a", "BackSpace")
                    expected = ""
                if case in ("copy", "copy-cancel"):
                    command("xdotool", "key", "ctrl+a", "ctrl+c")
                if case in ("cancel", "copy-cancel"):
                    command("xdotool", "type", "discard me")
                    command("xdotool", "key", "Escape")
                    expected = ""
                elif case == "close":
                    command("i3-msg", f'[id="{dialog}"] kill')
                    expected = ""
                elif case == "shutdown":
                    helper.terminate()
                    expected = ""
                elif case == "button":
                    command("xdotool", "key", "ctrl+Tab", "space")
                else:
                    if case == "destination-closed":
                        command("i3-msg", f'[id="{destination}"] kill')
                    key = "KP_Enter" if case == "keypad" else "Return"
                    command("xdotool", "keydown", key)
                    time.sleep(0.85 if case == "unchanged" else 0.15)
                    assert json.loads(data.read_text())["text"] == "", "Must wait for Enter release"
                    command("xdotool", "keyup", key)
                with selectors.DefaultSelector() as ready:
                    ready.register(helper.stdout, selectors.EVENT_READ)
                    assert ready.select(timeout=15), f"No result in {case}"
                result = helper.stdout.readline().decode().strip()
                if case in ("background-readers", "slow-with-background-readers"):
                    reads = (root / "observer.reads").read_text().splitlines()
                    assert reads.count(str(len(original))) >= 2, "Both background readers must consume the dictation"
                if case == "destination-closed":
                    assert helper.wait(timeout=5) != 0, "Closed destination must fail insertion"
                    assert json.loads(data.read_text())["text"] == ""
                    print(f"PASS: {case}", flush=True)
                    continue
                assert result == ("cancelled" if case in ("cancel", "copy-cancel", "close", "shutdown") else "inserted"), (case, result)
                wait_for(lambda: json.loads(data.read_text())["text"] == expected)
                assert json.loads(data.read_text())["enters"] == 0, "Enter leaked into destination"
                for name in ("clipboard", "primary"):
                    expected_selection = original if case in ("copy", "copy-cancel") and name == "clipboard" else "original " + name
                    assert selection(name) == expected_selection.encode(), (case, name)
                wait_for(lambda: subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^ttser — popraw transkrypcję$"], capture_output=True).returncode != 0)
                if case == "shutdown":
                    assert helper.wait(timeout=5) == 0
                    helper = None
                print(f"PASS: {case}", flush=True)
        except BaseException:
            logs.flush()
            print((root / "processes.log").read_text(), file=sys.stderr)
            raise
        finally:
            for process in reversed(processes):
                stop_process(process)
            logs.close()


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--receiver":
        receiver(Path(sys.argv[2]))
    elif len(sys.argv) > 1 and sys.argv[1] == "--clipboard-observer":
        clipboard_observer(Path(sys.argv[2]))
    else:
        main()
