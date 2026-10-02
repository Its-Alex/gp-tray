PREFIX  ?= /usr/local
BINDIR  ?= $(PREFIX)/bin
# systemd user units: /usr/lib/systemd/user for packages, honours PREFIX otherwise
UNITDIR ?= $(PREFIX)/lib/systemd/user
APPSDIR ?= $(PREFIX)/share/applications
DOCDIR  ?= $(PREFIX)/share/doc/gp-tray

# Per-user install locations (make install-user)
USER_BINDIR  := $(HOME)/.local/bin
USER_UNITDIR := $(HOME)/.config/systemd/user
USER_APPSDIR := $(HOME)/.local/share/applications

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
	install -Dm644 portals.conf.example $(DESTDIR)$(DOCDIR)/portals.conf.example

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/gp-tray \
	      $(DESTDIR)$(UNITDIR)/gp-tray.service \
	      $(DESTDIR)$(APPSDIR)/gp-tray.desktop \
	      $(DESTDIR)$(DOCDIR)/portals.conf.example

install-user: gp-tray.service.user
	install -Dm755 gp-tray.py $(USER_BINDIR)/gp-tray
	install -Dm644 gp-tray.service.user $(USER_UNITDIR)/gp-tray.service
	install -Dm644 gp-tray.desktop $(USER_APPSDIR)/gp-tray.desktop
	@echo
	@echo "Installed. Enable and start with:"
	@echo "  systemctl --user daemon-reload"
	@echo "  systemctl --user enable --now gp-tray.service"

uninstall-user:
	-systemctl --user disable --now gp-tray.service 2>/dev/null
	rm -f $(USER_BINDIR)/gp-tray \
	      $(USER_UNITDIR)/gp-tray.service \
	      $(USER_APPSDIR)/gp-tray.desktop

clean:
	rm -f gp-tray.service gp-tray.service.user
