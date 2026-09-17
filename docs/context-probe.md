# Experimental context capture

The Linux daemon can collect local context reports when a recording starts:

```sh
ttser serve --context-probe-dir /absolute/path/to/ttser/target/context-probe
```

Pass the same option in the desktop autostart command to keep it enabled after
login. The usual `start` and `stop` commands need no changes. Remove the option
and restart the daemon to disable the experiment. There are no model API calls
or changes to transcription/insertion caused by this probe.

Each recording creates a separate directory containing:

- `context.json`: timestamp, capture delay/duration, X11 window ID/PID,
  application class, window title, AT-SPI field role, character count, caret and
  selection offsets, bounded text, and the capture status.
- `result.json`: the actual history directory, transcript and insertion outcome.
  Discarded recordings have no history link. Audio stays in normal history;
  these experimental files never modify existing history entries.

The `context` object contains at most 2800 Unicode characters: 100 for the
application class, 200 for the window title, 1500 before the caret, 500 after it,
and 500 from the first selection. Long selections contain their beginning and
end separated by an ellipsis. These are fixed prototype limits. The complete
transcript in `result.json` is separate from the context budget.

AT-SPI receives bounded character ranges, not a request for the whole document.
The probe matches the active window's PID to the accessibility application.
It reads standard extended attributes to activate lazily exposed accessibility
trees and briefly retries discovery while those trees are being populated.

A focus listener remembers only the last confirmed element handle, its top-level
window and process identity. It never reads field text between recordings.
When a global hotkey temporarily removes GTK's active/focused states through an
X11 keyboard grab, capture can still use that handle in the same X11 window and
process. An unfocused handle is not reused while its top-level accessibility
window remains active; that case requires finding the currently focused field.
Hidden/defunct objects are rejected. `focus_source` and `focus_age_ms` in version
3 reports distinguish live focus from a handle remembered before a grab. It uses Collection
when available, otherwise visits at most 1024 nodes. AT-SPI capture has a 750 ms
deadline and runs on a separate worker; it cannot delay recording or paste.
Initial accessibility connection setup has a two-second timeout. Reports
include scheduling delay separately from capture duration.

Rich editors can expose a focused container as an embedded-object marker
(`U+FFFC`) instead of its text. The probe follows the standard AT-SPI Hypertext
link at the caret, up to eight levels, to read the containing text block.
`focused_role`, `role` and `text_depth` identify the container and resolved block.
For a document with multiple paragraphs, context is currently limited to the
paragraph containing the caret, including its local selection. It does not
flatten the whole document. Remaining object markers in surrounding fragments
are omitted and flagged by `embedded_objects_omitted`.

The probe only reads; it never selects, copies, moves the caret or captures
screenshots. Fields identified as passwords are omitted. If the active X11
window changes during collection, captured field text is discarded. AT-SPI
reads are not an atomic snapshot, so edits within the same window during
capture can still race. Missing text or unsupported apps produce a diagnostic
status rather than preventing dictation. Terminal context can include command
output as well as input. App accessibility support varies; metadata-only
results are expected for some applications.

Files are private (0600; newly created directories 0700). They contain private
text and are not automatically deleted. Keep the output in gitignored `target/`
or `/tmp`. This prototype does not read clipboard contents, other windows,
past conversations or configuration secrets.

## Verification

```sh
cargo build -p ttser-linux --example capture_context --locked
python3 tests/desktop/context.py
```

Additional focus regressions:

```sh
python3 tests/desktop/context_focus.py
```

This checks a real terminal during a root-window keyboard grab and a fresh
Chromium profile without forced accessibility flags: a 40k-character textarea,
contenteditable with nested paragraphs, focus changes within the window, and
unchanged text/caret.
It requires `xfce4-terminal`, Chromium and matching `chromedriver`.

To test context through the existing system Scroll Lock binding without touching
the clipboard, stop the normal daemon first, then run:

```sh
python3 tests/desktop/context_hotkey.py
```

The test routes silence through an isolated audio source and holds Scroll Lock
until the recording exceeds its limit and is discarded. It verifies the field
text during the actual keyboard grab, with no paste or saved audio. Restart the
normal daemon afterwards.

For a complete transcription/paste test (requires a text-only clipboard), stop
the normal daemon first, then run:

```sh
python3 tests/desktop/dictation.py --wav /path/to/jfk.wav --languages pl en --system-hotkey
```

This explicitly takes the normal control socket while keeping test audio,
history, configuration and reports under `target/desktop-tests/`. It presses
and releases Scroll Lock through XTEST, checks context during the actual window
manager grab, and exits without restarting the normal daemon. Restart your
normal daemon after the test. Other tests use isolated sockets.

The GTK live test requires Python PyGObject with GTK 3, `xdotool` and X11. It focuses
isolated fixtures and restores the previous window. It checks a large Unicode
field, a large selection, an empty field and password exclusion, including
unchanged text/caret/selection. No clipboard mutation or microphone capture is
involved. Reports are under `target/desktop-tests/`.

For an individual read-only sample of the currently focused application:

```sh
target/debug/examples/capture_context target/context-probe-manual
```

The existing `tests/desktop/dictation.py` also enables the probe in its isolated
daemon and verifies one report per recording, the history link and transcript,
including repeated/busy key presses.
