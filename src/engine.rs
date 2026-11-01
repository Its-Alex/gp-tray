use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::time::Duration;

use anyhow::{Result, bail};
use futures_util::StreamExt;
use rtnetlink::packet_core::{NetlinkMessage, NetlinkPayload};
use rtnetlink::packet_route::RouteNetlinkMessage;
use rtnetlink::packet_route::link::{LinkAttribute, LinkFlags, LinkMessage};
use rtnetlink::{Handle, MulticastGroup};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Message, MessageStream};

use crate::paths::TUN_IFNAME;
use crate::systemd;

const RECONNECT_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnState {
    Disconnected,
    Authenticating,
    TunnelStarting,
    Connected,
}

impl VpnState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Authenticating => "authenticating",
            Self::TunnelStarting => "tunnel starting",
            Self::Connected => "connected",
        }
    }
}

impl fmt::Display for VpnState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Disconnected => "Disconnected",
            Self::Authenticating => "Authenticating",
            Self::TunnelStarting => "Tunnel starting",
            Self::Connected => "Connected",
        };
        formatter.write_str(label)
    }
}

/// Everything the tray renders is derived from this snapshot: our systemd
/// units and whether the kernel holds `gp0` up.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Snapshot {
    units: BTreeMap<String, String>,
    link_up: bool,
    authenticating: bool,
}

impl Snapshot {
    fn state(&self) -> VpnState {
        if self.authenticating {
            return VpnState::Authenticating;
        }
        let unit_active = self
            .units
            .values()
            .any(|state| matches!(state.as_str(), "active" | "activating" | "reloading"));
        match (unit_active, self.link_up) {
            (true, false) => VpnState::TunnelStarting,
            (_, true) => VpnState::Connected,
            (false, false) => VpnState::Disconnected,
        }
    }
}

#[derive(Clone)]
pub struct Engine {
    state: watch::Receiver<VpnState>,
    commands: mpsc::UnboundedSender<Command>,
}

enum Command {
    Authenticating(bool),
    Resync,
}

impl Engine {
    /// Starts the state engine and returns only once the first resync has
    /// run, so `state()` is accurate from the first call.
    pub async fn spawn() -> Self {
        let (state_tx, state_rx) = watch::channel(VpnState::Disconnected);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let mut snapshot = Snapshot::default();
        let session = open_session(&mut snapshot).await;
        state_tx.send_replace(snapshot.state());
        tokio::spawn(run(state_tx, command_rx, snapshot, session));
        Self {
            state: state_rx,
            commands: command_tx,
        }
    }

    pub fn state_stream(&self) -> watch::Receiver<VpnState> {
        self.state.clone()
    }

    pub fn state(&self) -> VpnState {
        *self.state.borrow()
    }

    /// SAML runs in the browser: the tray owns that phase, not systemd.
    pub fn set_authenticating(&self, authenticating: bool) {
        let _ = self.commands.send(Command::Authenticating(authenticating));
    }

    pub fn resync(&self) {
        let _ = self.commands.send(Command::Resync);
    }
}

enum EngineEvent {
    UnitState { name: String, active_state: String },
    UnitRemoved(String),
    UnitsChanged,
    LinkUp(bool),
    LinkUnknown,
}

enum Wake {
    Command(Option<Command>),
    Event(Result<EngineEvent>),
    Retry,
}

async fn run(
    state_tx: watch::Sender<VpnState>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut snapshot: Snapshot,
    mut session: Option<Session>,
) {
    let mut published = snapshot.state();
    loop {
        let wake = match session.as_mut() {
            Some(open) => {
                tokio::select! {
                    command = commands.recv() => Wake::Command(command),
                    event = open.next_event() => Wake::Event(event),
                }
            }
            None => {
                tokio::select! {
                    command = commands.recv() => Wake::Command(command),
                    () = tokio::time::sleep(RECONNECT_DELAY) => Wake::Retry,
                }
            }
        };

        let healthy = match wake {
            Wake::Command(None) => break,
            Wake::Command(Some(command)) => {
                apply_command(command, &mut snapshot, session.as_mut()).await
            }
            Wake::Event(Ok(event)) => apply_event(event, &mut snapshot, session.as_mut()).await,
            Wake::Event(Err(_)) => false,
            Wake::Retry => {
                session = open_session(&mut snapshot).await;
                true
            }
        };
        if !healthy {
            session = None;
        }
        let state = snapshot.state();
        if state != published {
            published = state;
            state_tx.send_replace(state);
        }
    }
}

