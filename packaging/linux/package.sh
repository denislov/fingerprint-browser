#!/bin/sh
# Builds the Linux artifacts: the archive, and the Debian package.
#
# Both are built from one staged tree, so the two cannot disagree about which
# files ship - the difference between them is only where the files go, which is
# the whole of what a package manager is for. Deliberately nothing else: no
# browser, no Xray, no service file - see docs/release.md for why the program
# ships none of those.
#
#     packaging/linux/package.sh
#     FP_SKIP_BUILD=1 packaging/linux/package.sh   # package what is already built
#     FP_SKIP_DEB=1 packaging/linux/package.sh     # the archive alone, no dpkg
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

# The Debian package is built with Debian's own tools, which are also what make
# its dependency list askable - dpkg-deb writes the package, dpkg says which
# installed package carries a library, and objdump says which libraries the
# binary asks for. Checked before the build rather than after it: a machine that
# cannot make the package should say so in the first second, not in the fifth
# minute.
if [ "${FP_SKIP_DEB:-}" != "1" ]; then
    for tool in dpkg-deb dpkg objdump; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            echo "$tool is not installed, and the Debian package is built with it." >&2
            echo "Install dpkg and binutils (Debian, Ubuntu: apt install dpkg" >&2
            echo "binutils), or set FP_SKIP_DEB=1 to build the archive alone." >&2
            exit 1
        fi
    done
fi

if [ "${FP_SKIP_BUILD:-}" != "1" ]; then
    cargo build --release -p app
fi

binary=target/release/fingerprint-browser
if [ ! -x "$binary" ]; then
    echo "$binary is not there: build it first, or unset FP_SKIP_BUILD" >&2
    exit 1
fi

version=$("$binary" --version | awk '{print $3}')
manifest=$(scripts/version.sh)
if [ "$version" != "$manifest" ]; then
    # The manifest is the source of truth and the binary is what ships; when they
    # disagree, one of them is stale and the artifact would be named for a
    # version it is not.
    echo "the binary reports $version and Cargo.toml says $manifest" >&2
    exit 1
fi

arch=$(uname -m)
name="fingerprint-browser-$version-linux-$arch"
stage="dist/$name"

