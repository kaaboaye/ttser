use anyhow::{Context, Result, ensure};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};
use ttser_core::runtime::{Command, Controller};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xkb::{self, ConnectionExt as _, PerClientFlag},
        xproto::{ConnectionExt as _, GrabMode, ModMask},
    },
    rust_connection::RustConnection,
};

pub struct Hotkey {
    connection: RustConnection,
    keycode: u8,
}

#[derive(Default)]
struct KeyState {
    held: bool,
    repeats: u32,
}

impl KeyState {
    fn update(&mut self, pressed: bool) -> Option<Command> {
        if self.held == pressed {
            if pressed {
                self.repeats = self.repeats.saturating_add(1);
            }
            return None;
        }
        self.held = pressed;
        if pressed {
            self.repeats = 0;
            Some(Command::Start)
        } else {
            Some(Command::Stop)
        }
    }
}

impl Hotkey {
    pub fn new(keycode: u8) -> Result<Self> {
        let (connection, screen) = x11rb::connect(None).context("Connecting X11 hotkey")?;
        let setup = connection.setup();
        ensure!(
            (setup.min_keycode..=setup.max_keycode).contains(&keycode),
            "Hotkey keycode is outside the X11 keymap"
        );
        ensure!(
            connection.xkb_use_extension(1, 0)?.reply()?.supported,
            "XKB is required for push-to-talk"
        );
        // Synthetic auto-repeat releases must never end a physical key hold.
        let flags = connection
            .xkb_per_client_flags(
                xkb::ID::USE_CORE_KBD.into(),
                PerClientFlag::DETECTABLE_AUTO_REPEAT,
                PerClientFlag::DETECTABLE_AUTO_REPEAT,
                xkb::BoolCtrl::default(),
                xkb::BoolCtrl::default(),
                xkb::BoolCtrl::default(),
            )?
            .reply()?;
        ensure!(
            flags.value.contains(PerClientFlag::DETECTABLE_AUTO_REPEAT),
            "X11 does not support detectable auto-repeat"
        );
        connection
            .grab_key(
                false,
                setup.roots[screen].root,
                ModMask::ANY,
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )?
            .check()
            .context("Grabbing push-to-talk key; remove its i3 start/stop bindings first")?;
        connection.flush()?;
        eprintln!("Hotkey ready: keycode={keycode}; native X11 press/release handling");
        Ok(Self {
            connection,
            keycode,
        })
    }

    pub fn listen(self, controller: Controller, shutdown: &AtomicBool) -> Result<()> {
        let mut key = KeyState::default();
        while !shutdown.load(Ordering::Relaxed) {
            let Some(event) = self.connection.poll_for_event()? else {
                thread::sleep(Duration::from_millis(10));
                continue;
            };
            let (pressed, time) = match event {
                Event::KeyPress(event) if event.detail == self.keycode => (true, event.time),
                Event::KeyRelease(event) if event.detail == self.keycode => (false, event.time),
                _ => continue,
            };
            if let Some(command) = key.update(pressed) {
                eprintln!(
                    "Hotkey {command:?}: keycode={}, x11_time={time}, repeats={}",
                    self.keycode, key.repeats
                );
                // A single event consumer preserves press/release order even when
                // microphone startup or the controller response is delayed.
                if let Err(error) = controller.request(command) {
                    eprintln!("Hotkey command failed: {command:?}: {error:#}");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_produce_exactly_one_ordered_pair_despite_repeats() {
        let mut key = KeyState::default();
        assert!(key.update(false).is_none());
        for _ in 0..5 {
            assert!(matches!(key.update(true), Some(Command::Start)));
            for _ in 0..100 {
                assert!(key.update(true).is_none());
            }
            assert!(matches!(key.update(false), Some(Command::Stop)));
            assert_eq!(key.repeats, 100);
            assert!(key.update(false).is_none());
        }
    }
}
