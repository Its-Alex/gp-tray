use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::config::PortalConfig;
use crate::engine::{Engine, VpnState};
use crate::notify::{Flags, Notice};
use crate::paths::{CONNECT_TIMEOUT, COOKIE_FILE, GATEWAY_FILE, Paths, SWITCH_TIMEOUT};
use crate::state_files;
use crate::systemd;
use crate::systemd_escape;

const START_UNIT_TIMEOUT: Duration = Duration::from_secs(30);
const VERIFY_DOWN_DELAY: Duration = Duration::from_secs(4);
const MAX_MESSAGE_LENGTH: usize = 300;

/// What the tray asked for, kept separate from the implementation so the
/// menu can stay synchronous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Connect {
        portal: String,
        gateway: Option<String>,
    },
    Disconnect,
}

pub struct Actions {
    requests: UnboundedSender<Request>,
}

impl Actions {
    pub fn spawn(
        engine: Engine,
        paths: Paths,
        config: PortalConfig,
        flags: Arc<Flags>,
        notices: UnboundedSender<Notice>,
    ) -> Self {
        let (requests, mut receiver) = unbounded_channel();
        let context = Context {
            engine,
            paths,
            config,
            flags,
            notices,
        };
        tokio::spawn(async move {
            let mut running: Option<JoinHandle<()>> = None;
            while let Some(request) = receiver.recv().await {
                if let Some(task) = running.take() {
                    task.abort();
                }
                let context = context.clone();
                running = Some(match request {
                    Request::Connect { portal, gateway } => {
                        tokio::spawn(async move { connect(context, portal, gateway).await })
                    }
                    Request::Disconnect => tokio::spawn(async move { disconnect(context).await }),
                });
            }
        });
        Self { requests }
    }

    pub fn connect(&self, portal: impl Into<String>, gateway: Option<String>) {
        let _ = self.requests.send(Request::Connect {
            portal: portal.into(),
            gateway,
        });
    }

    pub fn disconnect(&self) {
        let _ = self.requests.send(Request::Disconnect);
    }
}

#[derive(Clone)]
struct Context {
    engine: Engine,
    paths: Paths,
    config: PortalConfig,
    flags: Arc<Flags>,
    notices: UnboundedSender<Notice>,
}

impl Context {
    fn notify(&self, notice: Notice) {
        let _ = self.notices.send(notice);
    }

    fn clear_active(&self) {
        state_files::clear_active(&self.paths);
    }
}

async fn connect(context: Context, portal: String, gateway: Option<String>) {
    if context.engine.state() != VpnState::Disconnected {
        let active = state_files::read_active(&context.paths);
        let active_gateway = state_files::read_active_gateway(&context.paths);
        if active != portal || active_gateway != gateway.clone().unwrap_or_default() {
            context.notify(Notice::switching(
                &context.config.friendly(&portal),
                gateway.as_deref(),
            ));
            context.flags.user_disconnect.store(true, Ordering::Relaxed);
            teardown().await;
            let _ = state_files::write_active(&context.paths, &portal);
            let _ = state_files::write_active_gateway(&context.paths, gateway.as_deref());
            if !wait_until_disconnected(&context, SWITCH_TIMEOUT).await {
                context.notify(Notice::switch_failed());
                return;
            }
            context
                .flags
                .user_disconnect
                .store(false, Ordering::Relaxed);
        }
    }
    authenticate_and_start(&context, portal, gateway).await;
}

async fn disconnect(context: Context) {
    context.flags.user_disconnect.store(true, Ordering::Relaxed);
    teardown().await;
    tokio::time::sleep(VERIFY_DOWN_DELAY).await;
    if context.engine.state() != VpnState::Disconnected {
        teardown().await;
    } else {
        context.clear_active();
        context
            .flags
            .user_disconnect
            .store(false, Ordering::Relaxed);
    }
}

async fn authenticate_and_start(context: &Context, portal: String, gateway: Option<String>) {
    let _ = state_files::write_active(&context.paths, &portal);
    let _ = state_files::write_active_gateway(&context.paths, gateway.as_deref());
    context
        .flags
        .user_disconnect
        .store(false, Ordering::Relaxed);
    context.flags.notified_fail.store(false, Ordering::Relaxed);
    let deadline = Instant::now() + CONNECT_TIMEOUT;

    let authenticating = Authenticating::start(&context.engine);
    let outcome = run_auth(&context.paths.log_file, &portal, deadline).await;
    drop(authenticating);

    let (succeeded, cookie) = match outcome {
        AuthOutcome::Finished { succeeded, cookie } => (succeeded, cookie),
        AuthOutcome::TimedOut => {
            context.notify(Notice::auth_timed_out());
            context.flags.notified_fail.store(true, Ordering::Relaxed);
            context.clear_active();
            return;
        }
        AuthOutcome::SpawnFailed(error) => {
            context.notify(Notice::connect_failed(truncate(&format!(
                "cannot run gpauth: {error}"
            ))));
            context.flags.notified_fail.store(true, Ordering::Relaxed);
            context.clear_active();
            return;
        }
    };

    if !succeeded || cookie.iter().all(u8::is_ascii_whitespace) {
        let suppressed = context.flags.user_disconnect.load(Ordering::Relaxed)
            || context.flags.notified_fail.load(Ordering::Relaxed);
        if !suppressed {
            context.notify(Notice::auth_cancelled());
            context.flags.notified_fail.store(true, Ordering::Relaxed);
        }
        context.clear_active();
        return;
    }

    if let Err(detail) = start_tunnel(context, &portal, gateway.as_deref(), &cookie).await {
        context.notify(Notice::connect_failed(detail));
        context.flags.notified_fail.store(true, Ordering::Relaxed);
        context.clear_active();
        return;
    }

    wait_for_tunnel(context, &portal, deadline).await;
}

