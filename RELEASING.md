# Releasing gp-tray

Versions are git tags (`vX.Y.Z`) — there is no version string in the code.
Each release is a tag on `main` plus an update of the AUR package that
points at its GitHub tarball. No GitHub release is created; the tag alone
is what the AUR `source=` URL needs.

The canonical `PKGBUILD` lives in this repo at
[`packaging/arch/PKGBUILD`](packaging/arch/PKGBUILD); the AUR repo
(`ssh://aur@aur.archlinux.org/gp-tray.git`, web:
<https://aur.archlinux.org/packages/gp-tray>) is only the publish target.
`.SRCINFO` is generated from it — never tracked here.

## Normal flow (automated)

```bash
scripts/release.sh tag X.Y.Z
```

This checks the tree is clean and in sync, byte-compiles the script,
verifies the `make install` layout, then creates and pushes the annotated
tag. The [`Publish to AUR` workflow](.github/workflows/aur-publish.yml)
does the rest on the tag push:

1. pins `pkgver`, `pkgrel=1` and the tag tarball's sha256 into
   `packaging/arch/PKGBUILD` (`scripts/release.sh prepare`),
2. commits the pinned PKGBUILD back to `main`,
3. builds the package in an Arch container (`test: true`) and pushes
   `PKGBUILD` + regenerated `.SRCINFO` to the AUR via
   [KSXGitHub/github-actions-deploy-aur](https://github.com/KSXGitHub/github-actions-deploy-aur).

It needs one repository secret: `AUR_SSH_PRIVATE_KEY` — an SSH private key
whose public half is registered on the AUR account
([account settings](https://aur.archlinux.org/account/ItsAlex/edit)).

## Manual fallback

If CI is unavailable (requires an Arch machine with `base-devel`):

```bash
scripts/release.sh tag X.Y.Z        # if not already tagged
scripts/release.sh publish X.Y.Z    # prepare + commit + makepkg verify + push to AUR
```

For a packaging-only fix on an already-released version, bump the release
number instead of the version: `scripts/release.sh publish X.Y.Z 2`.

## Rules

- **Never move a published tag.** GitHub's
  `archive/refs/tags/vX.Y.Z.tar.gz` is generated from the tagged commit
  (the commit id is embedded in the tarball's `pax_global_header`), so
  re-tagging changes the checksum and breaks everyone's cached sources.
  Ship a new patch version instead.
- The pinned-PKGBUILD commit lands on `main` *after* the tag, so the
  PKGBUILD inside a release tarball always lags one release. That's
  expected: the tarball copy is informational, the AUR copy is what's
  consumed.
- `.SRCINFO` must be regenerated and committed with every AUR push
  (the script and the action both do this).

## Checklist (what the tooling verifies)

- [ ] `make install DESTDIR=… PREFIX=/usr` lays out binary, unit,
      `.desktop`, and all four icons (`release.sh tag`)
- [ ] tarball sha256 pinned from the actual tag archive (`release.sh prepare`)
- [ ] package builds from the pinned PKGBUILD (`test: true` in CI, or
      `makepkg -f` in `release.sh publish`)
- [ ] in-tree and AUR PKGBUILD identical (the AUR copy is `cp`'d from the tree)
