"""Full recording/transcription/paste test through a temporary PulseAudio sink."""
import argparse
import json
import os
import subprocess
import time
from pathlib import Path
from common import REPO, SELECTIONS, DesktopSession, active_window, command, selection, set_selection, stop_process, wait_for


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=REPO / "target/release/ttser")
    parser.add_argument("--wav", type=Path, required=True)
    parser.add_argument("--model", type=Path)
    parser.add_argument("--languages", nargs="*", default=["en"], help="Allowed languages; one fixes the language, no values permits all")
    parser.add_argument("--contains", default="ask not what your country can do for you")
    parser.add_argument("--system-hotkey", action="store_true", help="Use the existing Scroll Lock binding and default socket; stop the normal daemon first")
    args = parser.parse_args()
    assert args.binary.is_file() and args.wav.is_file()
    with DesktopSession("dictation") as test:
        sink = "ttser_test_" + str(os.getpid())
        module = command("pactl", "load-module", "module-null-sink", "sink_name=" + sink).decode().strip()
        test.cleanup.append(lambda: command("pactl", "unload-module", module))
        config = test.root / "config.yaml"
        settings = {"socket": str(test.root / "control.sock"), "input_device": "pulse", "languages": args.languages, "prompt": "", "max_seconds": 60, "history_dir": str(test.root / "history")}
        if args.system_hotkey:
            settings["socket"] = str(Path(os.environ["XDG_RUNTIME_DIR"]) / "ttser/control.sock")
            assert not Path(settings["socket"]).exists(), "Stop the normal daemon before this explicit system hotkey test"
        if args.model:
            settings["model"] = str(args.model.resolve())
        # JSON is a YAML subset, so this needs no Python YAML dependency.
        config.write_text(json.dumps(settings))
        env = {**os.environ, "PULSE_SOURCE": sink + ".monitor"}
        daemon = subprocess.Popen([str(args.binary), "--config", str(config), "serve", "--context-probe-dir", str(test.root / "context")], env=env, stdout=subprocess.DEVNULL, stderr=(test.root / "daemon.log").open("w"))
        test.cleanup.append(lambda: stop_process(daemon))
        def control(action):
            return command(str(args.binary), "--config", str(config), action).decode().strip()
        def ready():
            if daemon.poll() is not None:
                raise RuntimeError((test.root / "daemon.log").read_text())
            try:
                return control("status") == "idle"
            except subprocess.CalledProcessError:
                return False
        wait_for(ready, timeout=90)
        window = test.terminal([{"name": "speech"}])
        wait_for(lambda: (test.root / "speech.ready").exists())
        for name in SELECTIONS:
            set_selection(name, ("before dictation " + name).encode())
        test.check("unmatched-stop", "idle", control("stop"))
        if args.system_hotkey:
            test.cleanup.append(lambda: command("xdotool", "keyup", "Scroll_Lock"))
            command("xdotool", "keydown", "Scroll_Lock")
            wait_for(lambda: control("status") == "recording")
            test.check("hotkey-start", "recording", control("status"))
        else:
            test.check("start", "recording", control("start"))
        test.check("key-repeat", "recording", control("start"))
        time.sleep(0.2)
        player = subprocess.Popen(["paplay", "--device=" + sink, str(args.wav)])
        test.cleanup.append(lambda: stop_process(player))
        assert player.wait(timeout=45) == 0
        time.sleep(0.2)
        assert active_window() == window, "Test lost focus before transcription"
        if args.system_hotkey:
            command("xdotool", "keyup", "Scroll_Lock")
            wait_for(lambda: control("status") != "recording")
            test.check("hotkey-stop", "processing", control("status"))
        else:
            test.check("stop", "processing", control("stop"))
        test.check("busy-press", "processing", control("start"))
        wait_for(ready, timeout=90)
        test.check("busy-key-repeat-after-completion", "idle", control("start"))
        test.check("busy-release", "idle", control("stop"))
        (test.root / "speech.sent").touch()
        received = test.root / "speech.received"
        wait_for(received.exists)
        text = received.read_bytes().decode()
        test.check("bracketed-paste", True, text.startswith("\x1b[200~") and text.endswith("\x1b[201~"))
        transcript = text[6:-6]
        test.check("transcription", True, args.contains.lower() in transcript.lower(), transcript=transcript)
        test.check("no-enter", True, "\n" not in transcript and "\r" not in transcript)
        entries = list((test.root / "history").iterdir())
        test.check("history-entry-count", 1, len(entries))
        test.check("history-audio", True, (entries[0] / "audio.wav").stat().st_size > 44)
        record = (entries[0] / "record.yaml").read_text()
        test.check("history-transcript", True, args.contains.lower() in record.lower())
        test.check("history-inserted", True, "status: inserted" in record)
        wait_for(lambda: len(list((test.root / "context").glob("*/result.json"))) == 1)
        context_result = json.loads(next((test.root / "context").glob("*/result.json")).read_text())
        snapshot = json.loads(next((test.root / "context").glob("*/context.json")).read_text())
        test.check("context-session-count", 1, len(list((test.root / "context").iterdir())))
        test.check("context-history-link", str(entries[0]), context_result["history_directory"])
        test.check("context-transcript", transcript, context_result["transcript"])
        test.check("context-window", int(window), snapshot["window_id"])
        test.check("context-captured", "captured", snapshot["status"])
        test.check("context-terminal", "terminal", snapshot["role"])
        if args.system_hotkey:
            test.check("context-through-grab", "remembered_before_grab", snapshot["focus_source"])
        test.check("context-bounded", True, sum(len(value) for value in snapshot["context"].values()) <= 3000)
        for name in SELECTIONS:
            test.check("restored-" + name, "before dictation " + name, selection(name).decode())
        control("shutdown")
        test.check("shutdown", 0, daemon.wait(timeout=15))
        test.check("socket-removed", False, Path(settings["socket"]).exists())


if __name__ == "__main__":
    main()
