use std::time::Duration;

use futures_util::StreamExt;
use zbus::message::Type as MessageType;
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, MatchRule, MessageStream};

const TUNNEL_UNIT_PREFIX: &str = "gp-tray-tunnel@";
const TUNNEL_UNIT_SUFFIX: &str = ".service";

const SERVICE: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_INTERFACE: &str = "org.freedesktop.systemd1.Manager";
pub const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";

pub const MANAGER_SIGNAL_INTERFACE: &str = MANAGER_INTERFACE;
pub const UNIT_INTERFACE: &str = "org.freedesktop.systemd1.Unit";

type ListedUnit = (
    String,
    String,
    String,
    String,
    String,
    String,
    OwnedObjectPath,
    u32,
    String,
    OwnedObjectPath,
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitListing {
    pub name: String,
    pub active_state: String,
    pub path: String,
}

pub async fn connect() -> zbus::Result<Connection> {
    Connection::system().await
}

pub async fn subscribe(connection: &Connection) -> zbus::Result<()> {
    connection
        .call_method(
            Some(SERVICE),
            MANAGER_PATH,
            Some(MANAGER_INTERFACE),
            "Subscribe",
            &(),
        )
        .await?;
    Ok(())
}

pub async fn list_units(connection: &Connection) -> zbus::Result<Vec<UnitListing>> {
    let reply = connection
        .call_method(
            Some(SERVICE),
            MANAGER_PATH,
            Some(MANAGER_INTERFACE),
            "ListUnits",
            &(),
        )
        .await?;
    let listed: Vec<ListedUnit> = reply.body().deserialize()?;
    Ok(listed
        .into_iter()
        .map(|unit| UnitListing {
            name: unit.0,
            active_state: unit.3,
            path: unit.6.as_str().to_owned(),
        })
        .collect())
}

pub async fn unit_active_state(connection: &Connection, unit_path: &str) -> zbus::Result<String> {
    unit_property(connection, unit_path, "ActiveState")
        .await?
        .body()
        .deserialize()
}

/// Why a finished unit failed: `Result` and `ExecMainStatus`, the two
/// properties an unprivileged user can always read.
pub async fn unit_failure(connection: &Connection, unit_path: &str) -> zbus::Result<(String, u32)> {
    let result: String = unit_property(connection, unit_path, "Result")
        .await?
        .body()
        .deserialize()?;
    let status: u32 = unit_property(connection, unit_path, "ExecMainStatus")
        .await?
        .body()
        .deserialize()?;
    Ok((result, status))
}

/// Queues the start job and returns its object path; the caller waits for
/// `JobRemoved`, which is when the blocking `systemctl start` would return.
pub async fn start_unit(connection: &Connection, unit: &str) -> zbus::Result<OwnedObjectPath> {
    let reply = connection
        .call_method(
            Some(SERVICE),
            MANAGER_PATH,
            Some(MANAGER_INTERFACE),
            "StartUnit",
            &(unit, "replace"),
        )
        .await?;
    reply.body().deserialize()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobResult {
    Done,
    Failed(String),
    TimedOut,
}

/// Watches systemd's `JobRemoved` signal for the job `start_unit` returned.
pub struct JobWatcher {
    stream: MessageStream,
}

impl JobWatcher {
    /// Install the rule before the job starts, or its completion can be missed.
    pub async fn new(connection: &Connection) -> zbus::Result<Self> {
        subscribe(connection).await?;
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(SERVICE)?
            .interface(MANAGER_INTERFACE)?
            .member("JobRemoved")?
            .build();
        Ok(Self {
            stream: MessageStream::for_match_rule(rule, connection, Some(16)).await?,
        })
    }

    pub async fn wait(&mut self, job: &str, timeout: Duration) -> zbus::Result<JobResult> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            tokio::select! {
                message = self.stream.next() => {
                    let Some(message) = message else {
                        return Ok(JobResult::TimedOut);
                    };
                    let (_, removed, _unit, result): (u32, OwnedObjectPath, String, String) =
                        message?.body().deserialize()?;
                    if removed.as_str() == job {
                        return Ok(if result == "done" {
                            JobResult::Done
                        } else {
                            JobResult::Failed(result)
                        });
                    }
                }
                () = tokio::time::sleep_until(deadline) => return Ok(JobResult::TimedOut),
            }
        }
    }
}

