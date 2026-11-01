use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub cache_dir: PathBuf,
    pub log_file: PathBuf,
    pub active_portal: PathBuf,
    pub active_gateway: PathBuf,
    pub last_gateways: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        Self::resolve(std::env::var_os("HOME").map(PathBuf::from), |key| {
            std::env::var_os(key).map(PathBuf::from)
        })
    }

    pub(crate) fn resolve(home: Option<PathBuf>, var: impl Fn(&str) -> Option<PathBuf>) -> Self {
        let home = home.unwrap_or_else(|| PathBuf::from("."));
        let base = |key: &str, fallback: &str| {
            var(key)
                .filter(|value| !value.as_os_str().is_empty())
                .unwrap_or_else(|| home.join(fallback))
        };

        let config_dir = base("XDG_CONFIG_HOME", ".config").join("gp-tray");
        let cache_dir = base("XDG_CACHE_HOME", ".cache").join("gp-tray");

        Self {
            config_file: config_dir.join("portals.conf"),
            log_file: cache_dir.join("connect.log"),
            active_portal: cache_dir.join("active_portal"),
            active_gateway: cache_dir.join("active_gateway"),
            last_gateways: cache_dir.join("last_gateways"),
            config_dir,
            cache_dir,
        }
    }

    pub fn icon_fallback_dir(exe: &Path) -> PathBuf {
        let real = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
        real.parent()
            .map(|parent| parent.join("icons"))
            .unwrap_or_else(|| PathBuf::from("icons"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve_with(vars: &[(&str, &str)]) -> Paths {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        Paths::resolve(Some(PathBuf::from("/home/alex")), |key| {
            vars.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| PathBuf::from(value))
        })
    }

    #[test]
    fn xdg_variables_win_over_home() {
        let paths = resolve_with(&[("XDG_CONFIG_HOME", "/cfg"), ("XDG_CACHE_HOME", "/cache")]);
        assert_eq!(
            paths.config_file,
            PathBuf::from("/cfg/gp-tray/portals.conf")
        );
        assert_eq!(paths.log_file, PathBuf::from("/cache/gp-tray/connect.log"));
        assert_eq!(
            paths.active_portal,
            PathBuf::from("/cache/gp-tray/active_portal")
        );
        assert_eq!(
            paths.active_gateway,
            PathBuf::from("/cache/gp-tray/active_gateway")
        );
        assert_eq!(
            paths.last_gateways,
            PathBuf::from("/cache/gp-tray/last_gateways")
        );
    }

    #[test]
    fn unset_or_empty_variables_fall_back_to_home() {
        for vars in [
            vec![],
            vec![("XDG_CONFIG_HOME", ""), ("XDG_CACHE_HOME", "")],
        ] {
            let paths = resolve_with(&vars);
            assert_eq!(
                paths.config_file,
                PathBuf::from("/home/alex/.config/gp-tray/portals.conf")
            );
            assert_eq!(
                paths.log_file,
                PathBuf::from("/home/alex/.cache/gp-tray/connect.log")
            );
        }
    }

    #[test]
    fn icon_fallback_dir_sits_next_to_the_binary() {
        let dir = std::env::temp_dir().join("gp-tray-icon-fallback-test");
        std::fs::create_dir_all(dir.join("icons")).unwrap();
        let exe = dir.join("gp-tray-bin");
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(Paths::icon_fallback_dir(&exe), dir.join("icons"));
    }
}
