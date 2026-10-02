#!/usr/bin/env python3
"""System-tray status indicator for GlobalProtect (gpclient).

A free, dependency-light AppIndicator that lets you connect/disconnect the
GlobalProtect VPN via the open-source `gpclient`, switch between multiple
portals one click at a time, and get desktop notifications on connect
failure, auth cancel, unexpected drop, and successful connect.

Privilege model (no pkexec): SAML auth runs as the user (gpauth, browser in
the session); the cookie is handed via /run/gp-tray/cookie to the root
systemd unit gp-tray-tunnel@<portal>.service, which the user may start/stop
without a prompt thanks to the shipped polkit rule.

Portals are read from ~/.config/gp-tray/portals.conf (see portals.conf.example),
one per line:  Friendly Name = portal.hostname
"""
import os, subprocess, time, gi
gi.require_version("Gtk", "3.0"); gi.require_version("GLib", "2.0")
try:
    gi.require_version("AyatanaAppIndicator3", "0.1")
    from gi.repository import AyatanaAppIndicator3 as AppIndicator
except (ValueError, ImportError):
    gi.require_version("AppIndicator3", "0.1")
    from gi.repository import AppIndicator3 as AppIndicator
from gi.repository import Gtk, GLib

XDG_CONFIG_HOME = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
XDG_CACHE_HOME = os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache")
CONFIG_DIR = os.path.join(XDG_CONFIG_HOME, "gp-tray")
CONFIG_FILE = os.path.join(CONFIG_DIR, "portals.conf")
LOG_DIR = os.path.join(XDG_CACHE_HOME, "gp-tray")
LOG_FILE = os.path.join(LOG_DIR, "connect.log")
ACTIVE_FILE = os.path.join(LOG_DIR, "active_portal")
POLL_SECONDS = 3
CONNECT_TIMEOUT = 120  # seconds; abort an auth/connect that never brings up a tun
COOKIE_FILE = "/run/gp-tray/cookie"  # handoff to gp-tray-tunnel@.service (tmpfiles.d)
TUNNEL_UNIT_GLOB = "gp-tray-tunnel@*.service"
TUN_IFNAME = "gp0"  # fixed by --interface in the gp-tray-tunnel helper
ICON_CONNECTED = "gp-tray-connected"
ICON_CONNECTING = "gp-tray-connecting"
ICON_DISCONNECTED = "gp-tray-disconnected"
# When running straight from the repo the icons aren't installed in any icon
# theme, so point the indicator at the icons/ directory next to this script.
ICON_FALLBACK_DIR = os.path.join(
    os.path.dirname(os.path.realpath(__file__)), "icons")


CONFIG_TEMPLATE = """\
# gp-tray portals — one per line:  Friendly Name = portal.hostname
# Lines starting with '#' are ignored.
#
#Work VPN      = portal.example.com
#Secondary VPN = vpn2.example.com
"""


def ensure_config():
    """Create the config directory and a commented template on first run so
    users can just edit ~/.config/gp-tray/portals.conf."""
    if os.path.exists(CONFIG_FILE):
        return
    try:
        os.makedirs(CONFIG_DIR, exist_ok=True)
        with open(CONFIG_FILE, "x") as f:
            f.write(CONFIG_TEMPLATE)
    except OSError:
        pass


def load_portals():
    """Read (friendly name, host) pairs from the config file. Falls back to
    example placeholders so first run still shows something to edit."""
    portals = []
    try:
        with open(CONFIG_FILE) as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#") or "=" not in line:
                    continue
                name, host = line.split("=", 1)
                name, host = name.strip(), host.strip()
                if host:
                    portals.append((name or host, host))
    except OSError:
        pass
    if not portals:
        portals = [("Work VPN", "portal.example.com"),
                   ("Secondary VPN", "vpn2.example.com")]
    env = os.environ.get("GP_PORTAL")  # optional override
    if env and env not in {h for _, h in portals}:
        portals.insert(0, (env, env))
    return portals


ensure_config()
PORTALS = load_portals()
NAME_OF = {host: name for name, host in PORTALS}


def friendly(portal):
    return NAME_OF.get(portal, portal) if portal else ""


def notify(title, body="", critical=False, icon="network-vpn-symbolic"):
    try:
        subprocess.Popen([
            "notify-send", "--app-name=GlobalProtect",
            "-u", "critical" if critical else "normal",
            "-i", icon, title, body], start_new_session=True)
    except Exception:
        pass
    try:
        with open(LOG_FILE, "a") as f:
            f.write(f"\n[gp-tray] {title}: {body}\n")
    except OSError:
        pass