struct Authenticating<'a> {
    engine: &'a Engine,
}

impl<'a> Authenticating<'a> {
    fn start(engine: &'a Engine) -> Self {
        engine.set_authenticating(true);
        Self { engine }
    }
}

impl Drop for Authenticating<'_> {
    fn drop(&mut self) {
        self.engine.set_authenticating(false);
    }
}

enum AuthOutcome {
    Finished { succeeded: bool, cookie: Vec<u8> },
    TimedOut,
    SpawnFailed(io::Error),
}

/// Runs `gpauth` in the user's session: SAML happens in the browser, its
/// stdout is the single-use cookie the tunnel unit needs.
async fn run_auth(log_file: &Path, portal: &str, deadline: Instant) -> AuthOutcome {
    let stderr = match OpenOptions::new().create(true).append(true).open(log_file) {
        Ok(file) => file,
        Err(error) => return AuthOutcome::SpawnFailed(error),
    };
    let spawned = Command::new("gpauth")
        .args(["--browser", "default", portal])
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .process_group(0)
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return AuthOutcome::SpawnFailed(error),
    };
    let Some(mut pipe) = child.stdout.take() else {
        return AuthOutcome::SpawnFailed(io::Error::other("gpauth has no stdout pipe"));
    };
    let reader = tokio::spawn(async move {
        let mut cookie = Vec::new();
        let _ = pipe.read_to_end(&mut cookie).await;
        cookie
    });

    let status = tokio::select! {
        status = child.wait() => status,
        () = tokio::time::sleep_until(deadline) => {
            terminate(&mut child).await;
            return AuthOutcome::TimedOut;
        }
    };
    let cookie = reader.await.unwrap_or_default();
    match status {
        Ok(status) => AuthOutcome::Finished {
            succeeded: status.success(),
            cookie,
        },
        Err(error) => AuthOutcome::SpawnFailed(error),
    }
}

async fn terminate(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

async fn start_tunnel(
    context: &Context,
    portal: &str,
    gateway: Option<&str>,
    cookie: &[u8],
) -> Result<(), String> {
    write_handoff(
        Path::new(COOKIE_FILE),
        Path::new(GATEWAY_FILE),
        cookie,
        gateway,
    )?;

    let unit = systemd_escape::unit_name(portal);
    let started = match systemd::connect().await {
        Ok(connection) => {
            tokio::time::timeout(START_UNIT_TIMEOUT, systemd::start_unit(&connection, &unit)).await
        }
        Err(error) => {
            remove_handoff();
            return Err(truncate(&systemd::describe(&error)));
        }
    };
    remove_handoff();

    match started {
        Err(_) => Err("systemctl start timed out".to_owned()),
        Ok(Err(error)) => Err(truncate(&systemd::describe(&error))),
        Ok(Ok(())) => {
            let _ = state_files::write_last_gateway(&context.paths, portal, gateway);
            Ok(())
        }
    }
}

async fn wait_for_tunnel(context: &Context, portal: &str, deadline: Instant) {
    let unit = systemd_escape::unit_name(portal);
    let mut states = context.engine.state_stream();
    let mut connection: Option<zbus::Connection> = None;

    loop {
        let state = *states.borrow_and_update();
        match state {
            VpnState::Connected => return,
            VpnState::Disconnected => {
                if !unit_active(&mut connection, &unit).await {
                    return;
                }
            }
            VpnState::Authenticating | VpnState::TunnelStarting => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        tokio::select! {
            changed = states.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = tokio::time::sleep(remaining) => {
                teardown().await;
                context.notify(Notice::tunnel_timed_out());
                context.flags.notified_fail.store(true, Ordering::Relaxed);
                context.clear_active();
                return;
            }
        }
    }
}

async fn unit_active(connection: &mut Option<zbus::Connection>, unit: &str) -> bool {
    if connection.is_none() {
        *connection = systemd::connect().await.ok();
    }
    let Some(connection) = connection.as_ref() else {
        return true;
    };
    let Ok(path) = systemd::unit_path(connection, unit).await else {
        return false;
    };
    match systemd::unit_active_state(connection, path.as_str()).await {
        Ok(state) => matches!(state.as_str(), "active" | "activating" | "reloading"),
        Err(_) => true,
    }
}

async fn wait_until_disconnected(context: &Context, timeout: Duration) -> bool {
    let mut states = context.engine.state_stream();
    let deadline = Instant::now() + timeout;
    loop {
        if *states.borrow_and_update() == VpnState::Disconnected {
            return true;
        }
        tokio::select! {
            changed = states.changed() => {
                if changed.is_err() {
                    return true;
                }
            }
            () = tokio::time::sleep_until(deadline) => {
                return context.engine.state() == VpnState::Disconnected;
            }
        }
    }
}

async fn teardown() {
    let Ok(connection) = systemd::connect().await else {
        return;
    };
    let Ok(units) = systemd::list_units(&connection).await else {
        return;
    };
    let stops = units
        .iter()
        .filter(|unit| systemd::is_tunnel_unit(&unit.name))
        .map(|unit| systemd::stop_unit(&connection, &unit.name));
    futures_util::future::join_all(stops).await;
}

fn write_handoff(
    cookie_file: &Path,
    gateway_file: &Path,
    cookie: &[u8],
    gateway: Option<&str>,
) -> Result<(), String> {
    for path in [cookie_file, gateway_file] {
        let _ = std::fs::remove_file(path);
    }
    write_exclusive(cookie_file, cookie)?;
    write_exclusive(gateway_file, gateway.unwrap_or("").as_bytes())
}

fn write_exclusive(path: &Path, data: &[u8]) -> Result<(), String> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path);
    match file {
        Ok(mut file) => file
            .write_all(data)
            .map_err(|error| describe_handoff_error(&error)),
        Err(error) => Err(describe_handoff_error(&error)),
    }
}

