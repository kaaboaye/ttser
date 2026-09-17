# Linux desktop integration

The setup below is for Linux/X11 with i3, systemd user services, and PipeWire
with WirePlumber. See [build prerequisites and configuration](usage.md) first,
including GTK 4, the local model, and `~/.config/ttser/config.yaml`.

Install the binary from the repository root:

```sh
cargo install --path crates/ttser --root ~/.local --locked
```

## Supervised startup

Starting `ttser serve` directly from i3 can fail during login if audio is not
ready yet. The daemon initializes its audio feedback before loading the model;
an ALSA initialization error ends the process and leaves the shortcut without a
running daemon. Use a systemd user service to order startup after the audio
services and retry if initialization still fails.

Create `~/.config/systemd/user/ttser.service`:

```sh
mkdir -p ~/.config/systemd/user
```

```ini
[Unit]
Description=Local push-to-talk dictation
Wants=pipewire.service pipewire-pulse.service wireplumber.service
After=pipewire.service pipewire-pulse.service wireplumber.service
StartLimitIntervalSec=0

[Service]
ExecStart=%h/.local/bin/ttser serve
Restart=on-failure
RestartSec=5
TimeoutStopSec=120
```

`%h` is the user's home directory. Startup ordering does not guarantee that
audio devices are already available, so failed starts are retried every five
seconds without a start-rate cutoff. Persistent errors remain visible in the
journal; stop the service while fixing them. A normal shutdown does not trigger
a restart.

Replace the previous dictation autostart and bindings in `~/.config/i3/config`
with:

```i3
exec --no-startup-id systemctl --user import-environment DISPLAY XAUTHORITY && systemctl --user start ttser.service
bindsym Scroll_Lock exec --no-startup-id ~/.local/bin/ttser start
bindsym --release Scroll_Lock exec --no-startup-id ~/.local/bin/ttser stop
```

The i3 session imports its X11 environment before starting the service. Do not
enable the service at user-manager startup: it needs the logged-in graphical
session. This unit intentionally has no `[Install]` section. Remove the old
direct `ttser serve` autostart so only the service owns the daemon.

Stop any manually running daemon with `~/.local/bin/ttser shutdown` before the
first service start; a "Cannot reach" response is expected if no daemon is
running. Also stop the previous `whisper-server` when migrating to
ttser to avoid loading two copies of the model.

From a terminal in the X11 session, validate and activate the setup:

```sh
systemd-analyze --user verify ~/.config/systemd/user/ttser.service
i3 -C -c ~/.config/i3/config
systemctl --user daemon-reload
systemctl --user import-environment DISPLAY XAUTHORITY
systemctl --user start ttser.service
i3-msg reload
```

Reloading i3 updates the bindings but does not rerun `exec`, which is why the
initial service start is explicit. Future logins run it automatically. Builds
and tests do not install the service or change desktop configuration.

## Status and troubleshooting

Wait for model loading to finish, then check that the daemon reports `idle`:

```sh
~/.local/bin/ttser status
systemctl --user status ttser.service
journalctl --user -u ttser.service -b -n 100 --no-pager
```

Hold Scroll Lock to record and release it to transcribe. If the shortcut does
nothing, check the service and journal first. `Cannot reach ... control.sock`
means the client cannot contact the daemon. An ALSA error during startup points
to audio initialization; check `pipewire.service`, `pipewire-pulse.service`, and
`wireplumber.service` with `systemctl --user status`.

Service output goes to the journal, replacing the old `daemon.log` redirection.
Follow it with `journalctl --user -u ttser.service -f`. After changing the YAML
configuration or installing a new binary, restart the daemon:

```sh
systemctl --user restart ttser.service
```

Use `systemctl --user stop ttser.service` to stop dictation. To disable startup
at future logins too, remove the dictation `exec` line from the i3 configuration.

## Transcript review

The transcript review dialog floats automatically in i3 via its dialog and
transient-window hints; no extra binding or floating rule is needed. It requires
GTK 4. Enter pastes, Shift+Enter adds a newline, and Escape
cancels. See [review and feedback settings](usage.md#review-and-correction-feedback).