pub async fn stop_unit(connection: &Connection, unit: &str) -> zbus::Result<()> {
    connection
        .call_method(
            Some(SERVICE),
            MANAGER_PATH,
            Some(MANAGER_INTERFACE),
            "StopUnit",
            &(unit, "replace"),
        )
        .await?;
    Ok(())
}

pub async fn unit_path(connection: &Connection, unit: &str) -> zbus::Result<OwnedObjectPath> {
    let reply = connection
        .call_method(
            Some(SERVICE),
            MANAGER_PATH,
            Some(MANAGER_INTERFACE),
            "GetUnit",
            &(unit,),
        )
        .await?;
    reply.body().deserialize()
}

/// The message a user can act on, without zbus' own wrapper text.
pub fn describe(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(_, Some(detail), _) => detail.clone(),
        other => other.to_string(),
    }
}

/// Mirrors `systemd-escape` for a single name: `/` becomes `-`, a leading `.`
/// and every byte outside `[A-Za-z0-9_.:-]` becomes a lowercase `\xNN` escape.
pub fn escape(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for (index, byte) in name.bytes().enumerate() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b':')
            || (byte == b'.' && index > 0);
        if byte == b'/' {
            escaped.push('-');
        } else if keep {
            escaped.push(char::from(byte));
        } else {
            escaped.push_str(&format!("\\x{byte:02x}"));
        }
    }
    escaped
}

pub fn unit_name(portal: &str) -> String {
    format!("{TUNNEL_UNIT_PREFIX}{}{TUNNEL_UNIT_SUFFIX}", escape(portal))
}

pub fn is_tunnel_unit(name: &str) -> bool {
    name.strip_prefix(TUNNEL_UNIT_PREFIX)
        .and_then(|rest| rest.strip_suffix(TUNNEL_UNIT_SUFFIX))
        .is_some_and(|instance| !instance.is_empty())
}

async fn unit_property(
    connection: &Connection,
    unit_path: &str,
    property: &str,
) -> zbus::Result<zbus::Message> {
    connection
        .call_method(
            Some(SERVICE),
            unit_path,
            Some(PROPERTIES_INTERFACE),
            "Get",
            &(UNIT_INTERFACE, property),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_our_tunnel_units() {
        for name in [
            "gp-tray-tunnel@portal.example.com.service",
            "gp-tray-tunnel@a.service",
        ] {
            assert!(is_tunnel_unit(name), "{name}");
        }
        for name in [
            "gp-tray.service",
            "gp-tray-tunnel@.service",
            "gp-tray-tunnel@a.service.d",
            "other@gp-tray-tunnel@a.service",
        ] {
            assert!(!is_tunnel_unit(name), "{name}");
        }
    }

    #[test]
    fn matches_systemd_escape_output() {
        let cases = [
            ("portal.example.com", "portal.example.com"),
            ("a b", "a\\x20b"),
            ("a-b", "a\\x2db"),
            ("a.b", "a.b"),
            ("a:b", "a:b"),
            ("a_b", "a_b"),
            ("a/b", "a-b"),
            ("A@b", "A\\x40b"),
            ("foo bar/baz", "foo\\x20bar-baz"),
            ("Ünïcode", "\\xc3\\x9cn\\xc3\\xafcode"),
            ("a+b", "a\\x2bb"),
            ("x$y", "x\\x24y"),
            ("   ", "\\x20\\x20\\x20"),
            ("", ""),
            ("a\\b", "a\\x5cb"),
            ("tab\tx", "tab\\x09x"),
            ("quote'q", "quote\\x27q"),
            ("a?b", "a\\x3fb"),
            ("a*b", "a\\x2ab"),
            (".a", "\\x2ea"),
            ("..a", "\\x2e.a"),
            (".", "\\x2e"),
            ("a.", "a."),
            (". a", "\\x2e\\x20a"),
            ("a b.c", "a\\x20b.c"),
            ("a/.b", "a-.b"),
            ("/.a", "-.a"),
            ("gp 0", "gp\\x200"),
            ("a\x01b", "a\\x01b"),
        ];
        for (input, expected) in cases {
            assert_eq!(escape(input), expected, "escaping {input:?}");
        }
    }

    #[test]
    fn builds_the_tunnel_unit_name() {
        assert_eq!(
            unit_name("portal.example.com"),
            "gp-tray-tunnel@portal.example.com.service"
        );
        assert_eq!(
            unit_name("My Portal"),
            "gp-tray-tunnel@My\\x20Portal.service"
        );
    }
}