fn remove_handoff() {
    for path in [COOKIE_FILE, GATEWAY_FILE] {
        let _ = std::fs::remove_file(path);
    }
}

/// The two handoff failures a user can actually fix, in words they can act on.
fn describe_handoff_error(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => {
            "/run/gp-tray missing — system support not installed (systemd-tmpfiles --create gp-tray.conf)"
                .to_owned()
        }
        io::ErrorKind::PermissionDenied | io::ErrorKind::AlreadyExists => {
            "cannot write /run/gp-tray files (owned by another user?)".to_owned()
        }
        _ => error.to_string(),
    }
}

fn truncate(message: &str) -> String {
    message.chars().take(MAX_MESSAGE_LENGTH).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gp-tray-actions-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn handoff_files_are_exclusive_private_and_raw() {
        let dir = temp_dir("handoff");
        let cookie = dir.join("cookie");
        let gateway = dir.join("gateway");
        write_handoff(&cookie, &gateway, b"cookie\nvalue\n", Some("EU Gateway")).unwrap();

        assert_eq!(std::fs::read(&cookie).unwrap(), b"cookie\nvalue\n");
        assert_eq!(std::fs::read(&gateway).unwrap(), b"EU Gateway");
        for path in [&cookie, &gateway] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{path:?}");
        }
    }

    #[test]
    fn handoff_replaces_stale_files_and_writes_empty_gateway_for_auto() {
        let dir = temp_dir("stale");
        let cookie = dir.join("cookie");
        let gateway = dir.join("gateway");
        std::fs::write(&cookie, "stale").unwrap();
        std::fs::write(&gateway, "stale").unwrap();

        write_handoff(&cookie, &gateway, b"fresh", None).unwrap();
        assert_eq!(std::fs::read(&cookie).unwrap(), b"fresh");
        assert_eq!(std::fs::read(&gateway).unwrap(), b"");
    }

    #[test]
    fn missing_handoff_directory_is_reported_in_words() {
        let dir = temp_dir("missing");
        let cookie = dir.join("absent").join("cookie");
        let gateway = dir.join("absent").join("gateway");
        assert_eq!(
            write_handoff(&cookie, &gateway, b"x", None).unwrap_err(),
            "/run/gp-tray missing — system support not installed (systemd-tmpfiles --create gp-tray.conf)"
        );
    }

    #[test]
    fn unwritable_handoff_directory_is_reported_in_words() {
        let dir = temp_dir("readonly");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let cookie = dir.join("cookie");
        let gateway = dir.join("gateway");
        let message = write_handoff(&cookie, &gateway, b"x", None).unwrap_err();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            message,
            "cannot write /run/gp-tray files (owned by another user?)"
        );
    }

    #[test]
    fn failures_are_truncated_to_three_hundred_characters() {
        assert_eq!(truncate(&"x".repeat(400)).len(), MAX_MESSAGE_LENGTH);
        assert_eq!(truncate("short"), "short");
    }
}
