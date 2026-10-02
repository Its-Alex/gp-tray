#!/usr/bin/env bash
# release.sh — tag a gp-tray release and publish it to the AUR.
#
#   scripts/release.sh tag X.Y.Z             preflight checks, annotated tag, push
#                                            (CI then publishes to the AUR)
#   scripts/release.sh prepare X.Y.Z [rel]   pin pkgver/pkgrel/sha256 in
#                                            packaging/arch/PKGBUILD (used by CI)
#   scripts/release.sh publish X.Y.Z [rel]   prepare + commit + push to the AUR;
#                                            local fallback when CI is unavailable
set -euo pipefail

REPO_URL="https://github.com/Its-Alex/gp-tray"
AUR_REMOTE="ssh://aur@aur.archlinux.org/gp-tray.git"
PKGBUILD="packaging/arch/PKGBUILD"

die() { echo "release.sh: $*" >&2; exit 1; }

usage() {
    # print the header comment block as help text
    sed -n '2,${/^#/!q;s/^# \{0,1\}//p}' "$0"
    exit "${1:-0}"
}

cd "$(git rev-parse --show-toplevel)"

cmd="${1:-}"
version="${2:-}"
pkgrel="${3:-1}"

case "$cmd" in
    ""|-h|--help|help) usage ;;
    tag|prepare|publish) ;;
    *) die "unknown command '$cmd' (expected tag|prepare|publish)" ;;
esac
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must be X.Y.Z (got '${version:-}')"
[[ "$pkgrel" =~ ^[0-9]+$ ]] || die "pkgrel must be a number (got '$pkgrel')"

tarball_sha256() {
    # GitHub generates tag archives on demand; retry while the tag propagates.
    local url="$REPO_URL/archive/refs/tags/v$version.tar.gz" tmp
    tmp=$(mktemp)
    trap 'rm -f "$tmp"' RETURN
    for _ in 1 2 3 4 5; do
        if curl -fsSL "$url" -o "$tmp"; then
            sha256sum "$tmp" | cut -d' ' -f1
            return
        fi
        sleep 5
    done
    die "could not download $url — is tag v$version pushed?"
}

check_install_layout() {
    local stage
    stage=$(mktemp -d)
    make install DESTDIR="$stage" PREFIX=/usr >/dev/null
    local f missing=0
    for f in usr/bin/gp-tray \
             usr/lib/systemd/user/gp-tray.service \
             usr/lib/systemd/system/gp-tray-tunnel@.service \
             usr/lib/gp-tray/gp-tray-tunnel \
             usr/share/polkit-1/rules.d/50-gp-tray.rules \
             usr/lib/tmpfiles.d/gp-tray.conf \
             usr/share/applications/gp-tray.desktop \
             usr/share/icons/hicolor/scalable/status/gp-tray-connected.svg \
             usr/share/icons/hicolor/scalable/status/gp-tray-connecting.svg \
             usr/share/icons/hicolor/scalable/status/gp-tray-disconnected.svg \
             usr/share/icons/hicolor/scalable/apps/gp-tray.svg; do
        [ -f "$stage/$f" ] || { echo "missing from install: $f" >&2; missing=1; }
    done
    make clean >/dev/null
    rm -rf "$stage"
    [ "$missing" -eq 0 ] || die "make install layout check failed"
}

cmd_tag() {
    [ -z "$(git status --porcelain)" ] || die "working tree not clean"
    [ "$(git branch --show-current)" = "main" ] || die "not on main"
    git fetch origin main
    [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "main not in sync with origin"
    git rev-parse -q --verify "refs/tags/v$version" >/dev/null && die "tag v$version already exists"
    python3 -m py_compile gp-tray.py
    check_install_layout
    git tag -a "v$version" -m "gp-tray $version"
    git push origin main "v$version"
    echo "Tagged v$version. The 'Publish to AUR' workflow takes it from here;"
    echo "if CI is unavailable, run: scripts/release.sh publish $version"
}

cmd_prepare() {
    local sha
    sha=$(tarball_sha256)
    sed -i \
        -e "s/^pkgver=.*/pkgver=$version/" \
        -e "s/^pkgrel=.*/pkgrel=$pkgrel/" \
        -e "s/^sha256sums=.*/sha256sums=('$sha')/" \
        "$PKGBUILD"
    echo "$PKGBUILD pinned to $version-$pkgrel ($sha)"
}

cmd_publish() {
    cmd_prepare
    if ! git diff --quiet -- "$PKGBUILD"; then
        git add "$PKGBUILD"
        git commit -m "packaging: $version-$pkgrel"
        git push origin main
    fi
    local workdir srcroot
    srcroot=$(pwd)
    workdir=$(mktemp -d)
    git clone "$AUR_REMOTE" "$workdir/aur"
    cp "$PKGBUILD" "$workdir/aur/PKGBUILD"
    (
        cd "$workdir/aur"
        makepkg --printsrcinfo > .SRCINFO
        makepkg -f          # verify checksum + build before publishing
        git add PKGBUILD .SRCINFO
        git -c user.name="$(git -C "$srcroot" config user.name)" \
            -c user.email="$(git -C "$srcroot" config user.email)" \
            commit -m "$version-$pkgrel: upstream release v$version"
        git push origin HEAD:master
    )
    rm -rf "$workdir"
    echo "Published gp-tray $version-$pkgrel to the AUR."
}

case "$cmd" in
    tag)     cmd_tag ;;
    prepare) cmd_prepare ;;
    publish) cmd_publish ;;
esac
