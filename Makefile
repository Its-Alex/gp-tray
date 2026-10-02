PREFIX  ?= /usr/local
BINDIR  ?= $(PREFIX)/bin
# systemd user units: /usr/lib/systemd/user for packages, honours PREFIX otherwise
UNITDIR ?= $(PREFIX)/lib/systemd/user
APPSDIR ?= $(PREFIX)/share/applications
ICONDIR ?= $(PREFIX)/share/icons/hicolor/scalable
DOCDIR  ?= $(PREFIX)/share/doc/gp-tray

# Per-user install locations (make install-user)
USER_BINDIR  := $(HOME)/.local/bin
USER_UNITDIR := $(HOME)/.config/systemd/user
USER_APPSDIR := $(HOME)/.local/share/applications
USER_ICONDIR := $(HOME)/.local/share/icons/hicolor/scalable

STATUS_ICONS := icons/gp-tray-connected.svg \
                icons/gp-tray-connecting.svg \
                icons/gp-tray-disconnected.svg

.PHONY: all install uninstall install-user uninstall-user

all:
	@echo "Targets: install (system, honours DESTDIR/PREFIX), install-user (~/.local)"

gp-tray.service: systemd/gp-tray.service.in
	sed 's|@BINDIR@|$(BINDIR)|' $< > $@

gp-tray.service.user: systemd/gp-tray.service.in
	sed 's|@BINDIR@|$(USER_BINDIR)|' $< > $@

install: gp-tray.service
	install -Dm755 gp-tray.py $(DESTDIR)$(BINDIR)/gp-tray
	install -Dm644 gp-tray.service $(DESTDIR)$(UNITDIR)/gp-tray.service
	install -Dm644 gp-tray.desktop $(DESTDIR)$(APPSDIR)/gp-tray.desktop
	install -Dm644 -t $(DESTDIR)$(ICONDIR)/status $(STATUS_ICONS)
	install -Dm644 icons/gp-tray.svg $(DESTDIR)$(ICONDIR)/apps/gp-tray.svg
	install -Dm644 portals.conf.example $(DESTDIR)$(DOCDIR)/portals.conf.example

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/gp-tray \
	      $(DESTDIR)$(UNITDIR)/gp-tray.service \
	      $(DESTDIR)$(APPSDIR)/gp-tray.desktop \
	      $(DESTDIR)$(ICONDIR)/status/gp-tray-connected.svg \
	      $(DESTDIR)$(ICONDIR)/status/gp-tray-connecting.svg \
	      $(DESTDIR)$(ICONDIR)/status/gp-tray-disconnected.svg \
	      $(DESTDIR)$(ICONDIR)/apps/gp-tray.svg \
	      $(DESTDIR)$(DOCDIR)/portals.conf.example

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
	rm -f gp-tray.service gp-tray.service.user
