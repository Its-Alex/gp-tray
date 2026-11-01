use zbus::Connection;
use zbus::zvariant::OwnedObjectPath;

const SERVICE: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_INTERFACE: &str = "org.freedesktop.systemd1.Manager";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";

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
