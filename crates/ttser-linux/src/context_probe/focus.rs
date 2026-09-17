use super::{Snapshot, TIMEOUT, active_window, find_focused, metadata};
use anyhow::{Context, Result};
use atspi::{
    State,
    events::object::StateChangedEvent,
    object_ref::ObjectRefOwned,
    proxy::{accessible::ObjectRefExt, registry::RegistryProxy},
    zbus,
};
use futures_lite::StreamExt;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct RememberedFocus {
    pub object: ObjectRefOwned,
    pub observed: Instant,
    pub toplevel: ObjectRefOwned,
    window: u32,
    pid: u32,
}

pub(super) struct Tracker {
    latest: Arc<Mutex<Option<RememberedFocus>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Tracker {
    pub async fn start(connection: zbus::Connection) -> Result<Self> {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .path_namespace("/org/a11y/atspi/accessible")?
            .build();
        let mut events = zbus::MessageStream::for_match_rule(rule, &connection, Some(128)).await?;
        let registry = RegistryProxy::new(&connection).await?;
        registry.register_event("window:activate").await?;
        let latest = Arc::new(Mutex::new(None));
        let state = latest.clone();
        let task = tokio::spawn(async move {
            let _ = tokio::time::timeout(TIMEOUT, refresh(&connection, &state)).await;
            while let Some(Ok(message)) = events.next().await {
                let header = message.header();
                if header.interface().map(|value| value.as_str())
                    == Some("org.a11y.atspi.Event.Object")
                {
                    if let Ok(event) = StateChangedEvent::try_from(&message)
                        && event.state == State::Focused
                        && event.enabled
                    {
                        let _ = tokio::time::timeout(
                            TIMEOUT,
                            remember(&connection, &state, event.item),
                        )
                        .await;
                    }
                } else if header.interface().map(|value| value.as_str())
                    == Some("org.a11y.atspi.Event.Window")
                    && header.member().map(|value| value.as_str()) == Some("Activate")
                {
                    let _ = tokio::time::timeout(TIMEOUT, refresh(&connection, &state)).await;
                }
            }
        });
        Ok(Self { latest, task })
    }

    pub fn current(&self, snapshot: &Snapshot) -> Option<RememberedFocus> {
        self.latest
            .lock()
            .ok()?
            .as_ref()
            .filter(|focus| {
                focus.window == snapshot.window_id && Some(focus.pid) == snapshot.process_id
            })
            .cloned()
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type SharedFocus = Arc<Mutex<Option<RememberedFocus>>>;

async fn remember(
    connection: &zbus::Connection,
    state: &SharedFocus,
    object: ObjectRefOwned,
) -> Result<()> {
    let mut snapshot = Snapshot::default();
    let (x11, root) = metadata(&mut snapshot)?;
    let pid = snapshot.process_id.context("No active process")?;
    let name = object.name().context("Null focus object")?;
    let bus = zbus::fdo::DBusProxy::new(connection).await?;
    if bus
        .get_connection_unix_process_id(name.clone().into())
        .await?
        != pid
    {
        return Ok(());
    }
    // A queued FocusIn may describe a field that has already lost focus.
    let proxy = object.as_accessible_proxy(connection).await?;
    if !proxy.get_state().await?.contains(State::Focused)
        || active_window(&x11, root).ok() != Some(snapshot.window_id)
    {
        return Ok(());
    }
    let application = proxy.get_application().await?;
    let mut toplevel = object.clone();
    for _ in 0..32 {
        let parent = toplevel
            .as_accessible_proxy(connection)
            .await?
            .parent()
            .await?;
        if parent.is_null() || parent == application {
            break;
        }
        toplevel = parent;
    }
    *state.lock().unwrap() = Some(RememberedFocus {
        object,
        observed: Instant::now(),
        toplevel,
        window: snapshot.window_id,
        pid,
    });
    Ok(())
}

async fn refresh(connection: &zbus::Connection, state: &SharedFocus) -> Result<()> {
    let mut snapshot = Snapshot::default();
    let (x11, root) = metadata(&mut snapshot)?;
    for _ in 0..5 {
        if active_window(&x11, root).ok() != Some(snapshot.window_id) {
            return Ok(());
        }
        if let Ok(object) = find_focused(connection, &mut snapshot).await {
            return remember(connection, state, object).await;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    Ok(())
}