def log_error_tail():
    try:
        with open(LOG_FILE, errors="ignore") as f:
            lines = f.read().splitlines()
    except OSError:
        return ""
    for ln in reversed(lines[-40:]):
        s = ln.strip()
        if s.startswith("Error:") or "certificate verify failed" in s or "Failed to" in s:
            return s[:300]
    for ln in reversed(lines[-40:]):
        if ln.strip():
            return ln.strip()[:300]
    return ""


def _tun_up():
    """True when *our* tunnel interface is up. The interface name is pinned
    by the tunnel helper (--interface gp0), so other tun devices on the
    system (tailscale0, virtual machines, …) never count as VPN state."""
    try:
        out = subprocess.run(["ip", "-br", "link", "show", "dev", TUN_IFNAME],
                             capture_output=True, text=True).stdout
        return "UP" in out
    except Exception:
        return False


_UNIT_CACHE = {}


def unit_for(portal):
    """systemd unit name for a portal's tunnel (instance is systemd-escaped)."""
    if portal not in _UNIT_CACHE:
        esc = subprocess.run(["systemd-escape", portal],
                             capture_output=True, text=True).stdout.strip()
        _UNIT_CACHE[portal] = f"gp-tray-tunnel@{esc}.service"
    return _UNIT_CACHE[portal]


def unit_state(unit):
    out = subprocess.run(["systemctl", "is-active", unit],
                         capture_output=True, text=True).stdout
    return out.split("\n", 1)[0].strip()


def tunnel_failure_detail(portal):
    """Best unprivileged hint for why the tunnel unit died (journal access
    may be restricted, but unit properties are world-readable)."""
    out = subprocess.run(
        ["systemctl", "show", unit_for(portal),
         "-p", "Result", "-p", "ExecMainStatus"],
        capture_output=True, text=True).stdout
    props = dict(l.split("=", 1) for l in out.splitlines() if "=" in l)
    result = props.get("Result", "")
    if result in ("", "success"):
        return ""
    return (f"Tunnel exited ({result}, status {props.get('ExecMainStatus', '?')}). "
            f"Details: journalctl -u {unit_for(portal)}")


def vpn_state(active_portal):
    """Tunnel state = our systemd unit's state cross-checked with our tun
    interface (the unit is 'active' from exec on, before the tunnel is up).
    Without a recorded portal (e.g. tray restart), fall back to matching any
    of our tunnel units by glob."""
    unit = unit_for(active_portal) if active_portal else TUNNEL_UNIT_GLOB
    if unit_state(unit) in ("active", "activating", "reloading"):
        return "connected" if _tun_up() else "connecting"
    # Unit-less but our interface exists: a gp0 tunnel started by hand.
    return "connected" if _tun_up() else "disconnected"


def read_active():
    try:
        with open(ACTIVE_FILE) as f:
            return f.read().strip()
    except OSError:
        return ""


def write_active(portal):
    try:
        with open(ACTIVE_FILE, "w") as f:
            f.write(portal)
    except OSError:
        pass


def clear_active():
    try:
        os.remove(ACTIVE_FILE)
    except OSError:
        pass


