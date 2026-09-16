# Architecture

| Package | Responsibility |
| --- | --- |
| `ttser-core` | YAML settings, audio capture/feedback, resampling, Whisper, optional local history, state machine and dictation controller |
| `ttser-linux` | X11 text insertion, Unix socket transport, process signals and single-instance lock |
| `ttser` | CLI parsing and wiring |

`ttser-core::TextOutput` exposes one operation:

```rust
fn insert(&mut self, text: &str) -> anyhow::Result<()>;
```

An implementation owns focus, clipboard lifetime and keyboard injection details.
Core never sees X11 windows, selection atoms, Unix sockets or platform keycodes.
Frontends use `Controller::request(Command)` for Start, Stop, Status and Shutdown;
the Linux adapter carries those commands over a Unix socket. Audio uses CPAL's
existing platform abstraction.

A future macOS/Windows integration can implement `TextOutput` in its own crate
and connect its CLI, hotkey handler or UI to the same controller. The current
desktop adapter supports X11 only; native Wayland, macOS and Windows integration,
cloud transcription and model download/management are outside this MVP.
