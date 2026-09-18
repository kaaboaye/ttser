use anyhow::{Result, ensure};
use std::{
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    NONE,
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask, PropMode,
            Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        TARGETS, TIMESTAMP, UTF8_STRING, TEXT, INCR,
        text_plain: b"text/plain",
        text_utf8: b"text/plain;charset=utf-8",
        text_UTF8: b"text/plain;charset=UTF-8",
        _TTSER_PASTE_TIME,
    }
}

struct Transfer {
    window: u32,
    property: Atom,
    target: Atom,
    data: Vec<u8>,
    offset: Option<usize>,
}

/// Owns the temporary selections until their text has actually been consumed.
pub(super) struct Paste<'a> {
    connection: &'a RustConnection,
    window: u32,
    selections: [Atom; 2],
    atoms: Atoms,
    timestamp: u32,
    chunk_size: usize,
    transfers: Vec<Transfer>,
    requestors: Vec<u32>,
}

impl<'a> Paste<'a> {
    pub(super) fn new(
        connection: &'a RustConnection,
        window: u32,
        selections: [Atom; 2],
    ) -> Result<Self> {
        let atoms = Atoms::new(connection)?.reply()?;
        connection
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                atoms._TTSER_PASTE_TIME,
                AtomEnum::STRING,
                &[],
            )?
            .check()?;
        let timestamp = loop {
            if let Event::PropertyNotify(event) = connection.wait_for_event()?
                && event.window == window
                && event.atom == atoms._TTSER_PASTE_TIME
            {
                break event.time;
            }
        };
        Ok(Self {
            connection,
            window,
            selections,
            atoms,
            timestamp,
            chunk_size: (usize::from(connection.setup().maximum_request_length) * 4 - 64)
                .min(64 * 1024),
            transfers: Vec::new(),
            requestors: Vec::new(),
        })
    }

    pub(super) fn claim(&self) -> Result<()> {
        for selection in self.selections {
            self.connection
                .set_selection_owner(self.window, selection, self.timestamp)?
                .check()?;
            ensure!(
                self.connection
                    .get_selection_owner(selection)?
                    .reply()?
                    .owner
                    == self.window,
                "Clipboard changed while preparing dictation"
            );
        }
        Ok(())
    }

    fn request(&mut self, event: SelectionRequestEvent, text: &str) -> Result<()> {
        let property = if event.property == NONE {
            event.target
        } else {
            event.property
        };
        let mut reply = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: event.time,
            requestor: event.requestor,
            selection: event.selection,
            target: event.target,
            property: NONE,
        };
        if event.target == self.atoms.TARGETS {
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    event.requestor,
                    property,
                    AtomEnum::ATOM,
                    &[
                        self.atoms.TARGETS,
                        self.atoms.TIMESTAMP,
                        self.atoms.UTF8_STRING,
                        self.atoms.TEXT,
                        AtomEnum::STRING.into(),
                        self.atoms.text_plain,
                        self.atoms.text_utf8,
                        self.atoms.text_UTF8,
                    ],
                )?
                .check()?;
            reply.property = property;
        } else if event.target == self.atoms.TIMESTAMP {
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    event.requestor,
                    property,
                    AtomEnum::INTEGER,
                    &[self.timestamp],
                )?
                .check()?;
            reply.property = property;
        } else if [
            self.atoms.UTF8_STRING,
            self.atoms.TEXT,
            AtomEnum::STRING.into(),
            self.atoms.text_plain,
            self.atoms.text_utf8,
            self.atoms.text_UTF8,
        ]
        .contains(&event.target)
        {
            let data = if event.target == AtomEnum::STRING.into() {
                text.chars()
                    .map(|c| u8::try_from(c as u32).unwrap_or(b'?'))
                    .collect()
            } else {
                text.as_bytes().to_vec()
            };
            let target = if event.target == self.atoms.TEXT {
                self.atoms.UTF8_STRING
            } else {
                event.target
            };
            // ICCCM property deletion acknowledges receipt, including the final INCR chunk.
            self.connection
                .change_window_attributes(
                    event.requestor,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                )?
                .check()?;
            if !self.requestors.contains(&event.requestor) {
                self.requestors.push(event.requestor);
            }
            let offset = if data.len() > self.chunk_size {
                self.connection
                    .change_property32(
                        PropMode::REPLACE,
                        event.requestor,
                        property,
                        self.atoms.INCR,
                        &[u32::try_from(data.len())?],
                    )?
                    .check()?;
                Some(0)
            } else {
                self.connection
                    .change_property8(PropMode::REPLACE, event.requestor, property, target, &data)?
                    .check()?;
                None
            };
            self.transfers.push(Transfer {
                window: event.requestor,
                property,
                target,
                data,
                offset,
            });
            reply.property = property;
        }
        self.connection
            .send_event(false, event.requestor, EventMask::NO_EVENT, reply)?
            .check()?;
        Ok(())
    }

    pub(super) fn wait(&mut self, text: &str, settle: Duration) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5) + settle;
        let mut delivered = false;
        let mut last_activity = Instant::now();
        loop {
            // Bound batches so an uncooperative client cannot prevent the timeout.
            for _ in 0..64 {
                let Some(event) = self.connection.poll_for_event()? else {
                    break;
                };
                match event {
                    Event::SelectionRequest(event)
                        if event.owner == self.window
                            && self.selections.contains(&event.selection) =>
                    {
                        self.request(event, text)?;
                        last_activity = Instant::now();
                    }
                    Event::PropertyNotify(event) if event.state == Property::DELETE => {
                        if let Some(index) = self
                            .transfers
                            .iter()
                            .position(|t| t.window == event.window && t.property == event.atom)
                        {
                            let transfer = &mut self.transfers[index];
                            if let Some(offset) = transfer.offset {
                                let end = (offset + self.chunk_size).min(transfer.data.len());
                                self.connection
                                    .change_property8(
                                        PropMode::REPLACE,
                                        transfer.window,
                                        transfer.property,
                                        transfer.target,
                                        &transfer.data[offset..end],
                                    )?
                                    .check()?;
                                transfer.offset = if offset == transfer.data.len() {
                                    None
                                } else {
                                    Some(end)
                                };
                            } else {
                                self.transfers.swap_remove(index);
                                delivered = true;
                            }
                            last_activity = Instant::now();
                        }
                    }
                    _ => {}
                }
            }
            if delivered && self.transfers.is_empty() && last_activity.elapsed() >= settle {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Destination did not finish reading dictated text from the clipboard"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for Paste<'_> {
    fn drop(&mut self) {
        for &window in &self.requestors {
            // Requestors can close at any time; discard cleanup errors but consume them
            // so they cannot interrupt the next insertion on this connection.
            if let Ok(cookie) = self.connection.change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
            ) {
                let _ = cookie.check();
            }
        }
    }
}

#[cfg(test)]
#[path = "desktop_paste_tests.rs"]
mod tests;
