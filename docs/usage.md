# Usage

## Build

Requires Rust 1.88 or newer, a C/C++ toolchain, CMake, Clang/libclang, pkg-config,
ALSA development files, and X11 with the XTEST extension. The default build also
requires Vulkan headers/loader and `glslc` (shaderc).

On Arch/Manjaro the build dependencies are:

```sh
sudo pacman -S --needed base-devel cmake clang pkgconf alsa-lib vulkan-headers vulkan-icd-loader shaderc
cargo build --release
```

A working Vulkan driver is required at runtime (`vulkan-radeon` for the AMD setup
used during development). Build without GPU support using
`cargo build --release --no-default-features`, or select CPU at runtime with
`ttser serve --cpu`.

## Configuration

Copy [`config.example.yaml`](../config.example.yaml) to `~/.config/ttser/config.yaml`. If `XDG_CONFIG_HOME`
is set, the default location is `$XDG_CONFIG_HOME/ttser/config.yaml`.
Use `--config PATH` to select a different YAML file.

```yaml
model: ~/.local/share/whisper/ggml-large-v3-turbo.bin
languages: [pl, en]
prompt: ""
history_dir: null
threads: 4
cpu: false
input_device: null
max_seconds: 300
socket: null
paste_delay_ms: 300
```

Settings resolve as **defaults → YAML → explicit CLI flags**. Unknown YAML keys
and invalid values are errors. `prompt: ""` disables the initial prompt. Relative
`model`, `socket` and `history_dir` paths in YAML resolve relative to that file;
`~/` is expanded.
CLI paths resolve relative to the working directory. Model files are supplied by
the user; ttser does not download them.

`languages` is the single language setting:

- `[pl]` fixes the language to Polish without detection.
- `[pl, en]` restricts detection to Polish and English. The highest probability
  within that list wins; an exact tie uses the first entry.
- `[]` or an omitted field permits detection among all supported languages.

`--languages pl,en` replaces the configured list. `--languages en` fixes English;
`--languages` without values clears the list and permits unrestricted detection.

For Polish/English dictation, `[pl, en]` prevents selection of Ukrainian or
Russian. This limits the input language decision, not the output alphabet, and
does not guarantee perfect recognition or handling of mixed-language sentences.
Forcing Polish on an English recording can distort or translate its text.
The prompt supplies text context; it does not restrict language detection.
It defaults to empty. Prefer vocabulary or example text to instructions: the
previous Polish instruction prompt distorted an English test recording even
with `languages: [en]` and translation disabled.
Restart `serve` after editing the configuration.

## Local history

Set `history_dir: ~/.local/state/ttser/history` to retain recordings submitted
to transcription by `serve`. History is disabled by default. Each recording
gets a unique subdirectory containing:

- `audio.wav`: the exact mono 16 kHz float samples passed to Whisper.
- `record.yaml`: raw output, cleaned text, selected language, language detection
  probabilities when available, settings, timestamp, processing time, outcome
  and any transcription/insertion error.

The recognizer still receives its audio directly from memory. History is a
separate local copy; it is never uploaded and has no automatic retention limit.
New history directories/files have private permissions on Unix systems.
An insertion failure still preserves the transcript. A history write failure
is reported on stderr and does not prevent dictation. An interrupted job may
retain an initial `recorded` entry without a final transcript. Recordings
discarded before transcription (shutdown, microphone error, time limit) and
standalone `transcribe` commands are not logged.

To disable future collection, set `history_dir: null` and restart the daemon.
Existing files remain until you delete them.

## Run

```sh
./target/release/ttser devices
./target/release/ttser serve
```

Keep `serve` running. It loads the model once, opens the control socket, and logs
`Ready for dictation`. From another process:

```sh
./target/release/ttser status
./target/release/ttser start
./target/release/ttser stop
./target/release/ttser shutdown
```

The default socket is `$XDG_RUNTIME_DIR/ttser/control.sock`. All commands accept
`--socket PATH`; a custom socket's parent directory must already exist. `start`
and `stop` acknowledge the command without waiting for transcription. `status`
reports `loading`, `idle`, `recording`, `processing`, or the latest error.

To transcribe a WAV to stdout without microphone or desktop interaction:

```sh
./target/release/ttser transcribe speech.wav --languages pl --prompt "Rust, Linux, Vulkan."
```

## Behavior

- High/low tones mark recording start/end. A low busy tone rejects a new press
  while loading or transcribing; key repeat does not start another recording.
- Audio stays in memory, is mixed to mono and resampled to 16 kHz. A recording
  exceeding `max_seconds` is discarded with an error tone. `stop` without a
  recording does nothing. Shutdown discards recording/pending output and waits
  for an in-progress inference to finish.
- The same cleanup as the original script removes line breaks, trims whitespace
  and removes one trailing period. No Enter is sent. Blank output is ignored.
- X11 insertion writes the text to both CLIPBOARD and PRIMARY and emits
  Shift+Insert. The destination is whichever field has focus at insertion time.
  There is no per-application detection. Physically held modifiers are given up
  to a second to be released instead of being forcibly released.
- After `paste_delay_ms`, previous clipboard contents are restored if the
  selection still belongs to this operation and still contains its transcript.
  The delay is a configurable allowance, not an acknowledgement from the target
  application. Slow destinations may need a larger value.
- Clipboard snapshots support text, HTML with a text alternative, PNG-compatible
  images and file lists. Proprietary formats and additional representations are
  not preserved. Unreadable/unsupported clipboard content aborts insertion before
  replacement. Restored selections are served while the daemon stays alive;
  persistence after shutdown depends on a clipboard manager.
- Errors go to stderr, `status`, and an error tone. Transcripts and recordings
  are retained only when local history is enabled. Failed insertions are not
  retried automatically because the target may already have received the text.
