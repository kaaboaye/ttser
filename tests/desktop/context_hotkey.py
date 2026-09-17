"""Exercise the installed Scroll Lock binding without pasting or using the clipboard.

Stop the normal daemon first. All artifacts except the shared control socket live
under target/. The isolated recording exceeds its duration limit and is discarded.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile

from common import REPO, active_window, command, find_window, focus, stop_process, wait_for


def main():
    binary = REPO / "target/release/ttser"
    socket = Path(os.environ["XDG_RUNTIME_DIR"]) / "ttser/control.sock"
    assert not socket.exists(), "Stop the normal daemon first"
    root = Path(tempfile.mkdtemp(prefix="context-hotkey-", dir=REPO / "target/desktop-tests"))
    print("Artifacts:", root, flush=True)
    original = active_window()
    sink = "ttser_context_" + str(os.getpid())
    module = command("pactl", "load-module", "module-null-sink", "sink_name=" + sink).decode().strip()
    config = root / "config.yaml"
    config.write_text(json.dumps({"socket": str(socket), "history_dir": str(root / "history"), "input_device": "pulse", "max_seconds": 2}))
    daemon = subprocess.Popen([str(binary), "--config", str(config), "serve", "--context-probe-dir", str(root / "context")], env={**os.environ, "PULSE_SOURCE": sink + ".monitor"}, stdout=subprocess.DEVNULL, stderr=(root / "daemon.log").open("w"))
    terminal = None
    def control(action):
        return subprocess.run([str(binary), "--config", str(config), action], capture_output=True, text=True)
    def ready():
        assert daemon.poll() is None, "Test daemon exited; inspect its log"
        return control("status").stdout.strip() == "idle"
    try:
        wait_for(ready, timeout=60)
        terminal = subprocess.Popen(["xfce4-terminal", "--disable-server", "--title=TTSER actual hotkey context", "--execute", "python3", "-c", "import time; print('HOTKEY_CONTEXT_MARKER', flush=True); time.sleep(90)"], env={**os.environ, "NO_AT_BRIDGE": "0"}, stderr=(root / "terminal.log").open("w"))
        window = find_window("TTSER actual hotkey context")
        focus(window)
        command("xdotool", "keydown", "Scroll_Lock")
        wait_for(lambda: control("status").stdout.strip() == "recording")
        report = wait_for(lambda: next((root / "context").glob("*/context.json"), None))
        snapshot = json.loads(report.read_text())
        assert snapshot["status"] == "captured", snapshot
        assert snapshot["role"] == "terminal", snapshot
        assert snapshot["focus_source"] == "remembered_before_grab", snapshot
        assert "HOTKEY_CONTEXT_MARKER" in snapshot["context"]["before_cursor"], snapshot
        wait_for(lambda: report.with_name("result.json").exists())
        result = json.loads(report.with_name("result.json").read_text())
        assert result["outcome"] == "discarded", result
        assert result["history_directory"] is None
        command("xdotool", "keyup", "Scroll_Lock")
        control("shutdown")
        assert daemon.wait(timeout=15) == 0
        assert len(list((root / "context").glob("*/context.json"))) == 1
        assert not (root / "history").exists()
        summary = {"passed": True, "actual_scroll_lock": True, "focus_source": snapshot["focus_source"], "capture_ms": snapshot["capture_ms"], "no_paste": True}
        (root / "report.json").write_text(json.dumps(summary, indent=2))
        print(json.dumps(summary), flush=True)
    finally:
        command("xdotool", "keyup", "Scroll_Lock")
        stop_process(daemon)
        if terminal: stop_process(terminal)
        command("pactl", "unload-module", module)
        command("xdotool", "windowactivate", "--sync", original)


if __name__ == "__main__":
    main()
