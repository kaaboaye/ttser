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

Review returns approved text or cancellation. Core saves changed approvals before
insertion, preserving the original transcript. The Linux adapter embeds a small
Python/PyGObject GTK 3 editor and exchanges UTF-8 through process pipes; it kills
the editor on shutdown. i3 recognizes the window as a transient dialog.

An implementation owns focus, clipboard lifetime and keyboard injection details.
Core never sees X11 windows, selection atoms, Unix sockets or platform keycodes.
Frontends use `Controller::request(Command)` for Start, Stop, Status and Shutdown;
the Linux adapter carries those commands over a Unix socket. Audio uses CPAL's
existing platform abstraction.

A future macOS/Windows integration can implement `TextOutput` and `TranscriptReview` in its own crate
and connect its CLI, hotkey handler or UI to the same controller. The current
desktop adapter supports X11 only; native Wayland, macOS and Windows integration,
cloud transcription and model download/management are outside this MVP.
