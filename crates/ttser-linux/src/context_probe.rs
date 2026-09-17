mod focus;

use anyhow::{Context, Result, bail, ensure};
use atspi::{
    Interface, MatchType, ObjectMatchRule, Role, SortOrder, State,
    object_ref::ObjectRefOwned,
    proxy::{
        accessible::{AccessibleProxy, ObjectRefExt},
        bus::BusProxy,
        collection::CollectionProxy,
        hyperlink::HyperlinkProxy,
        hypertext::HypertextProxy,
        registry::RegistryProxy,
        text::TextProxy,
    },
    zbus::{self, proxy::CacheProperties},
};
use serde::Serialize;
use std::{
    collections::{HashSet, VecDeque},
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use ttser_core::{
    observation::{RecordingObservation, RecordingObserver},
    speech::Transcript,
};
use x11rb::{
    connection::Connection,
    protocol::xproto::{AtomEnum, ConnectionExt},
    rust_connection::RustConnection,
};

const BEFORE: i32 = 1500;
const AFTER: i32 = 500;
const SELECTED: i32 = 500;
const TIMEOUT: Duration = Duration::from_millis(750);
const MAX_NODES: usize = 1024;

#[derive(Default, Serialize)]
struct Payload {
    application: String,
    window_title: String,
    before_cursor: String,
    after_cursor: String,
    selected_text: String,
}

#[derive(Default, Serialize)]
struct Snapshot {
    version: u32,
    started_at_unix_ms: u64,
    capture_delay_ms: u64,
    capture_ms: u64,
    window_id: u32,
    process_id: Option<u32>,
    status: String,
    role: Option<String>,
    focused_role: Option<String>,
    text_depth: usize,
    embedded_objects_omitted: bool,
    character_count: Option<i32>,
    caret_offset: Option<i32>,
    selection: Option<(i32, i32)>,
    nodes_visited: usize,
    focus_source: Option<String>,
    focus_age_ms: Option<u64>,
    context: Payload,
}

#[derive(Serialize)]
struct Outcome {
    history_directory: Option<PathBuf>,
    transcript: Option<String>,
    outcome: String,
}

enum Job {
    Capture {
        directory: PathBuf,
        started: Instant,
        unix_ms: u64,
    },
    Finish {
        directory: PathBuf,
        outcome: Outcome,
    },
}

pub struct ContextRecorder {
    root: PathBuf,
    sender: Option<mpsc::Sender<Job>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ContextRecorder {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)?;
        let root = root.canonicalize()?;
        let (sender, mut receiver) = mpsc::channel(16);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let worker = thread::Builder::new()
            .name("context-probe".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let connection = tokio::time::timeout(Duration::from_secs(2), connect()).await;
                    let connection = match connection {
                        Ok(Ok(connection)) => Ok(connection),
                        Ok(Err(error)) => Err(format!("AT-SPI unavailable: {error:#}")),
                        Err(_) => Err("AT-SPI connection timed out".into()),
                    };
                    let tracker = match &connection {
                        Ok(connection) => match focus::Tracker::start(connection.clone()).await {
                            Ok(tracker) => Some(tracker),
                            Err(error) => {
                                eprintln!("Context focus tracking unavailable: {error:#}");
                                None
                            }
                        },
                        Err(_) => None,
                    };
                    while let Some(job) = receiver.recv().await {
                        let result = match job {
                            Job::Capture {
                                directory,
                                started,
                                unix_ms,
                            } => {
                                let snapshot =
                                    capture(&connection, tracker.as_ref(), started, unix_ms).await;
                                write_report(&directory, "context.json", &snapshot)
                            }
                            Job::Finish { directory, outcome } => {
                                write_report(&directory, "result.json", &outcome)
                            }
                        };
                        if let Err(error) = result {
                            eprintln!("Context probe report failed: {error:#}");
                        }
                    }
                });
            })?;
        Ok(Self {
            root,
            sender: Some(sender),
            worker: Some(worker),
        })
    }
}

