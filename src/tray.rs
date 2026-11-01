use std::path::Path;

use ksni::menu::{MenuItem, StandardItem, SubMenu};
use ksni::{Category, Tray};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::config::PortalConfig;
use crate::engine::VpnState;
use crate::paths::Paths;
use crate::state_files::{self, LastGateways};

const ICON_CONNECTED: &str = "gp-tray-connected";
const ICON_CONNECTING: &str = "gp-tray-connecting";
const ICON_DISCONNECTED: &str = "gp-tray-disconnected";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayCommand {
    Connect {
        portal: String,
        gateway: Option<String>,
    },
    Disconnect,
    OpenLog,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Connected,
    Connecting,
    Disconnected,
}

/// The three looks the tray has, derived from the four engine states.
#[derive(Debug, Clone)]
pub struct View {
    pub state: VpnState,
    pub active_portal: String,
    pub active_gateway: String,
    pub remembered_gateways: LastGateways,
    pub config: PortalConfig,
}

impl View {
    pub fn read(state: VpnState, config: PortalConfig, paths: &Paths) -> Self {
        Self {
            state,
            active_portal: state_files::read_active(paths),
            active_gateway: state_files::read_active_gateway(paths),
            remembered_gateways: state_files::read_last_gateways(paths),
            config,
        }
    }

    fn phase(&self) -> Phase {
        match self.state {
            VpnState::Connected => Phase::Connected,
            VpnState::Disconnected => Phase::Disconnected,
            VpnState::Authenticating | VpnState::TunnelStarting => Phase::Connecting,
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self.phase() {
            Phase::Connected => ICON_CONNECTED,
            Phase::Connecting => ICON_CONNECTING,
            Phase::Disconnected => ICON_DISCONNECTED,
        }
    }

    fn active_name(&self) -> String {
        self.config.friendly(&self.active_portal)
    }

    fn gateway_suffix(&self) -> String {
        if self.active_gateway.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.active_gateway)
        }
    }

    pub fn title(&self) -> String {
        match self.phase() {
            Phase::Connected => format!(
                "GlobalProtect — Connected {}{}",
                self.active_name(),
                self.gateway_suffix()
            )
            .trim()
            .to_owned(),
            Phase::Connecting => "GlobalProtect — Connecting".to_owned(),
            Phase::Disconnected => "GlobalProtect — Disconnected".to_owned(),
        }
    }

    pub fn status_label(&self) -> String {
        let name = self.active_name();
        let fallback = |default: &str| {
            if name.is_empty() {
                default.to_owned()
            } else {
                name.clone()
            }
        };
        match self.phase() {
            Phase::Connected => format!(
                "●  Connected:  {}{}",
                fallback("GlobalProtect"),
                self.gateway_suffix()
            ),
            Phase::Connecting => {
                format!("…  Connecting:  {}{}", fallback("…"), self.gateway_suffix())
            }
            Phase::Disconnected => "○  Disconnected".to_owned(),
        }
    }

    fn on_this_portal(&self, host: &str) -> bool {
        self.phase() != Phase::Disconnected && host == self.active_portal
    }

    pub fn portal_label(&self, host: &str, name: &str) -> String {
        if self.on_this_portal(host) {
            let verb = match self.phase() {
                Phase::Connected => "✓ Connected",
                _ => "…  Connecting",
            };
            format!("{verb} — {name}")
        } else if self.phase() != Phase::Disconnected {
            format!("⇄ Switch to — {name}")
        } else {
            format!("Connect — {name}")
        }
    }

    pub fn portal_enabled(&self, host: &str, has_gateways: bool) -> bool {
        !self.on_this_portal(host) || has_gateways
    }

    fn gateway_mark(&self, host: &str) -> &str {
        if self.on_this_portal(host) {
            &self.active_gateway
        } else {
            self.remembered_gateways.get(host)
        }
    }

    pub fn gateway_label(&self, host: &str, gateway: Option<&str>) -> String {
        let base = gateway.unwrap_or("Auto (fastest)");
        if gateway.unwrap_or("") == self.gateway_mark(host) {
            format!("✓ {base}")
        } else {
            base.to_owned()
        }
    }

    pub fn gateway_enabled(&self, host: &str, gateway: Option<&str>) -> bool {
        !(self.on_this_portal(host) && gateway.unwrap_or("") == self.gateway_mark(host))
    }

    pub fn disconnect_enabled(&self) -> bool {
        self.phase() != Phase::Disconnected
    }
}

pub struct TrayApp {
    state: VpnState,
    config: PortalConfig,
    paths: Paths,
    icon_theme_path: String,
    commands: UnboundedSender<TrayCommand>,
}

