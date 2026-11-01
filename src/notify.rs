use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use notify_rust::{Notification, Urgency};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::PortalConfig;
use crate::engine::{Engine, VpnState};
use crate::paths::Paths;
use crate::state_files;
use crate::systemd;

const NOTIFICATION_ICON: &str = "network-vpn-symbolic";
const NOTIFICATION_ICON_ERROR: &str = "network-error-symbolic";
const NOTIFICATION_ICON_AUTH: &str = "dialog-password-symbolic";

const FAILURE_SETTLE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub critical: bool,
    pub icon: &'static str,
}

impl Notice {
    pub fn connected(name: &str) -> Self {
        Self {
            title: "VPN connected".to_owned(),
            body: name.to_owned(),
            critical: false,
            icon: NOTIFICATION_ICON,
        }
    }

    pub fn dropped() -> Self {
        Self {
            title: "VPN disconnected".to_owned(),
            body: "The GlobalProtect tunnel dropped.".to_owned(),
            critical: true,
            icon: NOTIFICATION_ICON_ERROR,
        }
    }

    pub fn switching(portal_name: &str, gateway: Option<&str>) -> Self {
        Self {
            title: "Switching VPN".to_owned(),
            body: format!(
                "→ {portal_name}{}",
                gateway
                    .map(|gateway| format!(" ({gateway})"))
                    .unwrap_or_default()
            ),
            critical: false,
            icon: NOTIFICATION_ICON,
        }
    }

    pub fn switch_failed() -> Self {
        Self {
            title: "VPN switch failed".to_owned(),
            body: "Previous session did not disconnect in time.".to_owned(),
            critical: true,
            icon: NOTIFICATION_ICON_ERROR,
        }
    }

    pub fn auth_timed_out() -> Self {
        Self {
            title: "VPN connect timed out".to_owned(),
            body: "Authentication did not complete in time.".to_owned(),
            critical: true,
            icon: NOTIFICATION_ICON_ERROR,
        }
    }

    pub fn tunnel_timed_out() -> Self {
        Self {
            title: "VPN connect timed out".to_owned(),
            body: "The tunnel did not come up in time; connection reset.".to_owned(),
            critical: true,
            icon: NOTIFICATION_ICON_ERROR,
        }
    }

    pub fn auth_cancelled() -> Self {
        Self {
            title: "VPN connect cancelled".to_owned(),
            body: "Authentication was dismissed or failed.".to_owned(),
            critical: true,
            icon: NOTIFICATION_ICON_AUTH,
        }
    }

    pub fn connect_failed(detail: impl Into<String>) -> Self {
        Self {
            title: "VPN connection failed".to_owned(),
            body: detail.into(),
            critical: true,
            icon: NOTIFICATION_ICON_ERROR,
        }
    }
}

/// Notification suppression state: a drop the user asked for is not news, and
/// one failure notification per attempt is enough.
#[derive(Debug, Default)]
pub struct Flags {
    pub user_disconnect: AtomicBool,
    pub notified_fail: AtomicBool,
}

/// Desktop notification plus the line the connect log keeps, in the
/// `[gp-tray] title: body` layout.
pub async fn deliver(log_file: &Path, notice: &Notice) {
    let mut notification = Notification::new();
    notification
        .appname("GlobalProtect")
        .summary(&notice.title)
        .body(&notice.body)
        .icon(notice.icon)
        .urgency(if notice.critical {
            Urgency::Critical
        } else {
            Urgency::Normal
        });
    let _ = notification.show_async().await;
    append_log(log_file, notice);
}

