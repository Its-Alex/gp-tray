# Dependencies and external tools

Every program gp-tray runs and every library it loads, why it is needed,
which package provides it, and what breaks without it. Package names are the
Arch Linux ones from the [PKGBUILD](../packaging/arch/PKGBUILD), which is the
authoritative list; other distributions are covered at the end.

For how these pieces fit together, see [architecture.md](architecture.md).

## Runtime: programs gp-tray executes

| Program | Called by | Why | Package | Without it |
|---|---|---|---|---|
| `gpauth` | Tray, on connect | Performs the SAML sign-in in your browser, as you, and prints the single-use cookie | `globalprotect-openconnect` | Cannot authenticate; connect fails immediately |
| `gpclient` | Tunnel unit (root) | Establishes the VPN tunnel from the cookie, on interface `gp0`, with the chosen or automatic gateway | `globalprotect-openconnect` | Tunnel unit fails at start |
| `xdg-open` | Tray, "View connect log" menu | Opens the log file in your default editor | `xdg-utils` | That menu entry does nothing |
| `rm`, `cat`, `grep` | Tunnel unit and its helper | Delete the handoff files after start; read and validate the gateway choice | `coreutils`, `grep` (in `base`) | Always present on Arch |

The tray itself runs no other program: it talks to systemd over D-Bus instead
of shelling out to `systemctl`, reads `gp0` from netlink instead of `ip`,
escapes unit names itself instead of calling `systemd-escape`, and sends
notifications over D-Bus instead of `notify-send`.

## Runtime: libraries

The tray is a single statically linked Rust binary. It needs the C library
and a running D-Bus daemon, nothing else — no Python interpreter, GTK, or
AppIndicator library.

| Component | Why | Package |
|---|---|---|
| glibc | The binary links it (as every Arch binary does) | `glibc` |
| D-Bus daemon (system and session bus) | Carries the systemd manager API, the tray icon protocol and desktop notifications | `dbus` / `dbus-broker` |
| hicolor icon theme | Provides the icon directory structure the state icons install into | `hicolor-icon-theme` |

D-Bus interfaces the tray speaks directly: `org.freedesktop.systemd1` on the
system bus (unit state, `StartUnit`/`StopUnit`); `org.kde.StatusNotifierItem`
/ `org.kde.StatusNotifierWatcher` and `com.canonical.dbusmenu` on the session
bus (icon and menu); `org.freedesktop.Notifications` for desktop alerts.

## Runtime: system services

| Service | Role | Package |
|---|---|---|
| systemd (system instance) | Runs and supervises the root tunnel unit, hands it the cookie and gateway as private credentials, and serves the manager API the tray subscribes to (`org.freedesktop.systemd1`) | `systemd` |
| systemd (user instance) | Runs the tray itself, tied to your graphical session | `systemd` |
| systemd-tmpfiles | Creates the `/run/gp-tray` handoff directory at boot | `systemd` |
| polkit | Evaluates the shipped rule that lets active local users start and stop the tunnel unit without a password. `pkexec`, which ships in the same package, is not used. | `polkit` |
| journald | Stores the tunnel logs (`journalctl -u 'gp-tray-tunnel@*'`) | `systemd` |

## Optional

| Component | When you need it | Package |
|---|---|---|
| AppIndicator GNOME Shell extension | Only on GNOME Shell, which hides tray icons without it. KDE Plasma, XFCE, Cinnamon, MATE and others display them natively. | `gnome-shell-extension-appindicator` |
| Web browser | `gpauth` uses your default browser for SAML. Any browser works; none is declared because every desktop has one. | (any) |

## Removed dependencies, and why

| Former dependency | Replaced by |
|---|---|
| `pkexec` (as a tool) | The systemd unit plus polkit rule; no password prompts |
| `procps-ng` (`pgrep`, `pkill`) | Unit state from systemd plus the `gp0` link, which is accurate where process matching was not |
| Python 3, PyGObject, GTK 3, Ayatana AppIndicator | One Rust binary that speaks the StatusNotifierItem protocol itself |
| `libnotify` / `notify-send` | `org.freedesktop.Notifications` over the session bus |
| `systemctl` | The systemd manager D-Bus API (`StartUnit`, `StopUnit`, `ListUnits`, unit signals) |
| `systemd-escape` | Unit-name escaping implemented in the tray, with tests against `systemd-escape` output |
| `iproute2` (`ip`) | Kernel netlink link events (`RTNLGRP_LINK`) for `gp0` |

## Build and release tooling (maintainers only)

None of these are needed to use gp-tray.

| Tool | Used by | Why |
|---|---|---|
| `cargo`, `rustc` | `make install`, `make install-user`, PKGBUILD | Build the tray (`cargo build --release --locked`) |
| `make`, `install`, `sed` | Makefile | Install files to the right places, fill install paths into the unit templates |
| `makepkg` | `scripts/release.sh publish`, CI | Build and verify the Arch package, generate `.SRCINFO` |
| `curl`, `sha256sum` | `scripts/release.sh prepare` | Download the release tarball and pin its checksum |
| `git` | `scripts/release.sh` | Tag releases, push the pinned PKGBUILD, push to the AUR |
| [KSXGitHub/github-actions-deploy-aur](https://github.com/KSXGitHub/github-actions-deploy-aur) | GitHub workflow | Build the package in an Arch container and push it to the AUR on each tag |

## Other distributions

gp-tray is only packaged for Arch, but runs anywhere these are available.
Install `gpclient`/`gpauth` from the
[GlobalProtect-openconnect releases](https://github.com/yuezk/GlobalProtect-openconnect#installation),
then build the tray:

```bash
cargo build --release --locked
sudo make install PREFIX=/usr
```

That puts the polkit rule and tmpfiles entry where your distribution looks
for them. At runtime the tray needs polkit and a D-Bus session and system bus,
which systemd-based desktops already provide; there is nothing to install for
Python, GTK or AppIndicator.