async fn apply_command(
    command: Command,
    snapshot: &mut Snapshot,
    session: Option<&mut Session>,
) -> bool {
    match command {
        Command::Authenticating(authenticating) => {
            snapshot.authenticating = authenticating;
            true
        }
        Command::Resync => match session {
            Some(session) => session.resync(snapshot).await.is_ok(),
            None => true,
        },
    }
}

async fn apply_event(
    event: EngineEvent,
    snapshot: &mut Snapshot,
    session: Option<&mut Session>,
) -> bool {
    match event {
        EngineEvent::UnitState { name, active_state } => {
            snapshot.units.insert(name, active_state);
            true
        }
        EngineEvent::UnitRemoved(name) => {
            snapshot.units.remove(&name);
            true
        }
        EngineEvent::UnitsChanged => match session {
            Some(session) => session.resync_units(snapshot).await.is_ok(),
            None => true,
        },
        EngineEvent::LinkUp(up) => {
            snapshot.link_up = up;
            true
        }
        EngineEvent::LinkUnknown => match session {
            Some(session) => match session.links.is_up().await {
                Ok(up) => {
                    snapshot.link_up = up;
                    true
                }
                Err(_) => false,
            },
            None => true,
        },
    }
}

async fn open_session(snapshot: &mut Snapshot) -> Option<Session> {
    let mut session = Session::connect().await.ok()?;
    session.resync(snapshot).await.ok()?;
    Some(session)
}

struct Session {
    connection: Connection,
    messages: MessageStream,
    links: LinkMonitor,
    unit_paths: HashMap<String, String>,
}

impl Session {
    async fn connect() -> Result<Self> {
        let connection = systemd::connect().await?;
        systemd::subscribe(&connection).await?;
        Ok(Self {
            messages: MessageStream::from(&connection),
            links: LinkMonitor::new()?,
            unit_paths: HashMap::new(),
            connection,
        })
    }

    async fn resync(&mut self, snapshot: &mut Snapshot) -> Result<()> {
        self.resync_units(snapshot).await?;
        snapshot.link_up = self.links.is_up().await?;
        Ok(())
    }

    async fn resync_units(&mut self, snapshot: &mut Snapshot) -> Result<()> {
        let listings = systemd::list_units(&self.connection).await?;
        let mut units = BTreeMap::new();
        self.unit_paths.clear();
        for listing in listings
            .into_iter()
            .filter(|unit| systemd::is_tunnel_unit(&unit.name))
        {
            self.unit_paths
                .insert(listing.path.clone(), listing.name.clone());
            units.insert(listing.name, listing.active_state);
        }
        snapshot.units = units;
        Ok(())
    }

    async fn next_event(&mut self) -> Result<EngineEvent> {
        loop {
            tokio::select! {
                message = self.messages.next() => {
                    let Some(message) = message else { bail!("D-Bus connection closed") };
                    if let Some(event) = self.translate(message?).await? {
                        return Ok(event);
                    }
                }
                link = self.links.next_event() => return link,
            }
        }
    }

    async fn translate(&mut self, message: Message) -> Result<Option<EngineEvent>> {
        let Some(signal) = parse_unit_signal(&message)? else {
            return Ok(None);
        };
        match signal {
            UnitSignal::Created { name, path } => {
                if !systemd::is_tunnel_unit(&name) {
                    return Ok(None);
                }
                self.unit_paths.insert(path.clone(), name.clone());
                match systemd::unit_active_state(&self.connection, &path).await {
                    Ok(active_state) => Ok(Some(EngineEvent::UnitState { name, active_state })),
                    Err(_) => Ok(Some(EngineEvent::UnitsChanged)),
                }
            }
            UnitSignal::Removed { name, path } => {
                self.unit_paths.remove(&path);
                Ok(systemd::is_tunnel_unit(&name).then_some(EngineEvent::UnitRemoved(name)))
            }
            UnitSignal::Properties { path, active_state } => {
                let Some(name) = self.unit_paths.get(&path).cloned() else {
                    return Ok(Some(EngineEvent::UnitsChanged));
                };
                match active_state {
                    ActiveStateProperty::Value(active_state) => {
                        Ok(Some(EngineEvent::UnitState { name, active_state }))
                    }
                    ActiveStateProperty::Invalidated => Ok(Some(EngineEvent::UnitsChanged)),
                    ActiveStateProperty::Unrelated => Ok(None),
                }
            }
        }
    }
}

