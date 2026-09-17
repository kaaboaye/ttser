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
            for case in ["unchanged", "multiline", "reverted", "cancel", "close", "shutdown", "empty", "long", "destination-closed"]:
                focus(destination)
                command("xdotool", "key", "ctrl+a", "BackSpace")
                wait_for(lambda: json.loads(data.read_text())["text"] == "")
                for name in ("clipboard", "primary"):
                    set_selection(name, ("original " + name).encode())
                original = "Zażółć gęślą 🐢" if case != "long" else "Żółw 🐢 " * 12000
                helper = spawn([str(REPO / "target/debug/examples/review_text")], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
                helper.stdin.write(original.encode())
                helper.stdin.close()
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
                if case == "cancel":
                    command("xdotool", "type", "discard me")
                    command("xdotool", "key", "Escape")
                    expected = ""
                elif case == "close":
                    command("i3-msg", f'[id="{dialog}"] kill')
                    expected = ""
                elif case == "shutdown":
                    helper.terminate()
                    expected = ""
                else:
                    if case == "destination-closed":
                        command("i3-msg", f'[id="{destination}"] kill')
                    command("xdotool", "keydown", "Return")
                    time.sleep(0.85 if case == "unchanged" else 0.15)
                    assert json.loads(data.read_text())["text"] == "", "Must wait for Enter release"
                    command("xdotool", "keyup", "Return")
                with selectors.DefaultSelector() as ready:
                    ready.register(helper.stdout, selectors.EVENT_READ)
                    assert ready.select(timeout=15), f"No result in {case}"
                result = helper.stdout.readline().decode().strip()
                if case == "destination-closed":
                    assert helper.wait(timeout=5) != 0, "Closed destination must fail insertion"
                    assert json.loads(data.read_text())["text"] == ""
                    print(f"PASS: {case}", flush=True)
                    continue
                assert result == ("cancelled" if case in ("cancel", "close", "shutdown") else "inserted"), (case, result)
                wait_for(lambda: json.loads(data.read_text())["text"] == expected)
                assert json.loads(data.read_text())["enters"] == 0, "Enter leaked into destination"
                for name in ("clipboard", "primary"):
                    assert selection(name) == ("original " + name).encode(), (case, name)
                wait_for(lambda: subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^ttser — popraw transkrypcję$"], capture_output=True).returncode != 0)
                stop_process(helper)
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
    else:
        main()
