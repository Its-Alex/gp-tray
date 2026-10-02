# gp-tray

A free, lightweight system-tray VPN indicator for **GlobalProtect** on Linux,
built on the open-source [`gpclient`](https://github.com/yuezk/GlobalProtect-openconnect).
It gives you the one thing the official client doesn't ship on Linux: a proper
tray applet with connect/disconnect, multi-portal switching, and desktop alerts.

This is a fork of [DavidVeksler/gp-tray](https://github.com/DavidVeksler/gp-tray),
reworked "the Linux way" — see [Changes from upstream](#changes-from-upstream).
Packaged for Arch Linux on the [AUR](https://aur.archlinux.org/packages/gp-tray).

![status](https://img.shields.io/badge/platform-linux-blue) ![license](https://img.shields.io/badge/license-MIT-green)

<p align="center">
  <img src="docs/tray-menu.png" alt="gp-tray menu showing the connected portal, a one-click switch to another portal, and disconnect" width="360">
</p>

## Features

- **Live status** in the tray — Connected / Connecting / Disconnected, with the
  active portal shown by name. Each state has its own colored icon (green /
  amber / grey) so the tray reads at a glance regardless of icon theme.
- **Multiple portals**, switched one click at a time (GlobalProtect runs a
  single tunnel). Great when one portal is for databases and another for VMs.
- **One-click switching** — picking another portal auto-disconnects the current
  tunnel, waits for it to drop, then connects the new one.
- **Desktop notifications** on:
  - connect failure (shows the real error line from the log, e.g. a TLS/cert problem),
  - authentication cancelled/dismissed,
  - unexpected tunnel drop,
  - successful connect.
- **Accurate state detection** — tunnel state comes from the systemd unit
  plus a live tun device, not process heuristics.
- **No pkexec, no password prompts** — SAML auth runs unprivileged in your
  browser (`gpauth`); the tunnel runs as a systemd **system** unit
  (`gp-tray-tunnel@<portal>.service`) that a shipped polkit rule lets active
  local users start/stop without authentication.

## Changes from upstream

What this fork changes compared to
[DavidVeksler/gp-tray](https://github.com/DavidVeksler/gp-tray):

- **No pkexec, no password prompts.** Upstream ran
  `pkexec gpclient connect/disconnect`, prompting for a password on every
  action and force-killing processes on disconnect. Here SAML auth runs
  unprivileged (`gpauth` in your browser) and the tunnel is a systemd
  **system** unit (`gp-tray-tunnel@<portal>.service`) with a fixed command
  line, authorized by a shipped polkit rule — see
  [How privileges work](#how-privileges-work).
- **systemd user service** (`gp-tray.service`, bound to
  `graphical-session.target`) instead of an ad-hoc
  `X-GNOME-Autostart` desktop file.
- **Supervised tunnel** — the VPN survives tray restarts, logs to the
  journal, and is torn down cleanly (`SIGINT` = openconnect logout).
- **Honest state detection** — systemd unit state cross-checked with a live
  tun device, replacing `pgrep gpclient` heuristics.
- **XDG Base Directory compliance** — config in
  `$XDG_CONFIG_HOME/gp-tray/`, logs/state in `$XDG_CACHE_HOME/gp-tray/`,
  with a commented `portals.conf` template created on first run.
- **Distinct per-state tray icons** (green / amber / grey shields) shipped
  and installed into the hicolor theme — upstream relied on theme VPN
  glyphs that are often missing or indistinguishable.
- **Proper packaging** — a `Makefile` with `DESTDIR`/`PREFIX`
  (`install.sh` removed), the canonical
  [PKGBUILD](packaging/arch/PKGBUILD) in-tree, an
  [AUR package](https://aur.archlinux.org/packages/gp-tray), and a
  [release pipeline](RELEASING.md) (`scripts/release.sh` + GitHub Actions)
  that publishes each tag to the AUR automatically.

## How privileges work

```
you click Connect
  └─ gpauth --browser default <portal>        (your user, browser SAML)
       └─ cookie → /run/gp-tray/cookie        (0600, single-use)
            └─ systemctl start gp-tray-tunnel@<portal>.service
                 └─ gpclient connect --cookie-on-stdin   (root, supervised
                    by systemd, cookie via systemd credentials, then deleted)
```

Only the tunnel itself runs as root, under a static unit whose command line
is fixed on disk; the polkit rule authorizes exactly that unit's
start/stop/restart for active local sessions (the same trust model as
NetworkManager). Tunnel logs: `journalctl -u 'gp-tray-tunnel@*'`.

## Requirements

- [`gpclient`](https://github.com/yuezk/GlobalProtect-openconnect) (`gpclient`, `gpservice`)
- Python 3 with GTK 3 introspection and an AppIndicator library:
  ```bash
  # Debian / Ubuntu / Pop!_OS
  sudo apt install python3-gi gir1.2-gtk-3.0 gir1.2-ayatanaappindicator3-0.1 libnotify-bin
  # Fedora
  sudo dnf install python3-gobject gtk3 libayatana-appindicator-gtk3 libnotify
  # Arch
  sudo pacman -S python-gobject gtk3 libayatana-appindicator libnotify
  ```
- polkit — evaluates the shipped rule that authorizes tunnel start/stop
  (`pkexec` itself is not used).
- **GNOME Shell users:** the top-bar tray icon needs the [AppIndicator and
  KStatusNotifierItem Support](https://extensions.gnome.org/extension/615/appindicator-support/)
  extension — vanilla GNOME Shell (Wayland or X11) hides AppIndicator icons
  without it. KDE Plasma, XFCE, Cinnamon, MATE, and other tray-capable
  desktops work out of the box.

## Install

### Arch Linux (AUR)

```bash
yay -S gp-tray   # or: paru -S gp-tray
```

### From source

```bash
git clone https://github.com/Its-Alex/gp-tray.git
cd gp-tray
make install-user               # tray for this user (~/.local)
sudo make install-privileged    # tunnel unit, root helper, polkit rule, tmpfiles
sudo systemd-tmpfiles --create gp-tray.conf && sudo systemctl daemon-reload
# or everything system-wide in one go:
sudo make install PREFIX=/usr
```

### Run it

gp-tray runs as a systemd **user** service tied to your graphical session:

```bash
systemctl --user daemon-reload
systemctl --user enable --now gp-tray.service
```

On first run it creates `~/.config/gp-tray/portals.conf` — edit it, then
restart the service:

```bash
$EDITOR ~/.config/gp-tray/portals.conf
```

```ini
# Friendly Name = portal.hostname
Work VPN      = portal.example.com
Secondary VPN = vpn2.example.com
```

```bash
systemctl --user restart gp-tray.service
```

Logs go to the journal: `journalctl --user -u gp-tray.service`.

## Configuration

Portals live in `$XDG_CONFIG_HOME/gp-tray/portals.conf`
(`~/.config/gp-tray/portals.conf` by default), one per line as
`Friendly Name = hostname`. Lines starting with `#` are ignored. You can also
set the `GP_PORTAL` environment variable to inject an extra portal at runtime.

## Troubleshooting

**`certificate verify failed` / `unable to get local issuer certificate`.**
Your portal is serving a valid cert but omitting an intermediate CA. Fetch the
intermediate named in the leaf certificate's *CA Issuers* URL, then trust it:

```bash
sudo cp intermediate.crt /usr/local/share/ca-certificates/
sudo update-ca-certificates
```

As a last resort you can add `--ignore-tls-errors` to the `gpclient` invocation
in `/usr/lib/gp-tray/gp-tray-tunnel`, but installing the intermediate is the
correct, secure fix.

**`systemctl start` asks for a password / fails with "Access denied".** The
polkit rule isn't installed (or polkit wasn't restarted after a manual
install). Check `/usr/share/polkit-1/rules.d/50-gp-tray.rules` (package) or
`/etc/polkit-1/rules.d/` (source install).

**Connect fails with `/run/gp-tray missing`.** The tmpfiles entry hasn't been
applied yet: `sudo systemd-tmpfiles --create gp-tray.conf`.

**Menu stuck on "Connecting".** Fixed in this project — older approaches keyed
off a separate `openconnect` process, which `gpclient` 2.x no longer spawns.

## Releasing

See [RELEASING.md](RELEASING.md) for the tag + AUR update process.

## License

MIT © David Veksler (original author). Fork maintained by
[Its-Alex](https://github.com/Its-Alex). Not affiliated with Palo Alto Networks
or GlobalProtect.