enum UnitSignal {
    Created {
        name: String,
        path: String,
    },
    Removed {
        name: String,
        path: String,
    },
    Properties {
        path: String,
        active_state: ActiveStateProperty,
    },
}

enum ActiveStateProperty {
    Value(String),
    Invalidated,
    Unrelated,
}

fn parse_unit_signal(message: &Message) -> Result<Option<UnitSignal>> {
    if message.message_type() != MessageType::Signal {
        return Ok(None);
    }
    let header = message.header();
    let (Some(interface), Some(member)) = (header.interface(), header.member()) else {
        return Ok(None);
    };
    match (interface.as_str(), member.as_str()) {
        (systemd::MANAGER_SIGNAL_INTERFACE, "UnitNew") => {
            let (name, path): (String, OwnedObjectPath) = message.body().deserialize()?;
            Ok(Some(UnitSignal::Created {
                name,
                path: path.as_str().to_owned(),
            }))
        }
        (systemd::MANAGER_SIGNAL_INTERFACE, "UnitRemoved") => {
            let (name, path): (String, OwnedObjectPath) = message.body().deserialize()?;
            Ok(Some(UnitSignal::Removed {
                name,
                path: path.as_str().to_owned(),
            }))
        }
        (systemd::UNIT_INTERFACE, "PropertiesChanged") => {
            let (changed_interface, changed, invalidated): (
                String,
                HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message.body().deserialize()?;
            if changed_interface != systemd::UNIT_INTERFACE {
                return Ok(None);
            }
            let Some(path) = header.path() else {
                return Ok(None);
            };
            let active_state = match changed.get("ActiveState").and_then(owned_string) {
                Some(active_state) => ActiveStateProperty::Value(active_state),
                None if invalidated.iter().any(|property| property == "ActiveState") => {
                    ActiveStateProperty::Invalidated
                }
                None => ActiveStateProperty::Unrelated,
            };
            Ok(Some(UnitSignal::Properties {
                path: path.as_str().to_owned(),
                active_state,
            }))
        }
        _ => Ok(None),
    }
}

struct LinkMonitor {
    handle: Handle,
    events: mpsc::UnboundedReceiver<NetlinkMessage<RouteNetlinkMessage>>,
    task: JoinHandle<()>,
}

impl LinkMonitor {
    fn new() -> Result<Self> {
        let (connection, handle, mut netlink_events) =
            rtnetlink::new_multicast_connection(&[MulticastGroup::Link])?;
        let (sender, events) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let connection = tokio::spawn(async move {
                let _ = connection.await;
            });
            while let Some((message, _address)) = netlink_events.next().await {
                if sender.send(message).is_err() {
                    break;
                }
            }
            connection.abort();
        });
        Ok(Self {
            handle,
            events,
            task,
        })
    }

    async fn is_up(&mut self) -> Result<bool> {
        let mut links = self.handle.link().get().execute();
        while let Some(link) = links.next().await {
            let link = link?;
            if link_name(&link) == Some(TUN_IFNAME) {
                return Ok(link_is_up(&link));
            }
        }
        Ok(false)
    }

    async fn next_event(&mut self) -> Result<EngineEvent> {
        loop {
            let Some(message) = self.events.recv().await else {
                bail!("netlink socket closed");
            };
            let event = match message.payload {
                NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewLink(link)) => {
                    link_event(&link, link_is_up(&link))
                }
                NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelLink(link)) => {
                    link_event(&link, false)
                }
                _ => continue,
            };
            match event {
                LinkEvent::Ignore => continue,
                LinkEvent::Unknown => return Ok(EngineEvent::LinkUnknown),
                LinkEvent::Changed(up) => return Ok(EngineEvent::LinkUp(up)),
            }
        }
    }
}

