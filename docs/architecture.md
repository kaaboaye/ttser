# Architecture

| Package | Responsibility |
| --- | --- |
| `ttser-core` | YAML settings, audio capture/feedback, resampling, Whisper, optional local history, correction feedback, state machine and dictation controller |
| `ttser-linux` | GTK transcript review, X11 text insertion, Unix socket transport, process signals and single-instance lock |
| `ttser` | CLI parsing and wiring |

`ttser-core::TextOutput` and `TranscriptReview` separate insertion from approval:

```rust
fn insert(&mut self, text: &str) -> anyhow::Result<()>;
fn review(&mut self, text: &str, cancelled: &AtomicBool) -> anyhow::Result<Option<String>>;
```

Review returns approved text or cancellation. Core saves changed approvals in the history record’s `corrected_text` field
before insertion, preserving the original in `transcript.text`. The Linux adapter implements
the GTK 4 editor in Rust, in the same process as the daemon. A dedicated GTK
thread receives review requests from the dictation worker and continues serving
the clipboard between reviews. Text stays in memory; the event loop observes
cancellation and destroys the window before acknowledging shutdown. i3 recognizes
the window as a transient dialog. All product code
is Rust; Python is confined to integration tests.

An implementation owns focus, clipboard lifetime and keyboard injection details.
Core never sees X11 windows, selection atoms, Unix sockets or platform keycodes.
Frontends use `Controller::request(Command)` for Start, Stop, Status and Shutdown;
the Linux adapter carries those commands over a Unix socket. Audio uses CPAL's
existing platform abstraction.

By default, `serve` uses a dedicated X11 connection to own the push-to-talk
key (keycode 78; configurable with `--hotkey-keycode`, disabled with `--no-hotkey`).
XKB detectable auto-repeat distinguishes physical releases from repeats;
one consumer filters repeated presses and submits Start/Stop in event order.
This avoids reordering commands from independently spawned shortcut clients.

A future macOS/Windows integration can implement `TextOutput` and `TranscriptReview` in its own crate
and connect its CLI, hotkey handler or UI to the same controller. The current
desktop adapter supports X11 only; native Wayland, macOS and Windows integration,
cloud transcription and model download/management are outside this MVP.