impl TrayApp {
    pub fn new(
        state: VpnState,
        config: PortalConfig,
        paths: Paths,
        icon_theme_path: String,
    ) -> (Self, UnboundedReceiver<TrayCommand>) {
        let (commands, receiver) = unbounded_channel();
        (
            Self {
                state,
                config,
                paths,
                icon_theme_path,
                commands,
            },
            receiver,
        )
    }

    pub fn set_state(&mut self, state: VpnState) {
        self.state = state;
    }

    fn view(&self) -> View {
        View::read(self.state, self.config.clone(), &self.paths)
    }
}

pub fn icon_theme_path(exe: &Path) -> String {
    let dir = Paths::icon_fallback_dir(exe);
    if dir.is_dir() {
        dir.to_string_lossy().into_owned()
    } else {
        String::new()
    }
}

impl Tray for TrayApp {
    fn id(&self) -> String {
        "gp-tray".to_owned()
    }

    fn category(&self) -> Category {
        Category::SystemServices
    }

    fn title(&self) -> String {
        self.view().title()
    }

    fn icon_name(&self) -> String {
        self.view().icon_name().to_owned()
    }

    fn icon_theme_path(&self) -> String {
        self.icon_theme_path.clone()
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let view = self.view();
        let mut items = vec![
            MenuItem::Standard(StandardItem {
                label: view.status_label(),
                enabled: false,
                ..Default::default()
            }),
            MenuItem::Separator,
        ];

        for portal in view.config.portals() {
            let label = view.portal_label(&portal.host, &portal.name);
            let enabled = view.portal_enabled(&portal.host, !portal.gateways.is_empty());
            if portal.gateways.is_empty() {
                items.push(connect_item(
                    &self.commands,
                    label,
                    enabled,
                    &portal.host,
                    None,
                ));
                continue;
            }
            let mut submenu = vec![connect_item(
                &self.commands,
                view.gateway_label(&portal.host, None),
                view.gateway_enabled(&portal.host, None),
                &portal.host,
                None,
            )];
            for gateway in &portal.gateways {
                submenu.push(connect_item(
                    &self.commands,
                    view.gateway_label(&portal.host, Some(gateway)),
                    view.gateway_enabled(&portal.host, Some(gateway)),
                    &portal.host,
                    Some(gateway.clone()),
                ));
            }
            items.push(MenuItem::SubMenu(SubMenu {
                label,
                enabled,
                submenu,
                ..Default::default()
            }));
        }

        let commands = self.commands.clone();
        items.push(MenuItem::Standard(StandardItem {
            label: "Disconnect".to_owned(),
            enabled: view.disconnect_enabled(),
            activate: Box::new(move |_| {
                let _ = commands.send(TrayCommand::Disconnect);
            }),
            ..Default::default()
        }));
        items.push(MenuItem::Separator);

        let commands = self.commands.clone();
        items.push(MenuItem::Standard(StandardItem {
            label: "View connect log".to_owned(),
            activate: Box::new(move |_| {
                let _ = commands.send(TrayCommand::OpenLog);
            }),
            ..Default::default()
        }));
        let commands = self.commands.clone();
        items.push(MenuItem::Standard(StandardItem {
            label: "Quit tray".to_owned(),
            activate: Box::new(move |_| {
                let _ = commands.send(TrayCommand::Quit);
            }),
            ..Default::default()
        }));
        items
    }
}