fn append_log(log_file: &Path, notice: &Notice) {
    let line = format!("\n[gp-tray] {}: {}\n", notice.title, notice.body);
    let appended = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .and_then(|mut file| file.write_all(line.as_bytes()));
    let _ = appended;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transition {
    None,
    Connected,
    Dropped,
    Failed,
}

fn transition(
    previous: VpnState,
    state: VpnState,
    user_disconnect: bool,
    notified_fail: bool,
) -> Transition {
    if state == previous {
        return Transition::None;
    }
    match state {
        VpnState::Connected => Transition::Connected,
        VpnState::Disconnected if !user_disconnect => match previous {
            VpnState::Connected => Transition::Dropped,
            VpnState::Authenticating | VpnState::TunnelStarting if !notified_fail => {
                Transition::Failed
            }
            _ => Transition::None,
        },
        _ => Transition::None,
    }
}

/// Watches the state stream and turns transitions into notifications, the one
/// place that owns the suppression rules.
pub fn spawn_watcher(
    engine: Engine,
    paths: Paths,
    config: PortalConfig,
    flags: Arc<Flags>,
    notices: UnboundedSender<Notice>,
) {
    tokio::spawn(async move {
        let mut states = engine.state_stream();
        let mut previous = *states.borrow_and_update();

        while states.changed().await.is_ok() {
            let state = *states.borrow_and_update();
            match transition(
                previous,
                state,
                flags.user_disconnect.load(Ordering::Relaxed),
                flags.notified_fail.load(Ordering::Relaxed),
            ) {
                Transition::Connected => {
                    let name = config.friendly(&state_files::read_active(&paths));
                    let name = if name.is_empty() {
                        "GlobalProtect".to_owned()
                    } else {
                        name
                    };
                    let _ = notices.send(Notice::connected(&name));
                    flags.notified_fail.store(false, Ordering::Relaxed);
                }
                Transition::Dropped => {
                    let _ = notices.send(Notice::dropped());
                }
                Transition::Failed => {
                    // A unit reports `inactive` for a moment while its start
                    // job is queued; only a settled disconnect is a failure.
                    tokio::time::sleep(FAILURE_SETTLE).await;
                    let settled = *states.borrow_and_update();
                    previous = settled;
                    if settled != VpnState::Disconnected
                        || flags.user_disconnect.load(Ordering::Relaxed)
                    {
                        continue;
                    }
                    if !flags.notified_fail.swap(true, Ordering::Relaxed) {
                        let body =
                            failure_body(&state_files::read_active(&paths), &paths.log_file).await;
                        let _ = notices.send(Notice::connect_failed(body));
                    }
                }
                Transition::None => {}
            }
            previous = state;

            if state == VpnState::Disconnected
                && flags.user_disconnect.swap(false, Ordering::Relaxed)
            {
                state_files::clear_active(&paths);
            }
        }
    });
}

/// What to tell the user about a failed connection: the unit's own exit
/// reason when systemd has one, else the last error line of the connect log.
pub async fn failure_body(portal: &str, log_file: &Path) -> String {
    let detail = failure_detail(portal).await;
    if detail.is_empty() {
        error_tail(log_file)
    } else {
        detail
    }
}

/// Best unprivileged hint for why the tunnel unit died: the unit's `Result`
/// and `ExecMainStatus`, the two properties any user can read.
async fn failure_detail(portal: &str) -> String {
    if portal.is_empty() {
        return String::new();
    }
    let unit = systemd::unit_name(portal);
    let Ok(connection) = systemd::connect().await else {
        return String::new();
    };
    let Ok(path) = systemd::unit_path(&connection, &unit).await else {
        return String::new();
    };
    let Ok((result, status)) = systemd::unit_failure(&connection, path.as_str()).await else {
        return String::new();
    };
    if result.is_empty() || result == "success" {
        return String::new();
    }
    format!("Tunnel exited ({result}, status {status}). Details: journalctl -u {unit}")
}

const TAIL_LINES: usize = 40;
const MAX_LENGTH: usize = 300;

/// Best guess at the reason a connection failed: the last error-looking line
/// of the connect log, else the last non-empty line.
fn error_tail(log_file: &Path) -> String {
    let Ok(bytes) = std::fs::read(log_file) else {
        return String::new();
    };
    error_tail_of(&String::from_utf8_lossy(&bytes))
}

fn error_tail_of(text: &str) -> String {
    let tail: Vec<&str> = text.lines().rev().take(TAIL_LINES).collect();

    for line in &tail {
        let line = line.trim();
        if line.starts_with("Error:")
            || line.contains("certificate verify failed")
            || line.contains("Failed to")
        {
            return truncate(line);
        }
    }
    for line in &tail {
        let line = line.trim();
        if !line.is_empty() {
            return truncate(line);
        }
    }
    String::new()
}

fn truncate(line: &str) -> String {
    line.chars().take(MAX_LENGTH).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gp-tray-notify-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn log_lines_keep_their_layout() {
        let dir = temp_dir("log");
        let log = dir.join("connect.log");
        append_log(&log, &Notice::connected("Work VPN"));
        append_log(&log, &Notice::dropped());
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "\n[gp-tray] VPN connected: Work VPN\n\n[gp-tray] VPN disconnected: The GlobalProtect tunnel dropped.\n"
        );
    }

    #[test]
    fn notices_keep_their_wording() {
        assert_eq!(Notice::connected("Work VPN").body, "Work VPN");
        assert_eq!(Notice::dropped().title, "VPN disconnected");
        assert!(Notice::dropped().critical);
        assert_eq!(
            Notice::switching("Work VPN", Some("EU Gateway")).body,
            "→ Work VPN (EU Gateway)"
        );
        assert_eq!(Notice::switching("Work VPN", None).body, "→ Work VPN");
        assert_eq!(Notice::switch_failed().title, "VPN switch failed");
        assert_eq!(
            Notice::auth_timed_out().body,
            "Authentication did not complete in time."
        );
        assert_eq!(
            Notice::tunnel_timed_out().body,
            "The tunnel did not come up in time; connection reset."
        );
        assert_eq!(Notice::auth_cancelled().title, "VPN connect cancelled");
        assert_eq!(Notice::connect_failed("boom").body, "boom");
    }

    #[test]
    fn transitions_follow_the_suppression_rules() {
        use Transition::*;
        let cases = [
            (
                VpnState::Disconnected,
                VpnState::Disconnected,
                false,
                false,
                None,
            ),
            (
                VpnState::Disconnected,
                VpnState::Authenticating,
                false,
                false,
                None,
            ),
            (
                VpnState::Authenticating,
                VpnState::TunnelStarting,
                false,
                false,
                None,
            ),
            (
                VpnState::TunnelStarting,
                VpnState::Connected,
                false,
                false,
                Connected,
            ),
            (VpnState::Connected, VpnState::Connected, false, false, None),
            (
                VpnState::Connected,
                VpnState::Disconnected,
                false,
                false,
                Dropped,
            ),
            (
                VpnState::Connected,
                VpnState::Disconnected,
                true,
                false,
                None,
            ),
            (
                VpnState::TunnelStarting,
                VpnState::Disconnected,
                false,
                false,
                Failed,
            ),
            (
                VpnState::Authenticating,
                VpnState::Disconnected,
                false,
                false,
                Failed,
            ),
            (
                VpnState::TunnelStarting,
                VpnState::Disconnected,
                true,
                false,
                None,
            ),
            (
                VpnState::TunnelStarting,
                VpnState::Disconnected,
                false,
                true,
                None,
            ),
        ];
        for (previous, state, user_disconnect, notified_fail, expected) in cases {
            assert_eq!(
                transition(previous, state, user_disconnect, notified_fail),
                expected,
                "{previous:?} -> {state:?} (user_disconnect={user_disconnect}, notified_fail={notified_fail})"
            );
        }
    }
    #[test]
    fn prefers_the_last_error_line() {
        let text = "\
    Connected to portal.example.com
    Error: gateway unreachable
    random chatter
    ";
        assert_eq!(error_tail_of(text), "Error: gateway unreachable");
    }

    #[test]
    fn recognises_error_shapes() {
        let cases = [
            ("noise\nError: bad cookie\n", "Error: bad cookie"),
            (
                "noise\nssl: certificate verify failed\n",
                "ssl: certificate verify failed",
            ),
            (
                "noise\nFailed to connect to 1.2.3.4\n",
                "Failed to connect to 1.2.3.4",
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(error_tail_of(text), expected);
        }
    }

    #[test]
    fn returns_last_error_when_noise_follows() {
        let text = "Error: first problem\nnoise one\nnoise two\n";
        assert_eq!(error_tail_of(text), "Error: first problem");
    }

    #[test]
    fn falls_back_to_the_last_non_empty_line_trimmed() {
        assert_eq!(error_tail_of("  hello  \n\n   \n"), "hello");
        assert_eq!(error_tail_of(""), "");
        assert_eq!(error_tail_of("\n\n"), "");
    }

    #[test]
    fn only_the_last_forty_lines_count() {
        let mut text = String::from("Error: ancient failure\n");
        for index in 0..45 {
            text.push_str(&format!("line {index}\n"));
        }
        assert_eq!(error_tail_of(&text), "line 44");
    }

    #[test]
    fn truncates_at_three_hundred_characters() {
        let long = format!("Error: {}", "x".repeat(400));
        assert_eq!(error_tail_of(&long).len(), MAX_LENGTH);
        let unicode = "é".repeat(400);
        assert_eq!(error_tail_of(&unicode).chars().count(), MAX_LENGTH);
    }

    #[test]
    fn missing_log_reads_as_empty() {
        assert_eq!(
            error_tail(Path::new("/nonexistent/gp-tray/connect.log")),
            ""
        );
    }

    #[test]
    fn invalid_utf8_is_ignored() {
        let dir = std::env::temp_dir().join("gp-tray-log-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("connect.log");
        let mut bytes = b"bad \xff byte\nError: real failure\n".to_vec();
        bytes.extend_from_slice(b"\xfe\n");
        std::fs::write(&file, bytes).unwrap();
        assert_eq!(error_tail(&file), "Error: real failure");
    }
}
