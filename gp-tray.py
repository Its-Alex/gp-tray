#!/usr/bin/env python3
"""System-tray status indicator for GlobalProtect (gpclient).

A free, dependency-light AppIndicator that lets you connect/disconnect the
GlobalProtect VPN via the open-source `gpclient`, switch between multiple
portals one click at a time, and get desktop notifications on connect
failure, auth cancel, unexpected drop, and successful connect.

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

CONFIG_DIR = os.path.expanduser("~/.config/gp-tray")
CONFIG_FILE = os.path.join(CONFIG_DIR, "portals.conf")
LOG_DIR = os.path.expanduser("~/.cache/gp-tray")
LOG_FILE = os.path.join(LOG_DIR, "connect.log")
ACTIVE_FILE = os.path.join(LOG_DIR, "active_portal")
POLL_SECONDS = 3
CONNECT_TIMEOUT = 120  # seconds; kill a connect that never brings up a tun (stuck SAML auth)
ICON_CONNECTED = "network-vpn-symbolic"
ICON_CONNECTING = "network-vpn-acquiring-symbolic"
ICON_DISCONNECTED = "network-vpn-disconnected-symbolic"


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


PORTALS = load_portals()
NAME_OF = {host: name for name, host in PORTALS}


def friendly(portal):
    return NAME_OF.get(portal, portal) if portal else ""


def _ok(args):
    return subprocess.run(args, capture_output=True).returncode == 0


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
    try:
        out = subprocess.run(["ip", "-br", "link", "show", "type", "tun"],
                             capture_output=True, text=True).stdout
        return any(l.split() and "UP" in l for l in out.splitlines())
    except Exception:
        return False


def vpn_state():
    """gpclient 2.x embeds openconnect (no separate process), so tie state to
    the gpclient/gpservice process plus a live tun device."""
    gp = _ok(["pgrep", "-x", "gpclient"]) or _ok(["pgrep", "-x", "gpservice"])
    tun = _tun_up()
    if gp and tun:  return "connected"
    if gp:          return "connecting"
    if tun:         return "connected"
    return "disconnected"


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

    def refresh(self):
        s = vpn_state()
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
                    notify("VPN connection failed", log_error_tail(),
                           critical=True, icon="network-error-symbolic")
                    self.notified_fail = True

        if s == "disconnected" and self.user_disconnect:
            clear_active()
            self.user_disconnect = False
        self.prev_state = s
        return True

    def _start_connect(self, portal):
        write_active(portal)
        self.user_disconnect = False
        self.notified_fail = False
        f = open(LOG_FILE, "ab")
        proc = subprocess.Popen(
            ["pkexec", "gpclient", "connect", "--browser", "default",
             "--auto-gateway", portal],
            stdout=f, stderr=f, start_new_session=True)
        self._connect_deadline = time.monotonic() + CONNECT_TIMEOUT
        self.active_token += 1
        token = self.active_token
        GLib.timeout_add(1500, self._watch_connect, proc, token)

    def _watch_connect(self, proc, token):
        if token != self.active_token:
            return False
        rc = proc.poll()
        if rc is None:
            if vpn_state() == "connected":
                return False  # tunnel up; nothing more to watch
            if time.monotonic() > self._connect_deadline:
                self._kill_connect()
                notify("VPN connect timed out",
                       "Authentication did not complete in time; connection reset.",
                       critical=True, icon="network-error-symbolic")
                self.notified_fail = True
                clear_active()
                return False
            return True
        if rc != 0 and not self.user_disconnect and not self.notified_fail:
            if rc in (126, 127):
                notify("VPN connect cancelled",
                       "Authentication was dismissed or failed.",
                       critical=True, icon="dialog-password-symbolic")
            else:
                notify("VPN connection failed", log_error_tail(),
                       critical=True, icon="network-error-symbolic")
            self.notified_fail = True
            clear_active()
        return False

    def on_connect(self, _item, portal):
        if vpn_state() != "disconnected" and read_active() != portal:
            notify("Switching VPN", f"→ {friendly(portal)}")
            self.user_disconnect = True
            self.active_token += 1
            subprocess.Popen(["pkexec", "gpclient", "disconnect"],
                             start_new_session=True)
            write_active(portal)
            self._switch_attempts = 0
            GLib.timeout_add(1000, self._connect_when_down, portal)
            return
        self._start_connect(portal)

    def _connect_when_down(self, portal):
        self._switch_attempts += 1
        if vpn_state() == "disconnected":
            self.user_disconnect = False
            self._start_connect(portal)
            return False
        if self._switch_attempts > 30:
            notify("VPN switch failed",
                   "Previous session did not disconnect in time.",
                   critical=True, icon="network-error-symbolic")
            return False
        return True

    def _kill_connect(self):
        # Force-clear a stuck connect/auth. gpclient runs as root (needs pkexec);
        # gpauth is the SAML browser helper running as us. `disconnect` handles an
        # established tunnel; the pkills handle a connect wedged mid-auth (which
        # `disconnect` alone is a no-op against). Single pkexec = one polkit prompt.
        subprocess.Popen(
            ["pkexec", "sh", "-c",
             "gpclient disconnect 2>/dev/null; pkill -x gpclient; pkill -x gpauth"],
            start_new_session=True)

    def on_disconnect(self, *_):
        self.user_disconnect = True
        self.active_token += 1
        self._kill_connect()
        GLib.timeout_add_seconds(4, self._verify_down)

    def _verify_down(self):
        if vpn_state() != "disconnected":
            self._kill_connect()  # retry once if the first teardown didn't take
        return False


if __name__ == "__main__":
    GPTray(); Gtk.main()
