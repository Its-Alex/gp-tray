# Releasing gp-tray

Versions are git tags (`vX.Y.Z`) — there is no version string in the code.
Each release is a tag on `main` plus an update of the AUR package that
points at its GitHub tarball. No GitHub release is created; the tag alone
is what the AUR `source=` URL needs.

## 1. Tag the release

```bash
git tag -a vX.Y.Z -m "gp-tray X.Y.Z — <summary>"
git push origin main vX.Y.Z
```

Never move a published tag: GitHub's `archive/refs/tags/vX.Y.Z.tar.gz` is
generated from the tagged commit (the commit id is embedded in the
tarball's `pax_global_header`), so re-tagging changes the checksum and
breaks everyone's cached sources. Ship a new patch version instead.

## 2. Update the AUR package

The AUR repo is `ssh://aur@aur.archlinux.org/gp-tray.git`
(web: <https://aur.archlinux.org/packages/gp-tray>).

```bash
git clone ssh://aur@aur.archlinux.org/gp-tray.git && cd gp-tray

# New upstream version:
curl -sL https://github.com/Its-Alex/gp-tray/archive/refs/tags/vX.Y.Z.tar.gz | sha256sum
# → set pkgver=X.Y.Z, pkgrel=1, sha256sums=('<new hash>') in PKGBUILD
# (packaging-only change on the same version: bump pkgrel instead)

makepkg --printsrcinfo > .SRCINFO   # must be committed with every PKGBUILD change
makepkg -f                           # verify checksum + build
namcap gp-tray-*.pkg.tar.zst         # optional lint, if installed

git add PKGBUILD .SRCINFO
git commit -m "X.Y.Z-1: <summary>"
git push origin HEAD:master          # AUR only accepts the master branch
```

## Checklist

- [ ] `make install DESTDIR=$(mktemp -d) PREFIX=/usr` lays out all files
- [ ] Tag pushed; tarball downloads and its sha256 matches the PKGBUILD
- [ ] `.SRCINFO` regenerated
- [ ] `makepkg -f` builds and the package contains `usr/bin/gp-tray`,
      the systemd unit, the `.desktop` file, and the icons
