#!/bin/sh
# Builds the Linux archive.
#
# One directory with the binary, the icon, the menu entry, the installer and the
# documentation, tarred and checksummed under `dist/`. Deliberately nothing else:
# no browser, no Xray, no service file - see docs/release.md for why the program
# ships none of those.
#
#     packaging/linux/package.sh
#     FP_SKIP_BUILD=1 packaging/linux/package.sh   # package what is already built
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

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

echo "dist/$name.tar.gz"