fn connect_item(
    commands: &UnboundedSender<TrayCommand>,
    label: String,
    enabled: bool,
    host: &str,
    gateway: Option<String>,
) -> MenuItem<TrayApp> {
    let commands = commands.clone();
    let portal = host.to_owned();
    MenuItem::Standard(StandardItem {
        label,
        enabled,
        activate: Box::new(move |_| {
            let _ = commands.send(TrayCommand::Connect {
                portal: portal.clone(),
                gateway: gateway.clone(),
            });
        }),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Portal;

    fn portal(name: &str, host: &str, gateways: &[&str]) -> Portal {
        Portal {
            name: name.to_owned(),
            host: host.to_owned(),
            gateways: gateways.iter().map(|gw| (*gw).to_owned()).collect(),
        }
    }

    fn make_view(
        state: VpnState,
        active_portal: &str,
        active_gateway: &str,
        last: &[(&str, &str)],
    ) -> View {
        let mut remembered = LastGateways::default();
        for (host, gateway) in last {
            remembered.set(host, gateway);
        }
        View {
            state,
            active_portal: active_portal.to_owned(),
            active_gateway: active_gateway.to_owned(),
            remembered_gateways: remembered,
            config: PortalConfig::new(vec![
                portal(
                    "Work VPN",
                    "portal.example.com",
                    &["EU-Frankfurt", "US-East"],
                ),
                portal("Secondary", "vpn2.example.com", &[]),
            ]),
        }
    }

    #[test]
    fn disconnected_shows_connect_labels() {
        let view = make_view(VpnState::Disconnected, "", "", &[]);
        assert_eq!(view.title(), "GlobalProtect — Disconnected");
        assert_eq!(view.status_label(), "○  Disconnected");
        assert_eq!(view.icon_name(), ICON_DISCONNECTED);
        assert_eq!(
            view.portal_label("portal.example.com", "Work VPN"),
            "Connect — Work VPN"
        );
        assert!(view.portal_enabled("portal.example.com", true));
        assert!(!view.disconnect_enabled());
        assert_eq!(
            view.gateway_label("portal.example.com", None),
            "✓ Auto (fastest)"
        );
        assert!(view.gateway_enabled("portal.example.com", None));
    }

    #[test]
    fn connected_marks_the_portal_and_gateway() {
        let view = make_view(
            VpnState::Connected,
            "portal.example.com",
            "EU-Frankfurt",
            &[],
        );
        assert_eq!(
            view.title(),
            "GlobalProtect — Connected Work VPN (EU-Frankfurt)"
        );
        assert_eq!(
            view.status_label(),
            "●  Connected:  Work VPN (EU-Frankfurt)"
        );
        assert_eq!(view.icon_name(), ICON_CONNECTED);
        assert_eq!(
            view.portal_label("portal.example.com", "Work VPN"),
            "✓ Connected — Work VPN"
        );
        assert_eq!(
            view.portal_label("vpn2.example.com", "Secondary"),
            "⇄ Switch to — Secondary"
        );
        assert_eq!(
            view.gateway_label("portal.example.com", None),
            "Auto (fastest)"
        );
        assert_eq!(
            view.gateway_label("portal.example.com", Some("EU-Frankfurt")),
            "✓ EU-Frankfurt"
        );
        assert!(view.gateway_enabled("portal.example.com", None));
        assert!(!view.gateway_enabled("portal.example.com", Some("EU-Frankfurt")));
        assert!(view.disconnect_enabled());
    }

    #[test]
    fn connected_portal_with_gateways_stays_sensitive() {
        let view = make_view(VpnState::Connected, "portal.example.com", "", &[]);
        assert!(view.portal_enabled("portal.example.com", true));
        let view = make_view(VpnState::Disconnected, "portal.example.com", "", &[]);
        assert!(view.portal_enabled("portal.example.com", false));
    }

    #[test]
    fn connecting_uses_the_connecting_phase() {
        for state in [VpnState::Authenticating, VpnState::TunnelStarting] {
            let view = make_view(state, "portal.example.com", "", &[]);
            assert_eq!(view.title(), "GlobalProtect — Connecting");
            assert_eq!(view.icon_name(), ICON_CONNECTING);
            assert_eq!(
                view.portal_label("portal.example.com", "Work VPN"),
                "…  Connecting — Work VPN"
            );
            assert_eq!(view.status_label(), "…  Connecting:  Work VPN");
            assert!(view.disconnect_enabled());
        }
    }

    #[test]
    fn no_active_portal_falls_back_to_placeholders() {
        let view = make_view(VpnState::Connected, "", "", &[]);
        assert_eq!(view.title(), "GlobalProtect — Connected");
        assert_eq!(view.status_label(), "●  Connected:  GlobalProtect");
        let view = make_view(VpnState::Authenticating, "", "", &[]);
        assert_eq!(view.status_label(), "…  Connecting:  …");
    }

    #[test]
    fn remembered_gateway_is_marked_when_not_connected_to_that_portal() {
        let view = make_view(
            VpnState::Connected,
            "portal.example.com",
            "EU-Frankfurt",
            &[("vpn2.example.com", ""), ("portal.example.com", "US-East")],
        );
        assert_eq!(
            view.gateway_label("vpn2.example.com", None),
            "✓ Auto (fastest)"
        );
        assert!(view.gateway_enabled("vpn2.example.com", None));
    }

    #[test]
    fn gateway_marks_follow_the_active_gateway_while_connected() {
        let view = make_view(
            VpnState::Connected,
            "portal.example.com",
            "EU-Frankfurt",
            &[("portal.example.com", "US-East")],
        );
        assert_eq!(
            view.gateway_label("portal.example.com", Some("US-East")),
            "US-East"
        );
        assert!(view.gateway_enabled("portal.example.com", Some("US-East")));
    }

    #[test]
    fn empty_icon_theme_path_outside_the_repo() {
        assert_eq!(icon_theme_path(Path::new("/nonexistent/gp-tray")), "");
    }
}
