"""Exercise the production X11TextOutput in real terminal/browser text fields."""
import argparse
import concurrent.futures
import json
import select
import shutil
import socket
import subprocess
import time
import urllib.request
from pathlib import Path
from common import REPO, SELECTIONS, DesktopSession, active_window, find_window, focus, selection, set_selection, stop_process, wait_for

CASES = [
    {"name": "ascii", "text": "Hello Rust 123 - punctuation: () [] {} / \\ @ # $ % & * + = _"},
    {"name": "unicode", "text": "Zażółć gęślą jaźń. ĄĆĘŁŃÓŚŹŻ — café € 🦀 😀"},
    {"name": "repeat", "text": "aaaa AAAA 1111 !!!! żżżż ŁŁŁŁ 🦀🦀🦀🦀"},
    {"name": "long", "text": ("Dyktuję po polsku i English: zażółć gęślą jaźń, Rust 123. " * 12).strip()},
]


def inject(test, binary, window, text, received=None):
    for name in SELECTIONS:
        set_selection(name, ("previous " + name).encode())
    assert active_window() == window, "Focus changed before injection"
    started = time.monotonic()
    process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8")
    test.cleanup.append(lambda: stop_process(process))
    process.stdin.write(text)
    process.stdin.close()
    copier = None
    if received:
        def copy_after_insertion():
            # Wait for delivery so a new copy cannot replace the pending paste.
            wait_for(received, timeout=5)
            assert all(selection(name).decode() == text for name in SELECTIONS), "Copy missed the clipboard restoration window"
            for name in SELECTIONS:
                set_selection(name, ("newer " + name).encode())
        executor = concurrent.futures.ThreadPoolExecutor(max_workers=1)
        test.cleanup.append(executor.shutdown)
        copier = executor.submit(copy_after_insertion)
    available, _, _ = select.select([process.stdout], [], [], 10)
    assert available, "Insertion timed out"
    output = process.stdout.readline().strip()
    if not output:
        raise RuntimeError(process.stderr.read())
    elapsed = time.monotonic() - started
    if copier:
        copier.result(timeout=5)
    assert active_window() == window, "Focus changed during injection"
    for name in SELECTIONS:
        expected = ("newer " if received else "previous ") + name
        assert selection(name).decode() == expected, f"Incorrect restoration of {name}"
    return round(elapsed, 3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=REPO / "target/debug/examples/insert_text")
    args = parser.parse_args()
    assert args.binary.is_file(), "Build: cargo build -p ttser-linux --example insert_text"
    chromium = shutil.which("chromium") or shutil.which("chromium-browser")
    assert chromium and shutil.which("chromedriver"), "Install matching Chromium and ChromeDriver"
    with DesktopSession("paste") as test:
        window = test.terminal(CASES)
        for case in CASES:
            wait_for(lambda: (test.root / (case["name"] + ".ready")).exists())
            elapsed = inject(test, args.binary, window, case["text"])
            (test.root / (case["name"] + ".sent")).touch()
            received = test.root / (case["name"] + ".received")
            wait_for(received.exists)
            test.check("terminal/" + case["name"], "\x1b[200~" + case["text"] + "\x1b[201~", received.read_bytes().decode(), seconds=elapsed)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        driver = subprocess.Popen(["chromedriver", "--port=" + str(port)], stdout=(test.root / "chromedriver.log").open("w"), stderr=subprocess.STDOUT)
        test.cleanup.append(lambda: stop_process(driver))
        def request(method, path, data=None):
            body = json.dumps(data).encode() if data is not None else None
            req = urllib.request.Request(f"http://127.0.0.1:{port}" + path, data=body, method=method, headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=30) as response:
                return json.load(response)["value"]
        def ready():
            try:
                return request("GET", "/status")
            except OSError:
                return False
        wait_for(ready)
        session = request("POST", "/session", {"capabilities": {"alwaysMatch": {"browserName": "chrome", "goog:chromeOptions": {"binary": chromium, "args": ["--user-data-dir=" + str(test.root / "browser-profile"), "--no-first-run", "--no-default-browser-check", "--disable-background-networking"]}}}})["sessionId"]
        route = "/session/" + session
        test.cleanup.append(lambda: request("DELETE", route))
        def script(code):
            return request("POST", route + "/execute/sync", {"script": code, "args": []})
        title = "TTSER browser " + test.root.name
        page = test.root / "page.html"
        page.write_text(f'''<!doctype html><meta charset="utf-8"><title>{title}</title><h1>Isolated paste test</h1><textarea id="edit" rows="12" cols="90"></textarea><div id="rich" contenteditable="true" style="border:1px solid;padding:20px"></div>
<script>
document.addEventListener('keydown', event => {{
    if (event.key === 'Insert' && event.shiftKey && window.pasteBusyMs) {{
        const end = performance.now() + window.pasteBusyMs;
        while (performance.now() < end) {{}}
    }}
}});
</script>''')
        request("POST", route + "/url", {"url": page.as_uri()})
        window = find_window(title)
        focus(window)
        element = request("POST", route + "/element", {"using": "css selector", "value": "#edit"})["element-6066-11e4-a52e-4f735466cecf"]
        request("POST", route + "/element/" + element + "/click", {})
        for case in CASES + [{"name": "new_copy", "text": "Preserve a newer clipboard copy"}]:
            script("const e=document.getElementById('edit');e.value='BEFORE[]AFTER';e.focus();e.setSelectionRange(7,7)")
            assert script("return document.hasFocus() && document.activeElement.id === 'edit'"), "Browser field lacks keyboard focus"
            expected = "BEFORE[" + case["text"] + "]AFTER"
            received = (lambda: script("return document.getElementById('edit').value") == expected) if case["name"] == "new_copy" else None
            elapsed = inject(test, args.binary, window, case["text"], received=received)
            test.check("browser/" + case["name"], "BEFORE[" + case["text"] + "]AFTER", script("return document.getElementById('edit').value"), seconds=elapsed)
        script("const e=document.getElementById('rich');e.textContent='BEFORE[]AFTER';e.focus();let r=document.createRange();r.setStart(e.firstChild,7);r.collapse(true);let s=getSelection();s.removeAllRanges();s.addRange(r)")
        assert script("return document.hasFocus() && document.activeElement.id === 'rich'")
        text = CASES[1]["text"]
        elapsed = inject(test, args.binary, window, text)
        test.check("browser/contenteditable", "BEFORE[" + text + "]AFTER", script("return document.getElementById('rich').textContent"), seconds=elapsed)
        script("document.getElementById('rich').textContent='';document.getElementById('rich').focus();window.pasteBusyMs=900")
        elapsed = inject(test, args.binary, window, text)
        actual = script("return document.getElementById('rich').textContent")
        test.check("browser/busy-contenteditable", text, actual, seconds=elapsed)


if __name__ == "__main__":
    main()
