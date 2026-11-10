#!/bin/sh
# Makes the retifs Photos package for Haiku (an hpkg) from the app folder
# build.sh made: the app with its library in apps/retifs_photos, and an
# entry in the Deskbar's Applications menu. retifs itself (the daemon) comes
# from the retifs package.
#
#     haiku/build.sh && haiku/package.sh
#     pkgman install haiku/build/retifs_photos-<version>-x86_64.hpkg
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
APP=$HERE/build/Photos
STAGE=$HERE/build/package
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)-4
ARCH=x86_64

for f in Photos lib/libretifs_photos.so; do
	[ -f "$APP/$f" ] || { echo "missing $APP/$f: build it first" >&2; exit 1; }
done

rm -rf "${STAGE:?}"
mkdir -p "$STAGE/apps" "$STAGE/data/deskbar/menu/Applications"
cp -a "$APP" "$STAGE/apps/retifs_photos"
ln -s ../../../../apps/retifs_photos/Photos \
	"$STAGE/data/deskbar/menu/Applications/retifs Photos"

cat > "$STAGE/.PackageInfo" <<EOF
name			retifs_photos
version			$VERSION
architecture	$ARCH
summary			"Photos on retifs"
description		"Photos on retifs: every picture in your libraries by month, as the \
thumbnails the extractor keeps with each; favourites, albums and people (your \
contacts), and a viewer to tag people and file photos in albums. A working \
prototype."
packager		"retifs <https://github.com/vooteleaer/retifs>"
vendor			"retifs"
licenses {
	"MIT"
}
copyrights {
	"2026 retifs contributors"
}
provides {
	retifs_photos = $VERSION
	app:retifs_photos = $VERSION
}
requires {
	haiku
	retifs
	lib:libgcc_s
	lib:libstdc++
}
urls {
	"https://github.com/vooteleaer/retifs"
}
EOF

PKG=$HERE/build/retifs_photos-$VERSION-$ARCH.hpkg
rm -f "$PKG"
package create -C "$STAGE" "$PKG"
echo "== $PKG"
package list "$PKG" | tail -12
