use std::io;
use std::path::Path;

use crate::paths::Paths;

fn read_trimmed(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map(|text| text.trim().to_owned())
        .unwrap_or_default()
}

fn write_trimmed(path: &Path, value: &str) -> io::Result<()> {
    std::fs::write(path, value)
}

pub fn read_active(paths: &Paths) -> String {
    read_trimmed(&paths.active_portal)
}

pub fn write_active(paths: &Paths, portal: &str) -> io::Result<()> {
    write_trimmed(&paths.active_portal, portal)
}

pub fn read_active_gateway(paths: &Paths) -> String {
    read_trimmed(&paths.active_gateway)
}

pub fn write_active_gateway(paths: &Paths, gateway: Option<&str>) -> io::Result<()> {
    write_trimmed(&paths.active_gateway, gateway.unwrap_or(""))
}

pub fn clear_active(paths: &Paths) {
    for path in [&paths.active_portal, &paths.active_gateway] {
        let _ = std::fs::remove_file(path);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LastGateways(Vec<(String, String)>);

impl LastGateways {
    pub fn parse(text: &str) -> Self {
        let mut last = Self::default();
        for line in text.lines() {
            let Some((host, gateway)) = line.split_once('=') else {
                continue;
            };
            last.set(host.trim(), gateway.trim());
        }
        last
    }

    pub fn get(&self, host: &str) -> &str {
        self.0
            .iter()
            .find(|(known, _)| known == host)
            .map(|(_, gateway)| gateway.as_str())
            .unwrap_or("")
    }

    pub fn set(&mut self, host: &str, gateway: &str) {
        match self.0.iter_mut().find(|(known, _)| known == host) {
            Some(entry) => entry.1 = gateway.to_owned(),
            None => self.0.push((host.to_owned(), gateway.to_owned())),
        }
    }

    pub fn render(&self) -> String {
        self.0
            .iter()
            .map(|(host, gateway)| format!("{host} = {gateway}\n"))
            .collect()
    }
}

pub fn read_last_gateways(paths: &Paths) -> LastGateways {
    std::fs::read_to_string(&paths.last_gateways)
        .map(|text| LastGateways::parse(&text))
        .unwrap_or_default()
}

pub fn write_last_gateway(paths: &Paths, portal: &str, gateway: Option<&str>) -> io::Result<()> {
    let mut last = read_last_gateways(paths);
    last.set(portal, gateway.unwrap_or(""));
    std::fs::write(&paths.last_gateways, last.render())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn paths(name: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("gp-tray-state-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = Paths::resolve(Some(PathBuf::from("/home/alex")), |key| match key {
            "XDG_CONFIG_HOME" => Some(dir.join("config")),
            "XDG_CACHE_HOME" => Some(dir.join("cache")),
            _ => None,
        });
        std::fs::create_dir_all(&paths.cache_dir).unwrap();
        paths
    }

    #[test]
    fn active_portal_round_trips_without_newline() {
        let paths = paths("active");
        assert_eq!(read_active(&paths), "");
        write_active(&paths, "portal.example.com").unwrap();
        assert_eq!(
            std::fs::read_to_string(&paths.active_portal).unwrap(),
            "portal.example.com"
        );
        assert_eq!(read_active(&paths), "portal.example.com");
        std::fs::write(&paths.active_portal, "  portal.example.com\n").unwrap();
        assert_eq!(read_active(&paths), "portal.example.com");
    }

    #[test]
    fn active_gateway_written_as_empty_string_for_auto() {
        let paths = paths("gateway");
        write_active_gateway(&paths, Some("EU-Frankfurt")).unwrap();
        assert_eq!(read_active_gateway(&paths), "EU-Frankfurt");
        write_active_gateway(&paths, None).unwrap();
        assert_eq!(std::fs::read_to_string(&paths.active_gateway).unwrap(), "");
        assert_eq!(read_active_gateway(&paths), "");
    }

    #[test]
    fn clear_active_removes_portal_and_gateway() {
        let paths = paths("clear");
        write_active(&paths, "portal.example.com").unwrap();
        write_active_gateway(&paths, Some("GW")).unwrap();
        clear_active(&paths);
        assert!(!paths.active_portal.exists());
        assert!(!paths.active_gateway.exists());
        clear_active(&paths);
    }

    #[test]
    fn last_gateways_parse_and_render_in_order() {
        let parsed = LastGateways::parse(
            "a.example.com = GW1\n\nb.example.com = \nbroken line\na.example.com = GW2\n",
        );
        assert_eq!(parsed.get("a.example.com"), "GW2");
        assert_eq!(parsed.get("b.example.com"), "");
        assert_eq!(parsed.get("absent.example.com"), "");
        assert_eq!(parsed.render(), "a.example.com = GW2\nb.example.com = \n");
    }

    #[test]
    fn last_gateway_write_keeps_existing_order_and_appends_new() {
        let paths = paths("last");
        write_last_gateway(&paths, "a.example.com", Some("GW1")).unwrap();
        write_last_gateway(&paths, "b.example.com", None).unwrap();
        write_last_gateway(&paths, "a.example.com", Some("GW2")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&paths.last_gateways).unwrap(),
            "a.example.com = GW2\nb.example.com = \n"
        );
        assert_eq!(read_last_gateways(&paths).get("a.example.com"), "GW2");
    }

    #[test]
    fn missing_files_read_as_empty() {
        let paths = paths("missing");
        assert_eq!(read_active(&paths), "");
        assert_eq!(read_active_gateway(&paths), "");
        assert_eq!(read_last_gateways(&paths), LastGateways::default());
    }
}