rm -rf "$stage"
install -d "$stage/docs" "$stage/assets"
install -m 0755 "$binary" "$stage/fingerprint-browser"
install -m 0755 packaging/linux/install.sh "$stage/install.sh"
install -m 0644 packaging/linux/fingerprint-browser.desktop "$stage/"
install -m 0644 assets/icon.png assets/icon.svg "$stage/assets/"
install -m 0644 README.md CHANGELOG.md LICENSE "$stage/"
install -m 0644 docs/*.md "$stage/docs/"

tar -czf "dist/$name.tar.gz" -C dist "$name"
# One line, the hash first: what `sha256sum -c` reads.
(cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")

if [ "${FP_SKIP_DEB:-}" = "1" ]; then
    echo "dist/$name.tar.gz"
    echo "skipped: the Debian package, because FP_SKIP_DEB=1" >&2
    exit 0
fi

# ---- the Debian package ----
#
# The same files as the archive, laid out where a Debian system looks for them:
# the binary in /usr/bin, the entry in /usr/share/applications, the icon in the
# icon theme's 512x512 directory, and the documentation - including the licence
# under policy's name for it - in /usr/share/doc/fingerprint-browser.
#
# Nothing here runs as root and nothing here installs anything: the tree is
# written into place by dpkg, which is what `--root-owner-group` below is for -
# a dpkg 1.19 and newer option, which is Ubuntu 20.04 and Debian 10 onwards.
case "$arch" in
    x86_64) deb_arch=amd64 ;;
    aarch64) deb_arch=arm64 ;;
    armv7l) deb_arch=armhf ;;
    i386 | i686) deb_arch=i386 ;;
    ppc64le) deb_arch=ppc64el ;;
    riscv64) deb_arch=riscv64 ;;
    *)
        echo "no Debian name for $arch, so a package built here would name an" >&2
        echo "architecture dpkg does not know" >&2
        exit 1
        ;;
esac

# Debian requires a name and an address. The project's own, rather than whoever
# happens to run the script: an artifact built twice has to be the same artifact,
# and the package's maintainer is not a fact about the build machine.
deb_maintainer=${FP_DEB_MAINTAINER:-"denislov <2864326614@qq.com>"}
deb_package="fingerprint-browser_${version}_${deb_arch}"
deb_dir="dist/$deb_package"
deb_doc="$deb_dir/usr/share/doc/fingerprint-browser"

rm -rf "$deb_dir"
install -d "$deb_dir/DEBIAN" \
    "$deb_dir/usr/bin" \
    "$deb_dir/usr/share/applications" \
    "$deb_dir/usr/share/icons/hicolor/512x512/apps" \
    "$deb_dir/usr/share/icons/hicolor/scalable/apps" \
    "$deb_doc"

install -m 0755 "$binary" "$deb_dir/usr/bin/fingerprint-browser"
install -m 0644 packaging/linux/fingerprint-browser.desktop \
    "$deb_dir/usr/share/applications/"
install -m 0644 assets/icon.png \
    "$deb_dir/usr/share/icons/hicolor/512x512/apps/fingerprint-browser.png"
install -m 0644 assets/icon.svg \
    "$deb_dir/usr/share/icons/hicolor/scalable/apps/fingerprint-browser.svg"
install -m 0644 README.md "$deb_doc/"
# Policy's name for the licence of the package, which is not the repository's
# name for the same file.
install -m 0644 LICENSE "$deb_doc/copyright"
install -m 0644 docs/*.md "$deb_doc/"
# Policy wants the changelog compressed and named `changelog.gz`. The markdown is
# the same text and is not installed beside it: one copy of the changelog in the
# package, which is also the copy `apt changelog` reads.
gzip -9nc CHANGELOG.md > "$deb_doc/changelog.gz"
chmod 0644 "$deb_doc/changelog.gz"

# The menu entry and the icon are caches, and both tools are optional: a desktop
# that has them picks the files up now, and one that does not will at its next
# refresh. Neither is allowed to fail the install.
for hook in postinst postrm; do
    cat > "$deb_dir/DEBIAN/$hook" <<'HOOK'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
HOOK
    chmod 0755 "$deb_dir/DEBIAN/$hook"
done

# What the binary asks the loader for, as the packages that carry it. Asked of
# dpkg rather than written down: a list here would be a second copy of what the
# linker already knows, wrong the first time a library moves between packages -
# which is a thing Debian does between releases.
deb_depends=""
for soname in $(objdump -p "$binary" | awk '/NEEDED/ {print $2}' | sort -u); do
    package=$(dpkg -S "$soname" 2>/dev/null | head -1 | cut -d: -f1)
    if [ -z "$package" ]; then
        echo "no installed package provides $soname, so the package cannot name" >&2
        echo "it as a dependency; the archive was still built" >&2
        exit 1
    fi
    case ",$deb_depends," in
        *",$package,"*) ;;
        *) deb_depends="${deb_depends:+$deb_depends,}$package" ;;
    esac
done
deb_depends=$(printf '%s\n' "$deb_depends" | tr ',' '\n' | sort -u | paste -sd, - |
    sed 's/,/, /g')

cat > "$deb_dir/DEBIAN/control" <<EOF
Package: fingerprint-browser
Version: $version
Architecture: $deb_arch
Maintainer: $deb_maintainer
Section: net
Priority: optional
Depends: $deb_depends
Installed-Size: $(du -k -s "$deb_dir/usr" | cut -f1)
Description: browser profiles for fingerprint-chromium
 Each profile owns its fingerprint seed, its data directory and its browser
 process, and each one runs its own fingerprint-chromium behind its own proxy.
 This window manages the profiles, the proxies and the browser cores; it never
 fetches a browser or a proxy for you.
EOF

dpkg-deb --root-owner-group --build "$deb_dir" "dist/$deb_package.deb" > /dev/null
(cd dist && sha256sum "$deb_package.deb" > "$deb_package.deb.sha256")

echo "dist/$name.tar.gz"
echo "dist/$deb_package.deb"
