use anyhow::{Context, Result, bail, ensure};
use arboard::{ClearExtLinux, Clipboard, GetExtLinux, ImageData, LinuxClipboardKind, SetExtLinux};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use ttser_core::TextOutput;
use x11rb::{
    CURRENT_TIME, NONE,
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ConnectionExt, CreateWindowAux, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
            WindowClass,
        },
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

enum Content {
    Empty,
    Text(String),
    Html(String, Option<String>),
    Image(ImageData<'static>),
    Files(Vec<PathBuf>),
}

struct Selection {
    kind: LinuxClipboardKind,
    atom: Atom,
    content: Content,
    owner: u32,
}

pub struct X11TextOutput {
    clipboard: Clipboard,
    connection: RustConnection,
    clipboard_atom: Atom,
    query_window: u32,
    paste_delay: Duration,
}

impl X11TextOutput {
    pub fn new(paste_delay: Duration) -> Result<Self> {
        ensure!(
            std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland"),
            "This backend requires an X11 session; native Wayland is not implemented yet"
        );
        let (connection, screen) =
            x11rb::connect(None).context("Connecting to X11 (check DISPLAY)")?;
        let query_window = connection.generate_id()?;
        connection
            .create_window(
                0,
                query_window,
                connection.setup().roots[screen].root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::default(),
            )?
            .check()?;
        connection
            .xtest_get_version(2, 2)?
            .reply()
            .context("X11 XTEST extension is required")?;
        let clipboard_atom = connection.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
        Ok(Self {
            clipboard: Clipboard::new()?,
            connection,
            clipboard_atom,
            query_window,
            paste_delay,
        })
    }

    fn owner(&self, atom: Atom) -> Result<u32> {
        Ok(self.connection.get_selection_owner(atom)?.reply()?.owner)
    }

    fn snapshot(&mut self, kind: LinuxClipboardKind, atom: Atom) -> Result<Selection> {
        let owner = self.owner(atom)?;
        let targets = if owner != NONE {
            self.targets(atom)?
        } else {
            Vec::new()
        };
        let supports = |name: &[u8]| -> Result<bool> {
            Ok(targets.contains(&self.connection.intern_atom(true, name)?.reply()?.atom))
        };
        let has_files = supports(b"text/uri-list")?;
        let has_html = supports(b"text/html")?;
        let has_image = supports(b"image/png")?;
        let mut has_text = false;
        for name in [
            b"UTF8_STRING".as_slice(),
            b"text/plain;charset=utf-8",
            b"text/plain;charset=UTF-8",
            b"STRING",
            b"TEXT",
            b"text/plain",
        ] {
            has_text |= supports(name)?;
        }
        let content = if owner == NONE {
            Content::Empty
        } else if has_files {
            Content::Files(self.clipboard.get().clipboard(kind).file_list()?)
        } else if has_html {
            Content::Html(
                self.clipboard.get().clipboard(kind).html()?,
                if has_text {
                    Some(self.clipboard.get().clipboard(kind).text()?)
                } else {
                    None
                },
            )
        } else if has_image {
            Content::Image(self.clipboard.get().clipboard(kind).image()?)
        } else if has_text {
            Content::Text(self.clipboard.get().clipboard(kind).text()?)
        } else {
            bail!("Clipboard contains an unsupported format; leaving it unchanged");
        };
        ensure!(
            self.owner(atom)? == owner,
            "Clipboard changed while preparing dictation"
        );
        Ok(Selection {
            kind,
            atom,
            content,
            owner,
        })
    }