impl RecordingObserver for ContextRecorder {
    fn started(&self) -> Option<Box<dyn RecordingObservation>> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
        let directory = self
            .root
            .join(format!("{}-{}", now.as_nanos(), std::process::id()));
        let sender = self.sender.as_ref()?;
        if sender
            .try_send(Job::Capture {
                directory: directory.clone(),
                started: Instant::now(),
                unix_ms: now.as_millis() as u64,
            })
            .is_err()
        {
            eprintln!("Context probe busy; skipping this recording");
            return None;
        }
        Some(Box::new(Observation {
            directory,
            sender: sender.clone(),
            finished: false,
        }))
    }
}

impl Drop for ContextRecorder {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Observation {
    directory: PathBuf,
    sender: mpsc::Sender<Job>,
    finished: bool,
}

impl Observation {
    fn send(&self, outcome: Outcome) {
        if self
            .sender
            .try_send(Job::Finish {
                directory: self.directory.clone(),
                outcome,
            })
            .is_err()
        {
            eprintln!("Context probe busy; result could not be saved");
        }
    }
}

impl RecordingObservation for Observation {
    fn finished(
        mut self: Box<Self>,
        history_directory: Option<&Path>,
        transcript: Option<&Transcript>,
        outcome: &str,
    ) {
        self.send(Outcome {
            history_directory: history_directory.map(Path::to_owned),
            transcript: transcript.map(|t| t.text.clone()),
            outcome: outcome.into(),
        });
        self.finished = true;
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        if !self.finished {
            self.send(Outcome {
                history_directory: None,
                transcript: None,
                outcome: "discarded".into(),
            });
        }
    }
}

fn write_report(directory: &Path, name: &str, value: &impl Serialize) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    let temporary = directory.join(format!("{name}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    fs::rename(temporary, directory.join(name))?;
    Ok(())
}

fn truncate(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

fn property(connection: &RustConnection, window: u32, name: &[u8], words: u32) -> Result<Vec<u8>> {
    let atom = connection.intern_atom(false, name)?.reply()?.atom;
    Ok(connection
        .get_property(false, window, atom, AtomEnum::ANY, 0, words)?
        .reply()?
        .value)
}

fn number(connection: &RustConnection, window: u32, name: &[u8]) -> Result<u32> {
    let bytes = property(connection, window, name, 1)?;
    Ok(u32::from_ne_bytes(
        bytes.get(..4).context("Missing X11 property")?.try_into()?,
    ))
}

fn active_window(connection: &RustConnection, root: u32) -> Result<u32> {
    number(connection, root, b"_NET_ACTIVE_WINDOW")
}

fn metadata(snapshot: &mut Snapshot) -> Result<(RustConnection, u32)> {
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    let window = active_window(&connection, root)?;
    ensure!(window != 0, "No active window");
    snapshot.window_id = window;
    snapshot.process_id = number(&connection, window, b"_NET_WM_PID").ok();
    let class = property(&connection, window, b"WM_CLASS", 100)?;
    snapshot.context.application = truncate(
        String::from_utf8_lossy(&class).replace('\0', " ").trim(),
        100,
    );
    let title = property(&connection, window, b"_NET_WM_NAME", 200)?;
    snapshot.context.window_title = truncate(&String::from_utf8_lossy(&title), 200);
    Ok((connection, root))
}

async fn connect() -> Result<zbus::Connection> {
    let session = zbus::Connection::session().await?;
    let address = BusProxy::new(&session).await?.get_address().await?;
    let connection = zbus::connection::Builder::address(address.as_str())?
        .build()
        .await?;
    // Registering a real accessibility client lets toolkits enable their lazy trees.
    RegistryProxy::new(&connection)
        .await?
        .register_event("object:state-changed:focused")
        .await?;
    Ok(connection)
}

async fn capture(
    connection: &Result<zbus::Connection, String>,
    tracker: Option<&focus::Tracker>,
    started: Instant,
    unix_ms: u64,
) -> Snapshot {
    let begin = Instant::now();
    let mut snapshot = Snapshot {
        version: 3,
        started_at_unix_ms: unix_ms,
        capture_delay_ms: started.elapsed().as_millis() as u64,
        ..Default::default()
    };
    match metadata(&mut snapshot) {
        Err(error) => snapshot.status = format!("X11 unavailable: {error:#}"),
        Ok((x11, root)) => {
            snapshot.status = match connection {
                Err(error) => error.clone(),
                Ok(connection) => {
                    match tokio::time::timeout(
                        TIMEOUT,
                        read_focused(connection, tracker, &mut snapshot),
                    )
                    .await
                    {
                        Ok(Ok(())) => "captured".into(),
                        Ok(Err(error)) => format!("{error:#}"),
                        Err(_) => "AT-SPI capture timed out".into(),
                    }
                }
            };
            if active_window(&x11, root).ok() != Some(snapshot.window_id) {
                snapshot.context.before_cursor.clear();
                snapshot.context.after_cursor.clear();
                snapshot.context.selected_text.clear();
                snapshot.status = "Active window changed during capture; text discarded".into();
            }
        }
    }
    snapshot.capture_ms = begin.elapsed().as_millis() as u64;
    snapshot
}

async fn read_focused(
    connection: &zbus::Connection,
    tracker: Option<&focus::Tracker>,
    snapshot: &mut Snapshot,
) -> Result<()> {
    if let Some(remembered) = tracker.and_then(|tracker| tracker.current(snapshot))
        && let Ok(proxy) = remembered.object.as_accessible_proxy(connection).await
        && let Ok(states) = proxy.get_state().await
        && !states.contains(State::Defunct)
        && states.contains(State::Showing)
    {
        let focused = states.contains(State::Focused);
        let top = remembered.toplevel.as_accessible_proxy(connection).await?;
        let window_active = top.get_state().await?.contains(State::Active);
        // A different field can gain focus inside the same window. Reuse an unfocused
        // handle only when the entire window temporarily lost accessibility focus.
        if focused || !window_active {
            snapshot.focus_source = Some(
                if focused {
                    "live"
                } else {
                    "remembered_before_grab"
                }
                .into(),
            );
            snapshot.focus_age_ms = Some(remembered.observed.elapsed().as_millis() as u64);
            return read_text(&proxy, connection, snapshot).await;
        }
    }
    let mut last_error = None;
    for _ in 0..5 {
        match find_focused(connection, snapshot).await {
            Ok(object) => {
                let proxy = object.as_accessible_proxy(connection).await?;
                snapshot.focus_source = Some("live".into());
                return read_text(&proxy, connection, snapshot).await;
            }
            Err(error) => last_error = Some(error),
        }
        // Accessibility activation and FocusIn notifications arrive asynchronously.
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    Err(last_error.unwrap())
}

async fn find_focused(
    connection: &zbus::Connection,
    snapshot: &mut Snapshot,
) -> Result<ObjectRefOwned> {
    snapshot.nodes_visited = 0;
    let pid = snapshot
        .process_id
        .context("Active window has no process ID")?;
    let desktop = AccessibleProxy::builder(connection)
        .destination("org.a11y.atspi.Registry")?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    let bus = zbus::fdo::DBusProxy::new(connection).await?;
    let mut application = None;
    for index in 0..desktop.child_count().await?.clamp(0, 128) {
        let object = desktop.get_child_at_index(index).await?;
        let Some(name) = object.name() else { continue };
        if bus
            .get_connection_unix_process_id(name.clone().into())
            .await
            .ok()
            == Some(pid)
        {
            application = Some(object);
            break;
        }
    }
    let application = application.context("Active application is not available through AT-SPI")?;
    let app = application.as_accessible_proxy(connection).await?;
    let mut active = None;
    for index in 0..app.child_count().await?.clamp(0, 64) {
        let object = app.get_child_at_index(index).await?;
        if object.is_null() {
            continue;
        }
        let proxy = object.as_accessible_proxy(connection).await?;
        // Some toolkits materialize web content only after extended properties are read.
        let _ = proxy.get_attributes().await;
        if proxy.get_state().await?.contains(State::Active) {
            active = Some(object);
            break;
        }
    }
    let active = active.context("AT-SPI does not expose an active window")?;
    let proxy = active.as_accessible_proxy(connection).await?;
    if proxy
        .get_interfaces()
        .await?
        .contains(Interface::Collection)
    {
        let collection = CollectionProxy::builder(connection)
            .destination(proxy.inner().destination().clone())?
            .path(proxy.inner().path().clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let rule = ObjectMatchRule::builder()
            .states([State::Focused], MatchType::All)
            .build();
        if let Ok(objects) = collection
            .get_matches(rule, SortOrder::Canonical, 8, true)
            .await
        {
            for object in objects {
                if object.is_null() {
                    continue;
                }
                let proxy = object.as_accessible_proxy(connection).await?;
                if proxy.get_state().await?.contains(State::Focused) {
                    return Ok(object);
                }
            }
        }
    }
    // Collection is optional in AT-SPI; bounded traversal also supports other toolkits.
    let mut queue = VecDeque::from([active]);
    let mut seen = HashSet::new();
    while let Some(object) = queue.pop_front() {
        if object.is_null()
            || !seen.insert((
                object.name_as_str().unwrap_or_default().to_owned(),
                object.path_as_str().to_owned(),
            ))
        {
            continue;
        }
        snapshot.nodes_visited += 1;
        let proxy = object.as_accessible_proxy(connection).await?;
        if proxy.get_state().await?.contains(State::Focused) {
            return Ok(object);
        }
        let remaining = MAX_NODES.saturating_sub(snapshot.nodes_visited + queue.len());
        for index in 0..proxy.child_count().await?.clamp(0, remaining as i32) {
            queue.push_back(proxy.get_child_at_index(index).await?);
        }
    }
    bail!("No focused accessible field found (search limit: {MAX_NODES} nodes)")
}

fn ranges(count: i32, caret: i32) -> Result<((i32, i32), (i32, i32))> {
    ensure!(
        count >= 0 && (0..=count).contains(&caret),
        "Invalid or unavailable caret offset"
    );
    Ok((
        ((caret - BEFORE).max(0), caret),
        (caret, caret.saturating_add(AFTER).min(count)),
    ))
}

async fn read_text(
    proxy: &AccessibleProxy<'_>,
    connection: &zbus::Connection,
    snapshot: &mut Snapshot,
) -> Result<()> {
    snapshot.focused_role = Some(proxy.get_role().await?.name().into());
    let mut object = ObjectRefOwned::try_from(proxy)?;
    let mut resolved = None;
    for depth in 0..8 {
        let current = object.as_accessible_proxy(connection).await?;
        let role = current.get_role().await?;
        ensure!(role != Role::PasswordText, "Password field omitted");
        let interfaces = current.get_interfaces().await?;
        ensure!(
            interfaces.contains(Interface::Text),
            "Focused element has no text interface"
        );
        let text = TextProxy::builder(connection)
            .destination(current.inner().destination().clone())?
            .path(current.inner().path().clone())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let count = text.character_count().await?;
        let caret = text.caret_offset().await?;
        ranges(count, caret)?;
        let offset = caret.min(count.saturating_sub(1));
        if count > 0 && text.get_text(offset, offset + 1).await? == "\u{fffc}" {
            ensure!(
                interfaces.contains(Interface::Hypertext),
                "Embedded text has no hypertext interface"
            );
            let hypertext = HypertextProxy::builder(connection)
                .destination(current.inner().destination().clone())?
                .path(current.inner().path().clone())?
                .build()
                .await?;
            let index = hypertext.get_link_index(offset).await?;
            ensure!(index >= 0, "Embedded object at caret cannot be resolved");
            let link = hypertext.get_link(index).await?;
            let hyperlink = HyperlinkProxy::builder(connection)
                .destination(link.name().context("Null embedded link")?.clone())?
                .path(link.path().clone())?
                .build()
                .await?;
            object = hyperlink.get_object(0).await?;
            continue;
        }
        snapshot.role = Some(role.name().into());
        snapshot.text_depth = depth;
        snapshot.character_count = Some(count);
        snapshot.caret_offset = Some(caret);
        resolved = Some((object.clone(), count, caret));
        break;
    }
    let (object, count, caret) = resolved.context("Embedded text nesting limit exceeded")?;
    let text = TextProxy::builder(connection)
        .destination(object.name().context("Null text object")?.clone())?
        .path(object.path().clone())?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    let ((start, _), (_, end)) = ranges(count, caret)?;
    snapshot.context.before_cursor = truncate(&text.get_text(start, caret).await?, BEFORE as usize);
    snapshot.context.after_cursor = truncate(&text.get_text(caret, end).await?, AFTER as usize);
    if text.get_n_selections().await.unwrap_or(0) > 0 {
        let (start, end) = text.get_selection(0).await?;
        ensure!(
            0 <= start && start <= end && end <= count,
            "Invalid selection offsets"
        );
        snapshot.selection = Some((start, end));
        snapshot.context.selected_text = if end - start <= SELECTED {
            truncate(&text.get_text(start, end).await?, SELECTED as usize)
        } else {
            let head = text.get_text(start, start + SELECTED / 2).await?;
            let tail = text.get_text(end - SELECTED / 2 + 1, end).await?;
            truncate(&head, (SELECTED / 2) as usize)
                + "…"
                + &truncate(&tail, (SELECTED / 2 - 1) as usize)
        };
    }
    for fragment in [
        &mut snapshot.context.before_cursor,
        &mut snapshot.context.after_cursor,
        &mut snapshot.context.selected_text,
    ] {
        if fragment.contains('\u{fffc}') {
            snapshot.embedded_objects_omitted = true;
            *fragment = fragment.replace('\u{fffc}', "");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn bounded_ranges_preserve_cursor_in_large_and_empty_fields() {
        assert_eq!(
            ranges(1_000_000, 500_000).unwrap(),
            ((498_500, 500_000), (500_000, 500_500))
        );
        assert_eq!(ranges(10, 0).unwrap(), ((0, 0), (0, 10)));
        assert_eq!(ranges(0, 0).unwrap(), ((0, 0), (0, 0)));
        assert!(ranges(10, -1).is_err());
        assert!(ranges(10, 11).is_err());
        assert_eq!(truncate("ąż🙂z", 3), "ąż🙂");
        const { assert!(BEFORE + AFTER + SELECTED + 100 + 200 <= 3000) };
    }

    #[test]
    fn reports_are_private_and_separate_from_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("probe");
        write_report(
            &root,
            "result.json",
            &Outcome {
                history_directory: Some(PathBuf::from("/history/123")),
                transcript: Some("Zażółć".into()),
                outcome: "inserted".into(),
            },
        )
        .unwrap();
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("result.json")).unwrap()).unwrap();
        assert_eq!(record["history_directory"], "/history/123");
        assert_eq!(record["transcript"], "Zażółć");
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.join("result.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn finished_and_discarded_recordings_keep_their_own_identity() {
        let (sender, mut receiver) = mpsc::channel(4);
        let first = Box::new(Observation {
            directory: "first".into(),
            sender: sender.clone(),
            finished: false,
        });
        let second = Box::new(Observation {
            directory: "second".into(),
            sender,
            finished: false,
        });
        second.finished(Some(Path::new("history/second")), None, "empty");
        drop(first);
        let Job::Finish { directory, outcome } = receiver.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(directory, Path::new("second"));
        assert_eq!(
            outcome.history_directory.unwrap(),
            Path::new("history/second")
        );
        let Job::Finish { directory, outcome } = receiver.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(directory, Path::new("first"));
        assert_eq!(outcome.outcome, "discarded");
        assert!(receiver.try_recv().is_err());
    }
}