impl Drop for LinkMonitor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

enum LinkEvent {
    Ignore,
    Unknown,
    Changed(bool),
}

fn link_event(link: &LinkMessage, up: bool) -> LinkEvent {
    match link_name(link) {
        Some(TUN_IFNAME) => LinkEvent::Changed(up),
        Some(_) => LinkEvent::Ignore,
        None => LinkEvent::Unknown,
    }
}

fn link_name(link: &LinkMessage) -> Option<&str> {
    link.attributes
        .iter()
        .find_map(|attribute| match attribute {
            LinkAttribute::IfName(name) => Some(name.as_str()),
            _ => None,
        })
}

fn link_is_up(link: &LinkMessage) -> bool {
    link.header.flags.contains(LinkFlags::Up)
}

fn owned_string(value: &OwnedValue) -> Option<String> {
    match &**value {
        Value::Str(text) => Some(text.as_str().to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(units: &[(&str, &str)], link_up: bool) -> Snapshot {
        Snapshot {
            units: units
                .iter()
                .map(|(name, state)| ((*name).to_owned(), (*state).to_owned()))
                .collect(),
            link_up,
            authenticating: false,
        }
    }

    const UNIT: &str = "gp-tray-tunnel@portal.example.com.service";

    #[test]
    fn derives_every_state_from_units_and_the_link() {
        let cases = [
            (snapshot(&[], false), VpnState::Disconnected),
            (snapshot(&[], true), VpnState::Connected),
            (
                snapshot(&[(UNIT, "activating")], false),
                VpnState::TunnelStarting,
            ),
            (
                snapshot(&[(UNIT, "active")], false),
                VpnState::TunnelStarting,
            ),
            (
                snapshot(&[(UNIT, "reloading")], false),
                VpnState::TunnelStarting,
            ),
            (snapshot(&[(UNIT, "active")], true), VpnState::Connected),
            (
                snapshot(&[(UNIT, "deactivating")], false),
                VpnState::Disconnected,
            ),
            (snapshot(&[(UNIT, "failed")], false), VpnState::Disconnected),
            (snapshot(&[(UNIT, "inactive")], true), VpnState::Connected),
        ];
        for (snapshot, expected) in cases {
            assert_eq!(snapshot.state(), expected, "{snapshot:?}");
        }
    }

    #[test]
    fn any_tunnel_unit_keeps_the_tunnel_starting() {
        let snapshot = snapshot(
            &[
                ("gp-tray-tunnel@a.example.com.service", "inactive"),
                ("gp-tray-tunnel@b.example.com.service", "activating"),
            ],
            false,
        );
        assert_eq!(snapshot.state(), VpnState::TunnelStarting);
    }

    #[test]
    fn authenticating_wins_over_tunnel_state() {
        let mut snapshot = snapshot(&[(UNIT, "active")], true);
        snapshot.authenticating = true;
        assert_eq!(snapshot.state(), VpnState::Authenticating);
    }

    #[test]
    fn states_have_a_stable_label() {
        assert_eq!(VpnState::Disconnected.as_str(), "disconnected");
        assert_eq!(VpnState::Authenticating.as_str(), "authenticating");
        assert_eq!(VpnState::TunnelStarting.as_str(), "tunnel starting");
        assert_eq!(VpnState::Connected.as_str(), "connected");
        assert_eq!(VpnState::Connected.to_string(), "Connected");
    }

    fn unit_created(name: &str, path: &str) -> Message {
        Message::signal(
            "/org/freedesktop/systemd1",
            systemd::MANAGER_SIGNAL_INTERFACE,
            "UnitNew",
        )
        .unwrap()
        .build(&(name, OwnedObjectPath::try_from(path).unwrap()))
        .unwrap()
    }

    fn properties_changed(
        unit_path: &str,
        changed_interface: &str,
        changed: HashMap<String, OwnedValue>,
        invalidated: Vec<String>,
    ) -> Message {
        Message::signal(unit_path, systemd::UNIT_INTERFACE, "PropertiesChanged")
            .unwrap()
            .build(&(changed_interface, changed, invalidated))
            .unwrap()
    }

    const UNIT_PATH: &str =
        "/org/freedesktop/systemd1/unit/gp_2dtray_2dtunnel_40portal_2eexample_2ecom_2eservice";

    #[test]
    fn parses_unit_new_and_unit_removed() {
        let created = unit_created("gp-tray-tunnel@portal.example.com.service", UNIT_PATH);
        let parsed = parse_unit_signal(&created).unwrap().unwrap();
        assert!(matches!(parsed, UnitSignal::Created { .. }));

        let removed = Message::signal(
            "/org/freedesktop/systemd1",
            systemd::MANAGER_SIGNAL_INTERFACE,
            "UnitRemoved",
        )
        .unwrap()
        .build(&(
            "gp-tray-tunnel@portal.example.com.service",
            OwnedObjectPath::try_from(UNIT_PATH).unwrap(),
        ))
        .unwrap();
        let parsed = parse_unit_signal(&removed).unwrap().unwrap();
        assert!(matches!(parsed, UnitSignal::Removed { .. }));
    }

    #[test]
    fn parses_active_state_changes() {
        let mut changed = HashMap::new();
        changed.insert(
            "ActiveState".to_owned(),
            Value::from("activating").try_to_owned().unwrap(),
        );
        let message = properties_changed(UNIT_PATH, systemd::UNIT_INTERFACE, changed, vec![]);
        let parsed = parse_unit_signal(&message).unwrap().unwrap();
        let UnitSignal::Properties { path, active_state } = parsed else {
            panic!("expected unit properties");
        };
        assert_eq!(path, UNIT_PATH);
        assert!(matches!(
            active_state,
            ActiveStateProperty::Value(state) if state == "activating"
        ));
    }

    #[test]
    fn separates_invalidated_and_unrelated_property_changes() {
        let invalidated = properties_changed(
            UNIT_PATH,
            systemd::UNIT_INTERFACE,
            HashMap::new(),
            vec!["ActiveState".to_owned()],
        );
        let parsed = parse_unit_signal(&invalidated).unwrap().unwrap();
        assert!(matches!(
            parsed,
            UnitSignal::Properties {
                active_state: ActiveStateProperty::Invalidated,
                ..
            }
        ));

        let unrelated = properties_changed(
            UNIT_PATH,
            systemd::UNIT_INTERFACE,
            HashMap::new(),
            vec!["MemoryCurrent".to_owned()],
        );
        let parsed = parse_unit_signal(&unrelated).unwrap().unwrap();
        assert!(matches!(
            parsed,
            UnitSignal::Properties {
                active_state: ActiveStateProperty::Unrelated,
                ..
            }
        ));
    }

    #[test]
    fn ignores_other_interfaces_and_non_signals() {
        let service_properties = properties_changed(
            UNIT_PATH,
            "org.freedesktop.systemd1.Service",
            HashMap::new(),
            vec![],
        );
        assert!(parse_unit_signal(&service_properties).unwrap().is_none());

        let call = Message::method_call("/org/freedesktop/systemd1", "Reload")
            .unwrap()
            .build(&())
            .unwrap();
        assert!(parse_unit_signal(&call).unwrap().is_none());
    }

    #[test]
    fn classifies_link_messages() {
        let mut gp0_up = LinkMessage::default();
        gp0_up
            .attributes
            .push(LinkAttribute::IfName(TUN_IFNAME.to_owned()));
        gp0_up.header.flags.insert(LinkFlags::Up);
        assert!(matches!(
            link_event(&gp0_up, link_is_up(&gp0_up)),
            LinkEvent::Changed(true)
        ));

        let mut gp0_down = LinkMessage::default();
        gp0_down
            .attributes
            .push(LinkAttribute::IfName(TUN_IFNAME.to_owned()));
        assert!(matches!(
            link_event(&gp0_down, link_is_up(&gp0_down)),
            LinkEvent::Changed(false)
        ));

        let mut other = LinkMessage::default();
        other
            .attributes
            .push(LinkAttribute::IfName("tailscale0".to_owned()));
        other.header.flags.insert(LinkFlags::Up);
        assert!(matches!(
            link_event(&other, link_is_up(&other)),
            LinkEvent::Ignore
        ));

        assert!(matches!(
            link_event(&LinkMessage::default(), false),
            LinkEvent::Unknown
        ));
    }
}
