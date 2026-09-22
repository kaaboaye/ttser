use super::*;
use std::sync::mpsc;
use x11rb::{
    CURRENT_TIME,
    protocol::xproto::{CreateWindowAux, WindowClass},
};

fn window(connection: &RustConnection) -> u32 {
    let id = connection.generate_id().unwrap();
    connection
        .create_window(
            0,
            id,
            connection.setup().roots[0].root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .unwrap()
        .check()
        .unwrap();
    id
}

fn notify(connection: &RustConnection) -> SelectionNotifyEvent {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(Event::SelectionNotify(event)) = connection.poll_for_event().unwrap() {
            return event;
        }
        assert!(Instant::now() < deadline, "No selection response");
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "requires an isolated X11 server"]
fn x11_paste_waits_for_delivery() {
    for (text, acknowledge) in [
        ("Zażółć 🦀".to_owned(), true),
        ("Żółw 🐢 ".repeat(12000), true),
        ("not consumed".to_owned(), false),
    ] {
        let (client, _) = x11rb::connect(None).unwrap();
        let destination_window = window(&client);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let payload = text.clone();
        let server = thread::spawn(move || {
            let (connection, _) = x11rb::connect(None).unwrap();
            let owner = window(&connection);
            let clipboard = connection
                .intern_atom(false, b"CLIPBOARD")
                .unwrap()
                .reply()
                .unwrap()
                .atom;
            let mut paste = Paste::new(
                &connection,
                owner,
                [clipboard, AtomEnum::PRIMARY.into()],
                destination_window,
            )
            .unwrap();
            paste.claim().unwrap();
            ready_tx.send(clipboard).unwrap();
            let result = paste.wait(&payload, Duration::from_millis(50));
            done_tx.send(result).unwrap();
        });
        let clipboard = ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let requestor = window(&client);
        let atoms = Atoms::new(&client).unwrap().reply().unwrap();
        let property = atoms._TTSER_PASTE_TIME;
        let convert = |target| {
            client
                .convert_selection(requestor, clipboard, target, property, CURRENT_TIME)
                .unwrap()
                .check()
                .unwrap();
            let event = notify(&client);
            assert_eq!(event.property, property);
        };
        convert(atoms.TARGETS);
        client
            .get_property(true, requestor, property, AtomEnum::ATOM, 0, 1024)
            .unwrap()
            .reply()
            .unwrap();
        // Format negotiation must not let restoration race a slow editor's text read.
        assert!(done_rx.recv_timeout(Duration::from_millis(900)).is_err());
        convert(atoms.UTF8_STRING);
        let initial = client
            .get_property(false, requestor, property, AtomEnum::ANY, 0, u32::MAX)
            .unwrap()
            .reply()
            .unwrap();
        // Neither conversion nor reading without deletion acknowledges delivery.
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
        if acknowledge {
            let mut received = Vec::new();
            if initial.type_ == atoms.INCR {
                loop {
                    client
                        .delete_property(requestor, property)
                        .unwrap()
                        .check()
                        .unwrap();
                    let deadline = Instant::now() + Duration::from_secs(3);
                    let chunk = loop {
                        let data = client
                            .get_property(false, requestor, property, AtomEnum::ANY, 0, u32::MAX)
                            .unwrap()
                            .reply()
                            .unwrap();
                        if data.type_ == atoms.UTF8_STRING {
                            break data.value;
                        }
                        assert!(Instant::now() < deadline, "Missing INCR chunk");
                        thread::sleep(Duration::from_millis(2));
                    };
                    if chunk.is_empty() {
                        break;
                    }
                    received.extend(chunk);
                }
            } else {
                received = initial.value;
            }
            assert_eq!(received, text.as_bytes());
            client
                .delete_property(requestor, property)
                .unwrap()
                .check()
                .unwrap();
            done_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
        } else {
            let error = done_rx
                .recv_timeout(Duration::from_secs(6))
                .unwrap()
                .unwrap_err();
            assert!(error.to_string().contains("did not finish reading"));
        }
        server.join().unwrap();
    }
}

fn read_property(
    connection: &RustConnection,
    window: u32,
    property: Atom,
) -> x11rb::protocol::xproto::GetPropertyReply {
    connection
        .get_property(false, window, property, AtomEnum::ANY, 0, u32::MAX)
        .unwrap()
        .reply()
        .unwrap()
}

fn next_chunk(connection: &RustConnection, window: u32, property: Atom, target: Atom) -> Vec<u8> {
    connection
        .delete_property(window, property)
        .unwrap()
        .check()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let data = read_property(connection, window, property);
        if data.type_ == target {
            assert_eq!(data.bytes_after, 0);
            return data.value;
        }
        assert!(Instant::now() < deadline, "Missing INCR chunk");
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "requires an isolated X11 server"]
fn x11_paste_abandoned_readers_do_not_block_delivery_or_later_pastes() {
    let cases = [
        ("Zażółć 🦀".to_owned(), Some(false)),
        ("Żółw 🐢 ".repeat(12000), Some(false)),
        ("Żółw 🐢 ".repeat(12000), Some(true)),
        ("Next dictation".to_owned(), None),
    ];
    let payloads = cases.clone();
    let (client, _) = x11rb::connect(None).unwrap();
    let destination_window = window(&client);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (connection, _) = x11rb::connect(None).unwrap();
        let owner = window(&connection);
        let clipboard = connection
            .intern_atom(false, b"CLIPBOARD")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        // The daemon reuses this connection, so cleanup errors must not leak into the next paste.
        for (text, _) in payloads {
            let mut paste = Paste::new(
                &connection,
                owner,
                [clipboard, AtomEnum::PRIMARY.into()],
                destination_window,
            )
            .unwrap();
            paste.claim().unwrap();
            ready_tx.send(clipboard).unwrap();
            let result = paste.wait(&text, Duration::from_millis(50));
            drop(paste);
            done_tx.send(result).unwrap();
        }
    });
    let atoms = Atoms::new(&client).unwrap().reply().unwrap();
    let property = atoms._TTSER_PASTE_TIME;
    for (text, read_first_chunk) in cases {
        let clipboard = ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let request = |window, target| {
            client
                .convert_selection(window, clipboard, target, property, CURRENT_TIME)
                .unwrap()
                .check()
                .unwrap();
            let event = notify(&client);
            assert_eq!(event.requestor, window);
            assert_eq!(event.property, property);
            read_property(&client, window, property)
        };
        if let Some(read_first_chunk) = read_first_chunk {
            let auxiliary = window(&client);
            let data = request(auxiliary, atoms.text_utf8);
            if data.type_ == atoms.INCR {
                if read_first_chunk {
                    let chunk = next_chunk(&client, auxiliary, property, atoms.text_utf8);
                    assert!(!chunk.is_empty());
                    assert!(text.as_bytes().starts_with(&chunk));
                }
            } else {
                assert_eq!(data.value, text.as_bytes());
            }
            client.destroy_window(auxiliary).unwrap().check().unwrap();
            assert!(
                matches!(
                    done_rx.recv_timeout(Duration::from_millis(150)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ),
                "An abandoned read must not count as delivery"
            );
        }
        let destination = window(&client);
        let data = request(destination, atoms.UTF8_STRING);
        let received = if data.type_ == atoms.INCR {
            let mut received = Vec::new();
            loop {
                let chunk = next_chunk(&client, destination, property, atoms.UTF8_STRING);
                if chunk.is_empty() {
                    break;
                }
                received.extend(chunk);
            }
            received
        } else {
            data.value
        };
        assert_eq!(received, text.as_bytes());
        assert!(
            matches!(
                done_rx.recv_timeout(Duration::from_millis(150)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "A live destination must still acknowledge delivery"
        );
        client
            .delete_property(destination, property)
            .unwrap()
            .check()
            .unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        client.destroy_window(destination).unwrap().check().unwrap();
    }
    server.join().unwrap();
}

#[test]
#[ignore = "requires an isolated X11 server"]
fn x11_paste_background_acknowledgement_cannot_replace_destination_delivery() {
    for destination_reads in [true, false] {
        let (destination, _) = x11rb::connect(None).unwrap();
        let destination_window = window(&destination);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (connection, _) = x11rb::connect(None).unwrap();
            let owner = window(&connection);
            let clipboard = connection
                .intern_atom(false, b"CLIPBOARD")
                .unwrap()
                .reply()
                .unwrap()
                .atom;
            let mut paste = Paste::new(
                &connection,
                owner,
                [clipboard, AtomEnum::PRIMARY.into()],
                destination_window,
            )
            .unwrap();
            paste.claim().unwrap();
            ready_tx.send(clipboard).unwrap();
            done_tx
                .send(paste.wait("dictation", Duration::from_millis(50)))
                .unwrap();
        });
        let clipboard = ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let (background, _) = x11rb::connect(None).unwrap();
        let atoms = Atoms::new(&background).unwrap().reply().unwrap();
        let read = |connection: &RustConnection, requestor| {
            connection
                .convert_selection(
                    requestor,
                    clipboard,
                    atoms.UTF8_STRING,
                    atoms._TTSER_PASTE_TIME,
                    CURRENT_TIME,
                )
                .unwrap()
                .check()
                .unwrap();
            assert_eq!(notify(connection).property, atoms._TTSER_PASTE_TIME);
            let data = connection
                .get_property(
                    true,
                    requestor,
                    atoms._TTSER_PASTE_TIME,
                    AtomEnum::ANY,
                    0,
                    u32::MAX,
                )
                .unwrap()
                .reply()
                .unwrap();
            assert_eq!(data.value, b"dictation");
        };
        read(&background, window(&background));
        assert!(
            matches!(
                done_rx.recv_timeout(Duration::from_millis(350)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "Background receipt must not restore the clipboard before a slow destination reads it"
        );
        if destination_reads {
            // Browsers request through a hidden sibling, not the focused input window itself.
            read(&destination, window(&destination));
            done_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
        } else {
            let error = done_rx
                .recv_timeout(Duration::from_secs(6))
                .unwrap()
                .unwrap_err();
            assert!(error.to_string().contains("did not finish reading"));
        }
        server.join().unwrap();
    }
}