    fn targets(&self, selection: Atom) -> Result<Vec<Atom>> {
        let target = self
            .connection
            .intern_atom(false, b"TARGETS")?
            .reply()?
            .atom;
        let property = self
            .connection
            .intern_atom(false, b"_TTSER_TARGETS")?
            .reply()?
            .atom;
        self.connection
            .convert_selection(self.query_window, selection, target, property, CURRENT_TIME)?
            .check()?;
        self.connection.flush()?;
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(Event::SelectionNotify(event)) = self.connection.poll_for_event()?
                && event.requestor == self.query_window
                && event.selection == selection
            {
                ensure!(
                    event.property != NONE,
                    "Clipboard owner cannot describe its formats"
                );
                let data = self
                    .connection
                    .get_property(true, self.query_window, property, AtomEnum::ATOM, 0, 1024)?
                    .reply()?;
                ensure!(
                    data.bytes_after == 0,
                    "Clipboard advertises too many formats"
                );
                return Ok(data
                    .value32()
                    .context("Invalid clipboard format list")?
                    .collect());
            }
            ensure!(Instant::now() < deadline, "Clipboard owner did not respond");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn restore(&mut self, selection: Selection, own_window: u32, text: &str) -> Result<()> {
        if self.owner(selection.atom)? != own_window {
            return Ok(());
        }
        // The same owner may replace its content without changing its window ID.
        if self
            .clipboard
            .get()
            .clipboard(selection.kind)
            .text()
            .ok()
            .as_deref()
            != Some(text)
        {
            return Ok(());
        }
        let set = self.clipboard.set().clipboard(selection.kind);
        match selection.content {
            Content::Empty => self.clipboard.clear_with().clipboard(selection.kind)?,
            Content::Text(text) => set.text(text)?,
            Content::Html(html, text) => set.html(html, text)?,
            Content::Image(image) => set.image(image)?,
            Content::Files(files) => set.file_list(&files)?,
        }
        Ok(())
    }

    fn keycodes(&self) -> Result<(u8, u8)> {
        let setup = self.connection.setup();
        let map = self
            .connection
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)?
            .reply()?;
        let find = |symbol| {
            map.keysyms
                .chunks(map.keysyms_per_keycode as usize)
                .position(|keys| keys.first() == Some(&symbol))
                .map(|index| setup.min_keycode + index as u8)
                .with_context(|| format!("X11 keymap has no key for keysym {symbol:#x}"))
        };
        Ok((find(0xffe1)?, find(0xff63)?))
    }

    fn wait_for_modifiers(&self) -> Result<()> {
        let modifiers = self.connection.get_modifier_mapping()?.reply()?.keycodes;
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let pressed = self.connection.query_keymap()?.reply()?.keys;
            if !modifiers
                .iter()
                .any(|&key| key != 0 && pressed[key as usize / 8] & (1 << (key % 8)) != 0)
            {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Release modifier keys before dictation is inserted"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn paste_keys(&self, shift: u8, insert: u8) -> Result<()> {
        let send = |event, key| -> Result<()> {
            self.connection
                .xtest_fake_input(event, key, CURRENT_TIME, NONE, 0, 0, 0)?
                .check()?;
            Ok(())
        };
        let press = send(KEY_PRESS_EVENT, shift).and_then(|()| send(KEY_PRESS_EVENT, insert));
        // Always attempt releases, even if the server rejects an earlier request.
        let release_insert = send(KEY_RELEASE_EVENT, insert);
        let release_shift = send(KEY_RELEASE_EVENT, shift);
        self.connection.flush()?;
        press.and(release_insert).and(release_shift)
    }
}

impl TextOutput for X11TextOutput {
    fn insert(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        self.wait_for_modifiers()?;
        let (shift, insert) = self.keycodes()?;
        let clipboard = self.snapshot(LinuxClipboardKind::Clipboard, self.clipboard_atom)?;
        let primary = self.snapshot(LinuxClipboardKind::Primary, AtomEnum::PRIMARY.into())?;
        ensure!(
            self.owner(clipboard.atom)? == clipboard.owner
                && self.owner(primary.atom)? == primary.owner,
            "Clipboard changed while preparing dictation"
        );
        self.clipboard.set().clipboard(clipboard.kind).text(text)?;
        let clipboard_owner = self.owner(clipboard.atom)?;
        if let Err(error) = self.clipboard.set().clipboard(primary.kind).text(text) {
            self.restore(clipboard, clipboard_owner, text)?;
            return Err(error.into());
        }
        let primary_owner = self.owner(primary.atom)?;
        let paste = self.paste_keys(shift, insert);
        thread::sleep(self.paste_delay);
        let restore_clipboard = self.restore(clipboard, clipboard_owner, text);
        let restore_primary = self.restore(primary, primary_owner, text);
        paste.and(restore_clipboard).and(restore_primary)
    }
}
