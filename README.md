# ttser

Local push-to-talk dictation in Rust. Hold a key, speak, and release it to paste
text at the current cursor. Audio and speech recognition stay on your computer.

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
