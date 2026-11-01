use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

pub const CONFIG_TEMPLATE: &str = "\
# gp-tray portals — one per line:  Friendly Name = portal.hostname
# Optionally declare gateways after '|' (comma-separated); they appear as a
# submenu so you can pick one instead of the automatic fastest gateway:
#   Friendly Name = portal.hostname | Gateway One, Gateway Two
# Lines starting with '#' are ignored.
#
#Work VPN      = portal.example.com | US-East, EU-Frankfurt
#Secondary VPN = vpn2.example.com
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Portal {
    pub name: String,
    pub host: String,
    pub gateways: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalConfig {
    portals: Vec<Portal>,
    names: HashMap<String, String>,
}

impl PortalConfig {
    pub fn new(portals: Vec<Portal>) -> Self {
        let names = portals
            .iter()
            .map(|portal| (portal.host.clone(), portal.name.clone()))
            .collect();
        Self { portals, names }
    }

    pub fn portals(&self) -> &[Portal] {
        &self.portals
    }

    pub fn friendly(&self, host: &str) -> String {
        if host.is_empty() {
            return String::new();
        }
        self.names
            .get(host)
            .cloned()
            .unwrap_or_else(|| host.to_owned())
    }

    pub fn with_env_override(self, gp_portal: Option<&str>) -> Self {
        let Some(host) = gp_portal.map(str::trim).filter(|host| !host.is_empty()) else {
            return self;
        };
        if self.portals.iter().any(|portal| portal.host == host) {
            return self;
        }
        let mut portals = self.portals;
        portals.insert(
            0,
            Portal {
                name: host.to_owned(),
                host: host.to_owned(),
                gateways: Vec::new(),
            },
        );
        Self::new(portals)
    }
}

pub fn ensure_config(config_dir: &Path, config_file: &Path) -> io::Result<()> {
    if config_file.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(config_dir)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(config_file)?;
    file.write_all(CONFIG_TEMPLATE.as_bytes())
}

pub fn parse(text: &str) -> Vec<Portal> {
    let mut portals = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, rest)) = line.split_once('=') else {
            continue;
        };
        let (host_part, gateway_part) = rest.split_once('|').unwrap_or((rest, ""));
        let host = host_part.trim();
        if host.is_empty() {
            continue;
        }
        let name = name.trim();
        portals.push(Portal {
            name: if name.is_empty() {
                host.to_owned()
            } else {
                name.to_owned()
            },
            host: host.to_owned(),
            gateways: gateway_part
                .split(',')
                .map(str::trim)
                .filter(|gateway| !gateway.is_empty())
                .map(str::to_owned)
                .collect(),
        });
    }
    portals
}

pub fn load(config_file: &Path) -> PortalConfig {
    let portals = std::fs::read_to_string(config_file)
        .map(|text| parse(&text))
        .unwrap_or_default();
    PortalConfig::new(if portals.is_empty() {
        example_placeholders()
    } else {
        portals
    })
}

fn example_placeholders() -> Vec<Portal> {
    [
        ("Work VPN", "portal.example.com"),
        ("Secondary VPN", "vpn2.example.com"),
    ]
    .into_iter()
    .map(|(name, host)| Portal {
        name: name.to_owned(),
        host: host.to_owned(),
        gateways: Vec::new(),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn portal(name: &str, host: &str, gateways: &[&str]) -> Portal {
        Portal {
            name: name.to_owned(),
            host: host.to_owned(),
            gateways: gateways.iter().map(|gw| (*gw).to_owned()).collect(),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gp-tray-config-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_names_hosts_and_gateways() {
        let text = "\
# comment
Work VPN      = portal.example.com | US-East, EU-Frankfurt
Secondary VPN = vpn2.example.com
No Submenu=vpn3.example.com|
";
        assert_eq!(
            parse(text),
            vec![
                portal(
                    "Work VPN",
                    "portal.example.com",
                    &["US-East", "EU-Frankfurt"]
                ),
                portal("Secondary VPN", "vpn2.example.com", &[]),
                portal("No Submenu", "vpn3.example.com", &[]),
            ]
        );
    }

    #[test]
    fn skips_lines_without_equals_or_host() {
        let text = "\
just some text
Empty Host =    | GW
  = orphan.example.com
   # indented comment = ignored
= empty.example.com
Only Gateways = | A, B
";
        assert_eq!(
            parse(text),
            vec![
                portal("orphan.example.com", "orphan.example.com", &[]),
                portal("empty.example.com", "empty.example.com", &[]),
            ]
        );
    }

    #[test]
    fn name_falls_back_to_host_and_gateways_are_trimmed() {
        let text = "  = portal.example.com |  A ,, B , \n";
        assert_eq!(
            parse(text),
            vec![portal(
                "portal.example.com",
                "portal.example.com",
                &["A", "B"]
            )]
        );
    }

    #[test]
    fn extra_equals_signs_stay_in_the_host() {
        assert_eq!(
            parse("Weird = host=a=b\n"),
            vec![portal("Weird", "host=a=b", &[])]
        );
    }

    #[test]
    fn empty_config_keeps_placeholders_and_ignored_lines_too() {
        let dir = temp_dir("placeholders");
        for body in ["", "# only comments\n", "not a portal line\n"] {
            let file = dir.join("portals.conf");
            std::fs::write(&file, body).unwrap();
            let config = load(&file);
            assert_eq!(
                config.portals(),
                &[
                    portal("Work VPN", "portal.example.com", &[]),
                    portal("Secondary VPN", "vpn2.example.com", &[]),
                ]
            );
        }
    }

    #[test]
    fn missing_config_keeps_placeholders() {
        let dir = temp_dir("missing");
        let config = load(&dir.join("absent.conf"));
        assert_eq!(config.portals().len(), 2);
    }

    #[test]
    fn gp_portal_override_is_prepended_once() {
        let config = PortalConfig::new(vec![portal("Work", "vpn.example.com", &[])])
            .with_env_override(Some("extra.example.com"));
        assert_eq!(
            config.portals(),
            &[
                portal("extra.example.com", "extra.example.com", &[]),
                portal("Work", "vpn.example.com", &[]),
            ]
        );

        let config = PortalConfig::new(vec![portal("Work", "vpn.example.com", &[])])
            .with_env_override(Some("vpn.example.com"));
        assert_eq!(config.portals().len(), 1);

        let config =
            PortalConfig::new(vec![portal("Work", "vpn.example.com", &[])]).with_env_override(None);
        assert_eq!(config.portals().len(), 1);
    }

    #[test]
    fn friendly_names_fall_back_to_the_host() {
        let config = PortalConfig::new(vec![
            portal("Work VPN", "portal.example.com", &[]),
            portal("Dup", "portal.example.com", &[]),
        ]);
        assert_eq!(config.friendly(""), "");
        assert_eq!(config.friendly("portal.example.com"), "Dup");
        assert_eq!(config.friendly("other.example.com"), "other.example.com");
    }

    #[test]
    fn ensure_config_creates_template_only_once() {
        let dir = temp_dir("ensure");
        let file = dir.join("nested").join("portals.conf");
        ensure_config(&dir.join("nested"), &file).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), CONFIG_TEMPLATE);

        std::fs::write(&file, "Mine = vpn.example.com\n").unwrap();
        ensure_config(&dir.join("nested"), &file).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Mine = vpn.example.com\n"
        );
    }
}
