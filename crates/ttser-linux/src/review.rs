use anyhow::{Context, Result, ensure};
use gtk4::{gdk, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    CURRENT_TIME,
    connection::Connection,
    protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, InputFocus, PropMode,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

struct Dialog(gtk4::Window);

impl Drop for Dialog {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

struct Request {
    text: String,
    target: u32,
    cancelled: Arc<AtomicBool>,
    reply: Sender<Result<Option<String>>>,
}

fn editor() -> &'static Sender<Request> {
    static EDITOR: OnceLock<Sender<Request>> = OnceLock::new();
    EDITOR.get_or_init(|| {
        let (requests, input) = mpsc::channel();
        // GTK must keep serving copied text even after the review window closes.
        thread::spawn(move || run_editor(input));
        requests
    })
}

fn run_editor(input: Receiver<Request>) {
    loop {
        match input.recv_timeout(Duration::from_millis(10)) {
            Ok(request) => {
                let result = show(&request.text, request.target, &request.cancelled);
                let _ = request.reply.send(result);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if gtk4::is_initialized_main_thread() {
            let context = glib::MainContext::default();
            // Bound each batch so clipboard traffic cannot starve new reviews.
            for _ in 0..64 {
                if !context.iteration(false) {
                    break;
                }
            }
        }
    }
}

pub(crate) fn review(
    connection: &RustConnection,
    text: &str,
    cancelled: &AtomicBool,
) -> Result<Option<(String, Destination)>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let destination = Destination::capture(connection)?;
    let dismiss = Arc::new(AtomicBool::new(false));
    let (reply, response) = mpsc::channel();
    editor()
        .send(Request {
            text: text.to_owned(),
            target: destination.target,
            cancelled: dismiss.clone(),
            reply,
        })
        .map_err(|_| anyhow::anyhow!("Transcript editor stopped"))?;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            dismiss.store(true, Ordering::Relaxed);
        }
        match response.recv_timeout(Duration::from_millis(10)) {
            Ok(result) => {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                return Ok(result?.map(|text| (text, destination)));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => anyhow::bail!("Transcript editor stopped"),
        }
    }
}

fn show(text: &str, target: u32, cancelled: &AtomicBool) -> Result<Option<String>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    ensure!(
        !gtk4::is_initialized() || gtk4::is_initialized_main_thread(),
        "Transcript review must stay on the thread that initialized GTK"
    );
    if !gtk4::is_initialized() {
        gdk::set_allowed_backends("x11");
        gtk4::init().context("Opening transcript editor (requires an X11 display and GTK 4)")?;
    }
    let dialog = Dialog(
        gtk4::Window::builder()
            .title("ttser — popraw transkrypcję")
            .default_width(620)
            .default_height(240)
            .modal(true)
            .build(),
    );
    let window = &dialog.0;
    let content = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();
    let editor = gtk4::TextView::builder()
        .wrap_mode(gtk4::WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    let buffer = editor.buffer();
    buffer.set_text(text);
    buffer.place_cursor(&buffer.end_iter());
    // Selecting a correction must not replace the user's PRIMARY selection.
    editor.connect_realize(|view| {
        view.buffer()
            .remove_selection_clipboard(&view.primary_clipboard());
    });
    editor.connect_unrealize(|view| {
        let buffer = view.buffer();
        // Balance TextView's teardown without reclaiming PRIMARY with a selection.
        buffer.place_cursor(&buffer.end_iter());
        buffer.add_selection_clipboard(&view.primary_clipboard());
    });
    content.append(
        &gtk4::ScrolledWindow::builder()
            .vexpand(true)
            .child(&editor)
            .build(),
    );
    let footer = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    footer.append(
        &gtk4::Label::builder()
            .label("Enter: wklej  ·  Shift+Enter: nowa linia  ·  Esc: anuluj")
            .hexpand(true)
            .xalign(0.0)
            .build(),
    );
    let submit = gtk4::Button::with_label("Wklej");
    footer.append(&submit);
    content.append(&footer);
    window.set_child(Some(&content));

    let result = Rc::new(RefCell::new(None));
    let accept = {
        let result = result.clone();
        move || {
            let (start, end) = buffer.bounds();
            *result.borrow_mut() = Some(Some(buffer.text(&start, &end, true).to_string()));
        }
    };
    submit.connect_clicked({
        let accept = accept.clone();
        move |_| accept()
    });
    window.connect_close_request({
        let result = result.clone();
        move |_| {
            *result.borrow_mut() = Some(None);
            glib::Propagation::Stop
        }
    });
    let keys = gtk4::EventControllerKey::new();
    keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let pending_enter = Rc::new(Cell::new(false));
    keys.connect_key_pressed({
        let result = result.clone();
        let pending_enter = pending_enter.clone();
        move |_, key, _, modifiers| {
            if key == gdk::Key::Escape {
                *result.borrow_mut() = Some(None);
                return glib::Propagation::Stop;
            }
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
                && (pending_enter.get() || !modifiers.contains(gdk::ModifierType::SHIFT_MASK))
            {
                // Retain focus through auto-repeat so Enter cannot reach the destination.
                pending_enter.set(true);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    });
    keys.connect_key_released(move |_, key, _, _| {
        if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) && pending_enter.replace(false) {
            accept();
        }
    });
    window.add_controller(keys);
    WidgetExt::realize(window);
    let surface = window
        .surface()
        .and_downcast::<gdk4_x11::X11Surface>()
        .context("Transcript editor requires the X11 GTK backend")?;
    let xid = u32::try_from(surface.xid())?;
    // Publish the GDK-created window before addressing it over our X11 connection.
    surface.display().sync();
    let (connection, _) = x11rb::connect(None).context("Setting transcript editor window hints")?;
    let window_type = connection
        .intern_atom(false, b"_NET_WM_WINDOW_TYPE")?
        .reply()?
        .atom;
    let dialog_type = connection
        .intern_atom(false, b"_NET_WM_WINDOW_TYPE_DIALOG")?
        .reply()?
        .atom;
    connection
        .change_property32(
            PropMode::REPLACE,
            xid,
            AtomEnum::WM_TRANSIENT_FOR,
            AtomEnum::WINDOW,
            &[target],
        )?
        .check()?;
    connection
        .change_property32(
            PropMode::REPLACE,
            xid,
            window_type,
            AtomEnum::ATOM,
            &[dialog_type],
        )?
        .check()?;
    window.present();
    editor.grab_focus();

    let context = glib::MainContext::default();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if let Some(result) = result.borrow_mut().take() {
            return Ok(result);
        }
        // Nonblocking iterations let shutdown interrupt review without a GTK callback
        // retaining the caller's cancellation flag beyond this invocation.
        if !context.iteration(false) {
            thread::sleep(Duration::from_millis(10));
        }
    }
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
