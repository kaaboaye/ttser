use anyhow::{Context, Result, ensure};
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    CURRENT_TIME,
    connection::Connection,
    protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, InputFocus},
    rust_connection::RustConnection,
};

struct Dialog(Child);

impl Drop for Dialog {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn review(
    connection: &RustConnection,
    text: &str,
    cancelled: &AtomicBool,
) -> Result<Option<(String, Destination)>> {
    let destination = Destination::capture(connection)?;
    let target = destination.target;
    let mut child = Dialog(
        Command::new("python3")
            .args(["-I", "-c", include_str!("review.py"), &target.to_string()])
            .env("GDK_BACKEND", "x11")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("Opening transcript editor (requires Python 3, PyGObject and GTK 3)")?,
    );
    child
        .0
        .stdin
        .take()
        .context("Editor input unavailable")?
        .write_all(text.as_bytes())?;
    let mut stdout = child.0.stdout.take().context("Editor output unavailable")?;
    // Drain while the editor runs: a long correction can exceed pipe capacity.
    let reader = thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).map(|_| text)
    });
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            child.0.kill()?;
        }
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let text = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Editor output reader panicked"))??;
    if cancelled.load(Ordering::Relaxed) || status.code() == Some(2) {
        return Ok(None);
    }
    ensure!(
        status.success(),
        "Transcript editor failed; install Python 3, PyGObject and GTK 3"
    );
    Ok(Some((text, destination)))
}

pub(crate) struct Destination {
    focus: u32,
    root: u32,
    active_atom: u32,
    target: u32,
}

impl Destination {
    fn capture(connection: &RustConnection) -> Result<Self> {
        let focus = connection.get_input_focus()?.reply()?.focus;
        ensure!(
            focus > 1,
            "Focus a destination field before reviewing dictation"
        );
        let root = connection.query_tree(focus)?.reply()?.root;
        let active_atom = connection
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
            .reply()?
            .atom;
        let active = || -> Result<u32> {
            connection
                .get_property(false, root, active_atom, AtomEnum::WINDOW, 0, 1)?
                .reply()?
                .value32()
                .and_then(|mut values| values.next())
                .filter(|window| *window > 1)
                .context("Window manager did not report an active destination")
        };
        let target = active()?;
        Ok(Self {
            focus,
            root,
            active_atom,
            target,
        })
    }

    pub(crate) fn restore(self, connection: &RustConnection) -> Result<()> {
        let Self {
            focus,
            root,
            active_atom,
            target,
        } = self;
        connection
            .get_window_attributes(target)?
            .reply()
            .context("Dictation destination was closed")?;
        connection
            .send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ClientMessageEvent::new(32, target, active_atom, [2, CURRENT_TIME, 0, 0, 0]),
            )?
            .check()?;
        connection.flush()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while connection
            .get_property(false, root, active_atom, AtomEnum::WINDOW, 0, 1)?
            .reply()?
            .value32()
            .and_then(|mut values| values.next())
            != Some(target)
        {
            ensure!(
                Instant::now() < deadline,
                "Could not restore dictation destination"
            );
            thread::sleep(Duration::from_millis(10));
        }
        connection
            .set_input_focus(InputFocus::PARENT, focus, CURRENT_TIME)?
            .check()?;
        ensure!(
            connection.get_input_focus()?.reply()?.focus == focus,
            "Dictation destination did not regain focus"
        );
        Ok(())
    }
}
