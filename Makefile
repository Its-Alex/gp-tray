PREFIX  ?= /usr/local
BINDIR  ?= $(PREFIX)/bin
# systemd user units: /usr/lib/systemd/user for packages, honours PREFIX otherwise
UNITDIR    ?= $(PREFIX)/lib/systemd/user
SYSUNITDIR ?= $(PREFIX)/lib/systemd/system
HELPERDIR  ?= $(PREFIX)/lib/gp-tray
APPSDIR ?= $(PREFIX)/share/applications
ICONDIR ?= $(PREFIX)/share/icons/hicolor/scalable
DOCDIR  ?= $(PREFIX)/share/doc/gp-tray
# polkit and systemd-tmpfiles do not search under /usr/local, so non-/usr
# prefixes fall back to the /etc admin directories
ifeq ($(PREFIX),/usr)
POLKITDIR   ?= /usr/share/polkit-1/rules.d
TMPFILESDIR ?= /usr/lib/tmpfiles.d
else
POLKITDIR   ?= /etc/polkit-1/rules.d
TMPFILESDIR ?= /etc/tmpfiles.d
endif

# Per-user install locations (make install-user)
USER_BINDIR  := $(HOME)/.local/bin
USER_UNITDIR := $(HOME)/.config/systemd/user
USER_APPSDIR := $(HOME)/.local/share/applications
USER_ICONDIR := $(HOME)/.local/share/icons/hicolor/scalable

STATUS_ICONS := icons/gp-tray-connected.svg \
                icons/gp-tray-connecting.svg \
                icons/gp-tray-disconnected.svg

.PHONY: all install uninstall install-user uninstall-user \
        install-privileged uninstall-privileged clean

all:
	@echo "Targets:"
	@echo "  install             system-wide (honours DESTDIR/PREFIX; for packaging)"
	@echo "  install-user        tray for this user (~/.local); then run, as root:"
	@echo "  install-privileged  tunnel unit + helper + polkit rule + tmpfiles"

gp-tray.service: systemd/gp-tray.service.in
	sed 's|@BINDIR@|$(BINDIR)|' $< > $@

gp-tray.service.user: systemd/gp-tray.service.in
	sed 's|@BINDIR@|$(USER_BINDIR)|' $< > $@

gp-tray-tunnel@.service: systemd/gp-tray-tunnel@.service.in
	sed 's|@HELPERDIR@|$(HELPERDIR)|' $< > $@

install: gp-tray.service gp-tray-tunnel@.service
	install -Dm755 gp-tray.py $(DESTDIR)$(BINDIR)/gp-tray
	install -Dm644 gp-tray.service $(DESTDIR)$(UNITDIR)/gp-tray.service
	install -Dm755 libexec/gp-tray-tunnel $(DESTDIR)$(HELPERDIR)/gp-tray-tunnel
	install -Dm644 gp-tray-tunnel@.service $(DESTDIR)$(SYSUNITDIR)/gp-tray-tunnel@.service
	install -Dm644 polkit/50-gp-tray.rules $(DESTDIR)$(POLKITDIR)/50-gp-tray.rules
	install -Dm644 tmpfiles.d/gp-tray.conf $(DESTDIR)$(TMPFILESDIR)/gp-tray.conf
	install -Dm644 gp-tray.desktop $(DESTDIR)$(APPSDIR)/gp-tray.desktop
	install -Dm644 -t $(DESTDIR)$(ICONDIR)/status $(STATUS_ICONS)
	install -Dm644 icons/gp-tray.svg $(DESTDIR)$(ICONDIR)/apps/gp-tray.svg
	install -Dm644 portals.conf.example $(DESTDIR)$(DOCDIR)/portals.conf.example

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/gp-tray \
	      $(DESTDIR)$(UNITDIR)/gp-tray.service \
	      $(DESTDIR)$(HELPERDIR)/gp-tray-tunnel \
	      $(DESTDIR)$(SYSUNITDIR)/gp-tray-tunnel@.service \
	      $(DESTDIR)$(POLKITDIR)/50-gp-tray.rules \
	      $(DESTDIR)$(TMPFILESDIR)/gp-tray.conf \
	      $(DESTDIR)$(APPSDIR)/gp-tray.desktop \
	      $(DESTDIR)$(ICONDIR)/status/gp-tray-connected.svg \
	      $(DESTDIR)$(ICONDIR)/status/gp-tray-connecting.svg \
	      $(DESTDIR)$(ICONDIR)/status/gp-tray-disconnected.svg \
	      $(DESTDIR)$(ICONDIR)/apps/gp-tray.svg \
	      $(DESTDIR)$(DOCDIR)/portals.conf.example

# System-side pieces needed by a per-user install (run as root)
install-privileged: gp-tray-tunnel@.service
	install -Dm755 libexec/gp-tray-tunnel $(DESTDIR)$(HELPERDIR)/gp-tray-tunnel
	install -Dm644 gp-tray-tunnel@.service $(DESTDIR)$(SYSUNITDIR)/gp-tray-tunnel@.service
	install -Dm644 polkit/50-gp-tray.rules $(DESTDIR)$(POLKITDIR)/50-gp-tray.rules
	install -Dm644 tmpfiles.d/gp-tray.conf $(DESTDIR)$(TMPFILESDIR)/gp-tray.conf
	@echo
	@echo "Installed. Activate with:"
	@echo "  systemd-tmpfiles --create gp-tray.conf"
	@echo "  systemctl daemon-reload"

uninstall-privileged:
	rm -f $(DESTDIR)$(HELPERDIR)/gp-tray-tunnel \
	      $(DESTDIR)$(SYSUNITDIR)/gp-tray-tunnel@.service \
	      $(DESTDIR)$(POLKITDIR)/50-gp-tray.rules \
	      $(DESTDIR)$(TMPFILESDIR)/gp-tray.conf

install-user: gp-tray.service.user
	install -Dm755 gp-tray.py $(USER_BINDIR)/gp-tray
	install -Dm644 gp-tray.service.user $(USER_UNITDIR)/gp-tray.service
	install -Dm644 gp-tray.desktop $(USER_APPSDIR)/gp-tray.desktop
	install -Dm644 -t $(USER_ICONDIR)/status $(STATUS_ICONS)
	install -Dm644 icons/gp-tray.svg $(USER_ICONDIR)/apps/gp-tray.svg
	@echo
	@echo "Installed. Enable and start with:"
	@echo "  systemctl --user daemon-reload"
	@echo "  systemctl --user enable --now gp-tray.service"
	@echo
	@echo "The privileged tunnel parts are system-wide; if not yet present, run:"
	@echo "  sudo make install-privileged"

uninstall-user:
	-systemctl --user disable --now gp-tray.service 2>/dev/null
	rm -f $(USER_BINDIR)/gp-tray \
	      $(USER_UNITDIR)/gp-tray.service \
	      $(USER_APPSDIR)/gp-tray.desktop \
	      $(USER_ICONDIR)/status/gp-tray-connected.svg \
	      $(USER_ICONDIR)/status/gp-tray-connecting.svg \
	      $(USER_ICONDIR)/status/gp-tray-disconnected.svg \
	      $(USER_ICONDIR)/apps/gp-tray.svg

clean:
	rm -f gp-tray.service gp-tray.service.user gp-tray-tunnel@.service
