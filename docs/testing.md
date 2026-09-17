# Development and testing

Run the following commands from the repository root.

Development artifacts belong in gitignored `target/` directories or `/tmp`.
The application's runtime state directory is reserved for real use. Experiments
may read existing recordings but must not add reports or annotations to that
directory or its history entries. Keep private transcripts out of version control.

## Rust checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Core tests do not require a model, microphone or display. Desktop integration
requires a live X11 session. The `insert_text` example reads UTF-8 text from stdin
until EOF, inserts it into the focused field, and keeps the restored clipboard
alive until the helper is terminated. Use a disposable field and never run it
with a shell prompt focused.

## Model regression test without desktop interaction

This opt-in test uses the public `samples/jfk.wav` from whisper.cpp. It checks
English recognition with Polish/English detection at original and reduced
volume, repeated inference using one loaded model, and original audio/text/gain
history round trips. It does not access
the microphone or clipboard, or download any files.

```sh
TTSER_TEST_MODEL=/path/to/ggml-large-v3-turbo.bin TTSER_TEST_WAV=/path/to/jfk.wav \
  cargo test -p ttser-core --features vulkan real_speech_preserves_english_and_history_across_recordings -- --ignored
```

## Audio level comparison

Requires Python 3 with PyYAML and a release binary. This reads existing local
history without changing it, uses the real recognizer, and does not access the
microphone or clipboard. Write private reports under `target/benchmarks/` or
`/tmp`. The output directory must not already exist.

```sh
cargo build --release --locked
python3 tests/compare_audio_levels.py \
  --history "$HOME/.local/state/ttser/history" \
  --output target/benchmarks/peak-comparison \
  --references /path/to/corrected-references.json
```

The optional references file is a JSON object mapping recording directory names
to corrected text. Without a corrected reference, historical output is used as
a provisional reference, not ground truth. Use `--recordings ID ...` to restrict
the comparison to particular recordings. The script tests original audio and peaks 0.1, 0.25, 0.5, 0.85,
0.95. Use `--peaks` and `--max-gain` to vary them. The experimental default cap
is 100x to reach the requested levels; production defaults to 10x.

`report.json` records actual output, reference origin, gain, peaks, binary hash
and word edit distance. Read the variants as well: punctuation and case are
ignored by the metric, and a word-order change need not be a language error.
See [the initial findings](audio-levels.md) for the limitations of this comparison.

## OpenRouter comparison on selected recordings

`tests/compare_openrouter.py` makes paid requests and uploads only explicitly
listed recordings to OpenRouter and the selected model providers. It is an
opt-in experiment, separate from the local application's speech engine and CI.
Requires Python 3 and PyYAML. The supplied YAML must contain
`openrouter_api_key`; the script never copies that configuration into its report.

```sh
python3 tests/compare_openrouter.py \
  --config "$HOME/.config/ttser/config.yaml" \
  --history "$HOME/.local/state/ttser/history" \
  --recordings RECORDING_ID ANOTHER_RECORDING_ID \
  --references /path/to/corrected-references.json \
  --output target/benchmarks/openrouter-comparison
```