class GPTray:
    def __init__(self):
        os.makedirs(LOG_DIR, exist_ok=True)
        self.prev_state = None
        self.user_disconnect = False
        self.active_token = 0
        self.notified_fail = False
        self._switch_attempts = 0
        self._connect_deadline = 0.0
        self.auth_proc = None  # running gpauth (SAML in the user's browser)

        if os.path.isdir(ICON_FALLBACK_DIR):
            self.ind = AppIndicator.Indicator.new_with_path(
                "gp-tray", ICON_DISCONNECTED,
                AppIndicator.IndicatorCategory.SYSTEM_SERVICES,
                ICON_FALLBACK_DIR)
        else:
            self.ind = AppIndicator.Indicator.new(
                "gp-tray", ICON_DISCONNECTED,
                AppIndicator.IndicatorCategory.SYSTEM_SERVICES)
        self.ind.set_status(AppIndicator.IndicatorStatus.ACTIVE)
        self.ind.set_title("GlobalProtect")
        self.menu = Gtk.Menu()

        self.status_item = Gtk.MenuItem(label="Status: …")
        self.status_item.set_sensitive(False)

        self.connect_items = []  # (menu item, portal, friendly name)
        for name, portal in PORTALS:
            it = Gtk.MenuItem(label=f"Connect — {name}")
            it.connect("activate", self.on_connect, portal)
            self.connect_items.append((it, portal, name))

        self.disconnect_item = Gtk.MenuItem(label="Disconnect")
        self.disconnect_item.connect("activate", self.on_disconnect)
        log_item = Gtk.MenuItem(label="View connect log")
        log_item.connect("activate", lambda *_: subprocess.Popen(["xdg-open", LOG_FILE]))
        quit_item = Gtk.MenuItem(label="Quit tray")
        quit_item.connect("activate", lambda *_: Gtk.main_quit())

        items = [self.status_item, Gtk.SeparatorMenuItem()]
        items += [it for it, _, _ in self.connect_items]
        items += [self.disconnect_item, Gtk.SeparatorMenuItem(), log_item, quit_item]
        for it in items:
            self.menu.append(it)
        self.menu.show_all()
        self.ind.set_menu(self.menu)
        self.refresh(); GLib.timeout_add_seconds(POLL_SECONDS, self.refresh)

    def _state(self):
        if self.auth_proc is not None and self.auth_proc.poll() is None:
            return "connecting"  # SAML auth in the browser
        return vpn_state(read_active())

    def refresh(self):
        s = self._state()
        active = read_active()
        aname = friendly(active) or active

        if s == "connected":
            self.ind.set_icon_full(ICON_CONNECTED, "Connected")
            self.ind.set_title(f"GlobalProtect — Connected {aname}".strip())
            self.status_item.set_label(f"●  Connected:  {aname or 'GlobalProtect'}")
        elif s == "connecting":
            self.ind.set_icon_full(ICON_CONNECTING, "Connecting")
            self.ind.set_title("GlobalProtect — Connecting")
            self.status_item.set_label(f"…  Connecting:  {aname or '…'}")
        else:
            self.ind.set_icon_full(ICON_DISCONNECTED, "Disconnected")
            self.ind.set_title("GlobalProtect — Disconnected")
            self.status_item.set_label("○  Disconnected")

        for it, portal, name in self.connect_items:
            if s != "disconnected" and portal == active:
                verb = "✓ Connected" if s == "connected" else "…  Connecting"
                it.set_label(f"{verb} — {name}")
                it.set_sensitive(False)
            elif s != "disconnected":
                it.set_label(f"⇄ Switch to — {name}")
                it.set_sensitive(True)
            else:
                it.set_label(f"Connect — {name}")
                it.set_sensitive(True)
        self.disconnect_item.set_sensitive(s != "disconnected")

        if self.prev_state is not None and s != self.prev_state:
            if s == "connected":
                notify("VPN connected", aname or "GlobalProtect")
                self.notified_fail = False
            elif s == "disconnected" and self.prev_state in ("connected", "connecting"):
                if self.user_disconnect:
                    pass
                elif self.prev_state == "connected":
                    notify("VPN disconnected", "The GlobalProtect tunnel dropped.",
                           critical=True, icon="network-error-symbolic")
                elif not self.notified_fail:
                    notify("VPN connection failed",
                           tunnel_failure_detail(active) or log_error_tail(),
                           critical=True, icon="network-error-symbolic")
                    self.notified_fail = True

        if s == "disconnected" and self.user_disconnect:
            clear_active()
            self.user_disconnect = False
        self.prev_state = s
        return True

    def _start_connect(self, portal):
        """Phase 1: SAML auth in the user's session (gpauth, no privileges).
        Phase 2 (_watch_auth): hand the cookie to the root tunnel unit."""
        write_active(portal)
        self.user_disconnect = False
        self.notified_fail = False
        f = open(LOG_FILE, "ab")
        self.auth_proc = subprocess.Popen(
            ["gpauth", "--browser", "default", portal],
            stdout=subprocess.PIPE, stderr=f, start_new_session=True)
        self._connect_deadline = time.monotonic() + CONNECT_TIMEOUT
        self.active_token += 1
        token = self.active_token
        GLib.timeout_add(1500, self._watch_auth, self.auth_proc, token, portal)

    def _watch_auth(self, proc, token, portal):
        if token != self.active_token:
            return False
        rc = proc.poll()
        if rc is None:
            if time.monotonic() > self._connect_deadline:
                proc.terminate()
                self.auth_proc = None
                notify("VPN connect timed out",
                       "Authentication did not complete in time.",
                       critical=True, icon="network-error-symbolic")
                self.notified_fail = True
                clear_active()
                return False
            return True
        self.auth_proc = None
        cookie = proc.stdout.read()
        if rc != 0 or not cookie.strip():
            if not self.user_disconnect and not self.notified_fail:
                notify("VPN connect cancelled",
                       "Authentication was dismissed or failed.",
                       critical=True, icon="dialog-password-symbolic")
                self.notified_fail = True
            clear_active()
            return False
        ok, err = self._start_tunnel(portal, cookie)
        if not ok:
            notify("VPN connection failed", err,
                   critical=True, icon="network-error-symbolic")
            self.notified_fail = True
            clear_active()
            return False
        GLib.timeout_add(1500, self._watch_tunnel, token, portal)
        return False

    def _start_tunnel(self, portal, cookie):
        """Hand the single-use cookie to systemd and start the tunnel unit.
        The polkit rule shipped with gp-tray makes this prompt-free; without
        it, systemctl falls back to a polkit agent prompt."""
        try:
            try:
                os.unlink(COOKIE_FILE)  # stale leftover from an aborted start
            except FileNotFoundError:
                pass
            fd = os.open(COOKIE_FILE, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "wb") as f:
                f.write(cookie)
        except FileNotFoundError:
            return False, ("/run/gp-tray missing — system support not installed "
                           "(systemd-tmpfiles --create gp-tray.conf)")
        except (PermissionError, FileExistsError):
            return False, f"cannot write {COOKIE_FILE} (owned by another user?)"
        try:
            r = subprocess.run(["systemctl", "start", unit_for(portal)],
                               capture_output=True, text=True, timeout=30)
        except subprocess.TimeoutExpired:
            return False, "systemctl start timed out"
        finally:
            try:
                os.unlink(COOKIE_FILE)  # the unit also removes it (ExecStartPost)
            except OSError:
                pass
        if r.returncode != 0:
            return False, (r.stderr.strip() or "systemctl start failed")[:300]
        return True, ""

    def _watch_tunnel(self, token, portal):
        """Wait for the started unit to actually bring the tunnel up."""
        if token != self.active_token:
            return False
        s = vpn_state(portal)
        if s == "connected":
            return False  # refresh() notifies the transition
        if s == "disconnected":
            return False  # unit died; refresh() reports via tunnel_failure_detail
        if time.monotonic() > self._connect_deadline:
            self._teardown()
            notify("VPN connect timed out",
                   "The tunnel did not come up in time; connection reset.",
                   critical=True, icon="network-error-symbolic")
            self.notified_fail = True
            clear_active()
            return False
        return True

    def on_connect(self, _item, portal):
        if self._state() != "disconnected" and read_active() != portal:
            notify("Switching VPN", f"→ {friendly(portal)}")
            self.user_disconnect = True
            self.active_token += 1
            self._teardown()
            write_active(portal)
            self._switch_attempts = 0
            GLib.timeout_add(1000, self._connect_when_down, portal)
            return
        self._start_connect(portal)

    def _connect_when_down(self, portal):
        self._switch_attempts += 1
        if self._state() == "disconnected":
            self.user_disconnect = False
            self._start_connect(portal)
            return False
        if self._switch_attempts > 30:
            notify("VPN switch failed",
                   "Previous session did not disconnect in time.",
                   critical=True, icon="network-error-symbolic")
            return False
        return True

    def _teardown(self):
        """Abort an in-flight auth (our own process) and stop any tunnel
        unit. Both are unprivileged thanks to the polkit rule — no pkexec."""
        if self.auth_proc is not None and self.auth_proc.poll() is None:
            self.auth_proc.terminate()
        self.auth_proc = None
        subprocess.Popen(["systemctl", "stop", TUNNEL_UNIT_GLOB],
                         start_new_session=True)

    def on_disconnect(self, *_):
        self.user_disconnect = True
        self.active_token += 1
        self._teardown()
        GLib.timeout_add_seconds(4, self._verify_down)

    def _verify_down(self):
        if self._state() != "disconnected":
            self._teardown()  # retry once if the first teardown didn't take
        return False


if __name__ == "__main__":
    GPTray(); Gtk.main()
