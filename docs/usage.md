# Usage

## Build

Requires Rust 1.88 or newer, a C/C++ toolchain, CMake, Clang/libclang, pkg-config,
ALSA development files, and X11 with the XTEST extension. The default build also
requires Vulkan headers/loader and `glslc` (shaderc). The review window is
implemented in Rust using GTK 4 (`gtk4` on Arch/Manjaro; `libgtk-4-dev` for
building and `libgtk-4-1` at runtime on Debian/Ubuntu). Python is only used by
integration tests and is not required to build or run the product.

On Arch/Manjaro the build dependencies are:

```sh
sudo pacman -S --needed base-devel cmake clang pkgconf alsa-lib vulkan-headers vulkan-icd-loader shaderc gtk4
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
audio_target_peak: 0.25
audio_max_gain: 10.0
max_seconds: 300
socket: null
paste_delay_ms: 300
```

Settings resolve as **defaults → YAML → explicit CLI flags**. Unknown YAML keys
produce a warning with the field name and are ignored; their values are not
retained in history. Invalid values of known fields and malformed YAML remain
errors. `prompt: ""` disables the initial prompt. Relative
`model`, `socket` and `history_dir` paths in YAML resolve relative
to that file; `~/` is expanded.
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
Restart `serve` after editing the configuration. With the
[Linux desktop service](linux.md), use `systemctl --user restart ttser.service`.

## Audio level

Before language detection and transcription, the program measures the peak
absolute amplitude and applies one constant multiplier to the entire recording:
`gain = min(audio_target_peak / peak, audio_max_gain)`. The defaults target 0.25
(-12 dBFS) and cap amplification at 10x (+20 dB). Louder input is attenuated to
the target; quiet input may remain below it when the gain cap is reached.
Zero and near-zero signals are left unchanged. Processing stays in memory and
applies to both live dictation and `transcribe`.

These settings follow a [local comparison](audio-levels.md), not a proven optimum for all speech. Peak
normalization also amplifies background noise and a loud click can limit the
gain for a whole recording. It is not noise removal or speech detection.
See the [FFmpeg normalization discussion](https://ffmpeg.org/ffmpeg-filters.html#dynaudnorm)
for the distinction between peak amplitude and perceived loudness.

## Local history

Set `history_dir: ~/.local/state/ttser/history` to retain recordings submitted
to transcription by `serve`. History is disabled by default. Each recording
gets a unique subdirectory containing:

- `audio.wav`: original mono 16 kHz float samples, before volume normalization.
- `record.yaml`: raw output, cleaned text, selected language, language detection
  probabilities when available, settings, timestamp, processing time, outcome
  and any transcription/insertion error. Approved corrections are stored in the
  top-level `corrected_text` field of this same file.

`transcript.audio` records the original peak, applied gain, and resulting peak.
Multiplying the saved samples by that gain reconstructs the buffer passed to
Whisper. It is null when input is skipped without inference. Record schema
version 3 also includes `corrected_text` only for changed approvals.

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

## Review and correction feedback

Every nonempty live transcript opens a small floating GTK dialog in i3. Edit the
text, then press **Enter** (or click **Wklej**) to paste it into the field that
was focused before the dialog opened. **Shift+Enter** inserts a newline.
**Escape** or closing the dialog discards the pending paste. Enter is consumed
by the dialog and does not submit the destination form. If the destination was
closed, insertion fails instead of pasting elsewhere.

Only an approved text that differs exactly from the original transcript creates
feedback. Editing and then reverting the change does not count. Whitespace and
newlines count as changes; approving an empty correction saves feedback without
pasting. Cancellation or shutdown saves no correction.

Corrections are stored directly in the recording's `record.yaml`, in the top-level
`corrected_text` field. The original remains in `transcript.text`; `audio.wav`
is next to the record. There are no separate feedback files or directories.
The field is absent for unchanged approvals and cancelled reviews, and can be an
empty string when an empty correction is approved. It is saved before insertion
and retained even if the paste fails.

Correction collection uses `history_dir`: when history is disabled, corrections
are not saved. Records remain local and are not uploaded or used for training
automatically. Write errors are reported on stderr without blocking the paste.

## Run

For automatic startup in i3 with audio startup ordering and recovery after
failures, follow [Linux desktop integration](linux.md). The commands below run
the daemon manually; stop the desktop service before starting a manual daemon
on the same socket.

```sh
./target/release/ttser devices
./target/release/ttser serve
```

`serve` handles Scroll Lock (X11 keycode 78) by default. Use `--hotkey-keycode`
to select another key or `--no-hotkey` for programmatic control only.

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
reports `loading`, `idle`, `recording`, `processing`, `reviewing`, or the latest error.

To transcribe a WAV to stdout without microphone or desktop interaction:

```sh
./target/release/ttser transcribe speech.wav --languages pl --prompt "Rust, Linux, Vulkan."
```

## Behavior

- High/low tones mark recording start/end. A low busy tone rejects a new press
  while loading, transcribing or reviewing; key repeat does not start another
  recording.
- Audio stays in memory, is mixed to mono and resampled to 16 kHz. A recording
  exceeding `max_seconds` is discarded with an error tone. `stop` without a
  recording does nothing. Shutdown discards recording/pending output and waits
  for an in-progress inference to finish.
- The same cleanup as the original script removes line breaks, trims whitespace
  and removes one trailing period. No Enter is sent. Blank output is ignored.
- X11 insertion writes the approved text to both CLIPBOARD and PRIMARY and emits
  Shift+Insert after restoring the field focused before review.
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
  are retained only when local history is enabled. Failed
  insertions are not retried automatically because the target may already have received the text.
