#!/bin/sh
# Installs this unpacked archive for the current user.
#
# `~/.local` is the XDG place for a user's own programs, and the one that needs
# no root: a browser-profile manager has no business in /usr, and on a machine
# where the user cannot write to /usr this is the only install that works. The
# binary goes on the PATH, the icon where the icon theme looks, and the entry
# where the application menu looks.
#
# Run it from the directory the archive was unpacked into:
#
#     ./install.sh            # into ~/.local
#     PREFIX=/opt/fp ./install.sh
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
data=${XDG_DATA_HOME:-$HOME/.local/share}
bindir="$prefix/bin"
appsdir="$data/applications"
icondir="$data/icons/hicolor/512x512/apps"

install -d "$bindir" "$appsdir" "$icondir"
install -m 0755 "$here/fingerprint-browser" "$bindir/fingerprint-browser"
install -m 0644 "$here/assets/icon.png" "$icondir/fingerprint-browser.png"

# The entry names the program, and the menu does not search a user's PATH for
# it: the path is written in so the launcher works whatever PATH happens to be.
sed "s|^Exec=.*|Exec=$bindir/fingerprint-browser|" \
    "$here/fingerprint-browser.desktop" > "$appsdir/fingerprint-browser.desktop"
chmod 0644 "$appsdir/fingerprint-browser.desktop"

# Both caches are optional: a desktop that has the tools picks the new files up
# immediately, and one that does not will on its next refresh.
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$appsdir" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f "$data/icons/hicolor" || true
fi

echo "Installed: $bindir/fingerprint-browser"
echo "Menu entry: $appsdir/fingerprint-browser.desktop"
case ":$PATH:" in
    *":$bindir:"*) ;;
    *) echo "Note: $bindir is not on your PATH, so 'fingerprint-browser' will not" \
            "run from a shell until you add it. The menu entry does not need it." ;;
esac
