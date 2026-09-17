"""Isolated shortcut and diagnostic-log regression test; requires a local model and PulseAudio."""
import argparse
import json
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path

from common import REPO, command, stop_process, wait_for


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=REPO / "target/debug/ttser")
    args = parser.parse_args()
    assert os.environ.get("TTSER_ISOLATED_X11") == "1", "Run only under xvfb-run"
    assert args.model.is_file() and args.binary.is_file()
    parent = REPO / "target/desktop-tests"
    parent.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="control-", dir=parent))
    print(f"Artifacts: {root}", flush=True)
    processes, modules = [], []
    log = (root / "daemon.log").open("w")
    wm_log = (root / "i3.log").open("w")
    try:
        # Separate sinks prevent feedback tones from becoming microphone input.
        for name in ("input", "output"):
            sink = f"ttser_control_{os.getpid()}_{name}"
            modules.append(command("pactl", "load-module", "module-null-sink", f"sink_name={sink}").decode().strip())
        alsa = root / "asound.conf"
        alsa.write_text('pcm.default { type pulse }\n')
        env = {
            **os.environ,
            "I3SOCK": str(root / "i3.sock"),
            "ALSA_CONFIG_PATH": str(alsa),
            "PULSE_SOURCE": f"ttser_control_{os.getpid()}_input.monitor",
            "PULSE_SINK": f"ttser_control_{os.getpid()}_output",
        }
        config = root / "config.yaml"
        config.write_text(json.dumps({"model": str(args.model.resolve()), "socket": str(root / "control.sock"), "cpu": True, "history_dir": None}))
        cli = [str(args.binary.resolve()), "--config", str(config)]
        daemon = subprocess.Popen([*cli, "serve"], env=env, stdout=log, stderr=log)
        processes.append(daemon)

        def control(action):
            return command(*cli, action).decode().strip()

        def ready():
            assert daemon.poll() is None, (root / "daemon.log").read_text()
            try:
                return control("status") == "idle"
            except subprocess.CalledProcessError:
                return False

        wait_for(ready, timeout=90)
        conflict = subprocess.run(
            [*cli, "--socket", str(root / "conflict.sock"), "serve", "--hotkey-keycode", "78"],
            env=env, capture_output=True, timeout=15,
        )
        assert conflict.returncode != 0
        assert b"Grabbing push-to-talk key" in conflict.stderr
        assert not (root / "conflict.sock").exists()
        assert control("stop") == "idle"
        assert control("start") == "recording"
        assert control("start") == "recording"
        time.sleep(0.15)
        assert control("stop") == "processing"
        wait_for(ready)

        wm_config = root / "i3.config"
        wm_config.write_text("font pango:monospace 10\n")
        wm = subprocess.Popen(["i3", "-c", str(wm_config)], env=env, stdout=wm_log, stderr=wm_log)
        processes.append(wm)
        wait_for(lambda: subprocess.run(["i3-msg", "-t", "get_version"], env=env, capture_output=True).returncode == 0)
        for index in range(6):
            if index == 1:
                command("xdotool", "keydown", "Shift_L")
            command("xdotool", "keydown", "Scroll_Lock")
            try:
                wait_for(lambda: control("status") == "recording")
                time.sleep(0.8)
            finally:
                if index == 1:
                    command("xdotool", "keyup", "Shift_L")
                command("xdotool", "keyup", "Scroll_Lock")
            wait_for(ready)
            print(f"Consecutive shortcut {index + 1}: OK", flush=True)

        control("shutdown")
        assert daemon.wait(timeout=15) == 0
        text = (root / "daemon.log").read_text()
        received = re.findall(r"Control #(\d+) received:", text)
        assert received
        for identifier in received:
            assert f"Control #{identifier} handling:" in text
            assert f"Control #{identifier} completed:" in text
        assert "state=recording, trigger=Trigger { held: true }, action=None" in text
        assert "state=idle, trigger=Trigger { held: false }, action=None" in text
        assert text.count("Microphone opening:") == 7
        assert text.count("Microphone started:") == 7
        assert text.count("Feedback queued: cue=Start") == 7
        assert text.count("Feedback queued: cue=Stop") == 7
        assert text.count("Hotkey Start:") == 6
        assert text.count("Hotkey Stop:") == 6
        assert re.search(r"Hotkey Stop:.*repeats=[1-9]", text)
        assert "callback_age_ms=Some(" in text
        assert "Audio output error:" not in text
        assert "output_error=Some(" not in text
        assert "Control request failed:" not in text
        assert "response lost:" not in text
        assert not (root / "history").exists()
        print("Request correlation, duplicate/unmatched commands, microphone and feedback diagnostics: OK", flush=True)
    finally:
        for process in reversed(processes):
            stop_process(process)
        for module in reversed(modules):
            command("pactl", "unload-module", module)
        log.close()
        wm_log.close()


if __name__ == "__main__":
    main()
