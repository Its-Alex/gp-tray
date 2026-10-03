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
| `systemctl` | Tray | Starts and stops the tunnel unit; reads its state and failure reason | `systemd` | No connect, disconnect, or state detection |
| `systemd-escape` | Tray | Turns a portal hostname into a valid unit instance name | `systemd` | Unit names for portals with special characters are wrong |
| `ip` | Tray | Checks whether the `gp0` interface is up (connected vs connecting) | `iproute2` | Tray never shows Connected |
| `notify-send` | Tray | Desktop notifications (connected, dropped, failed, cancelled) | `libnotify` | Silent failures; the tray icon still updates |
| `xdg-open` | Tray, "View connect log" menu | Opens the log file in your default editor | `xdg-utils` | That menu entry does nothing |
| `rm`, `cat`, `grep` | Tunnel unit and its helper | Delete the handoff files after start; read and validate the gateway choice | `coreutils`, `grep` (in `base`) | Always present on Arch |

## Runtime: libraries and language

| Component | Why | Package |
|---|---|---|
| Python 3 | The tray is a Python script | `python` |
| PyGObject | Python bindings to GLib and GTK (main loop, timers, menus) | `python-gobject` |
| GTK 3 | The tray menu widgets | `gtk3` |
| Ayatana AppIndicator | Publishes the tray icon over the StatusNotifierItem protocol that desktop panels display. The tray also accepts the older `AppIndicator3` library if Ayatana is absent. | `libayatana-appindicator` |
| hicolor icon theme | Provides the icon directory structure the state icons install into | `hicolor-icon-theme` |

At startup the library prints `libayatana-appindicator is deprecated`. That
warning is harmless; it refers to the upstream library's future, not to a
problem in gp-tray.

## Runtime: system services

| Service | Role | Package |
|---|---|---|
| systemd (system instance) | Runs and supervises the root tunnel unit, hands it the cookie and gateway as private credentials | `systemd` |
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
| `procps-ng` (`pgrep`, `pkill`) | Unit state from `systemctl` plus the `gp0` interface check, which is accurate where process matching was not |

## Build and release tooling (maintainers only)

None of these are needed to use gp-tray.

| Tool | Used by | Why |
|---|---|---|
| `make`, `install`, `sed` | Makefile | Install files to the right places, fill install paths into the unit templates |
| `makepkg` | `scripts/release.sh publish`, CI | Build and verify the Arch package, generate `.SRCINFO` |
| `curl`, `sha256sum` | `scripts/release.sh prepare` | Download the release tarball and pin its checksum |
| `git` | `scripts/release.sh` | Tag releases, push the pinned PKGBUILD, push to the AUR |
| `python3` | `scripts/release.sh tag` | Byte-compile the tray as a pre-release syntax check |
| [KSXGitHub/github-actions-deploy-aur](https://github.com/KSXGitHub/github-actions-deploy-aur) | GitHub workflow | Build the package in an Arch container and push it to the AUR on each tag |

## Other distributions

gp-tray is only packaged for Arch, but runs anywhere these are available.
Install `gpclient`/`gpauth` from the
[GlobalProtect-openconnect releases](https://github.com/yuezk/GlobalProtect-openconnect#installation),
then:

```bash
# Debian / Ubuntu / Pop!_OS
sudo apt install python3-gi gir1.2-gtk-3.0 gir1.2-ayatanaappindicator3-0.1 \
    libnotify-bin iproute2 xdg-utils polkitd
# Fedora
sudo dnf install python3-gobject gtk3 libayatana-appindicator-gtk3 \
    libnotify iproute xdg-utils polkit
```

systemd is assumed present on all of them. Then install from source with
`sudo make install PREFIX=/usr`, which puts the polkit rule and tmpfiles
entry where those distributions look for them.
