# Linux desktop integration

Install the binary, then replace the old dictation bindings/autostart with:

```sh
cargo install --path crates/ttser --root ~/.local --locked
```

```i3
exec --no-startup-id ~/.local/bin/ttser serve
bindsym Scroll_Lock exec --no-startup-id ~/.local/bin/ttser start
bindsym --release Scroll_Lock exec --no-startup-id ~/.local/bin/ttser stop
```

Stop the previous `whisper-server` when switching to ttser to avoid keeping two
copies of the model loaded. Builds and tests do not change the desktop
configuration automatically.
