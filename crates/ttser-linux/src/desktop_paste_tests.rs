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
            let mut paste =
                Paste::new(&connection, owner, [clipboard, AtomEnum::PRIMARY.into()]).unwrap();
            paste.claim().unwrap();
            ready_tx.send(clipboard).unwrap();
            let result = paste.wait(&payload, Duration::from_millis(50));
            done_tx.send(result).unwrap();
        });
        let clipboard = ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let (client, _) = x11rb::connect(None).unwrap();
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
