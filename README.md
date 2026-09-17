# ttser

Local push-to-talk dictation in Rust. Hold a key, speak, and release it to review
the transcript in a small window. Edit it and press Enter to paste at the previous
cursor; Shift+Enter adds a line break. Changed approvals are saved locally as
correction feedback in the recording’s `record.yaml` when history is enabled. Audio and speech recognition stay on your computer.

The MVP supports Linux/X11 with a CLI and local Whisper inference, optionally
accelerated through Vulkan. Recording and transcription use in-memory buffers.
The workspace separates the dictation core from desktop integration so native
macOS and Windows adapters can be added later.

## Documentation

- [Build, configuration and usage](docs/usage.md)
- [Linux desktop integration](docs/linux.md)
- [Architecture and platform interfaces](docs/architecture.md)
- [Development and testing](docs/testing.md)

Licensed under [Apache 2.0](LICENSE).