References map IDs to text or lists of acceptable texts. They are used only for
local scoring and are never sent to models. The experiment calls
`microsoft/mai-transcribe-2`, `openai/gpt-transcribe`, and `google/chirp-3` using
the [STT endpoint](https://openrouter.ai/docs/guides/overview/multimodal/stt).
Each model receives original and peak-0.25 audio, with automatic language and
explicit Polish: twelve requests per recording. Audio is converted in memory
to mono 16 kHz PCM16 WAV for all providers; the gain cap is 10x.

The private JSON and Markdown reports include actual output, request language,
source/payload hashes, gain, network-inclusive elapsed time, and API-reported
usage/cost. Errors are recorded without their response bodies. The script does
not automatically retry requests, and stops on authentication/billing refusals.
Single requests per condition do not establish repeatability or a latency SLA.
Raw word distance still needs human interpretation, particularly for fillers.

Offline harness tests make no API calls:

```sh
python3 -m unittest discover -s tests -p test_audio_benchmarks.py
```

## Headless review dialog integration

The Rust editor requires GTK 4. The test receiver additionally requires Python 3
with PyGObject/GTK 3, Xvfb, xauth, i3, xdotool and xclip. Python and GTK 3 are
test-only dependencies:

```sh
cargo build -p ttser-linux --example review_text --locked
TTSER_ISOLATED_X11=1 xvfb-run -a -s '-screen 0 1280x800x24' python3 tests/desktop/review.py
```

This runs in CI and owns a separate X server and i3 socket. It tests floating
placement, unchanged approval, Unicode, multiline correction, reverted edits,
empty approval, Escape, closing the dialog, shutdown, long text, focus restoration
closed destinations, and clipboard preservation through the production review and insertion backend.
Successive approvals and cancellations reuse one Rust worker thread in a process
whose PATH has no executables, to verify GTK reuse and independence from Python.
Approval is exercised through Enter, keypad Enter and the paste button.
Copying inside the editor must leave the clipboard usable after the dialog closes.
It also checks that holding Enter does not paste until release and that the
receiving field never gets an Enter key. Core tests cover exact feedback matching,
private persistence, disabled collection, write errors and paste failures.

## Live X11 integration tests

The shortcut/log regression test uses an isolated X server and two temporary
PulseAudio sinks (one silent input and one feedback output). It does not use the
physical microphone, speakers, real history, or the user's desktop. It needs an
existing local model; it loads it on CPU and transcribes only silence. It checks
consecutive native Scroll Lock holds with i3 running, auto-repeat filtering, repeated CLI starts,
unmatched stops, request IDs, microphone timings, and output callback diagnostics:

```sh
cargo build --locked
TTSER_ISOLATED_X11=1 xvfb-run -a python3 tests/desktop/control.py --model /path/to/model.bin
```

These opt-in tests temporarily focus isolated windows and use both selections.
Run them on an idle Linux/X11 desktop. They save and restore the previous focus,
mouse position and text selections. The terminal runs a raw Python receiver,
never a shell. Chromium uses a disposable profile and a local test page.
To avoid losing clipboard representations, the harness refuses to start when
the current clipboard contains non-text formats.

Dependencies: Python 3 (standard library only), `xfce4-terminal`, `xdotool`,
`xclip`, `xmodmap`. The browser test additionally needs matching `chromium` and
`chromedriver`; dictation needs `pactl`, `paplay` and a PulseAudio-compatible
server (including PipeWire).

### Paste correctness and clipboard restoration

```sh
cargo build -p ttser-linux --example insert_text --locked
python3 tests/desktop/paste.py
```

Checks ASCII/symbols, Polish accents, emoji, repeated characters, and a 695-character
transcript in xfce4-terminal and Chromium. Browser cases insert between existing
characters; contenteditable is tested too. Terminal cases verify bracketed-paste
boundaries. Every case checks restoration of both selections. A concurrent-copy
case verifies that restoration preserves newer clipboard data. Text is inserted
only by the production Rust backend; WebDriver only prepares/reads the field.
The harness sends the text directly to the Rust helper through a stdin pipe;
there is no intermediate payload file. The helper stays alive after EOF to
serve the restored selections until the harness terminates it.

### Entire dictation path

Provide a WAV of known speech and an existing Whisper GGML model:

```sh
cargo build --release --locked
python3 tests/desktop/dictation.py --wav /path/to/jfk.wav
python3 tests/desktop/dictation.py --wav /path/to/jfk.wav --languages pl en
```

The default assertion matches `samples/jfk.wav` from
[whisper.cpp](https://github.com/ggml-org/whisper.cpp/blob/master/samples/jfk.wav).
For another recording use `--contains "expected words"`. `--model PATH` overrides
the default model location. Tests never download models or recordings.
The second invocation also tests restricted language detection, ensuring an
English recording remains English when Polish is another allowed language.

A temporary null sink feeds the recording through the actual CPAL input path.
The test uses a separate YAML config/socket, exercises start/stop, key repeat,
busy state, Whisper, actual terminal insertion and shutdown. It unloads its
temporary audio module afterwards and never changes the system default devices.

Each run writes logs and `report.json` to a unique directory under
`target/desktop-tests/` and exits nonzero on failure. These tests are not part of
headless CI. Pure configuration, resampling, trigger-state and IPC tests run with
`cargo test --workspace --all-features`.
