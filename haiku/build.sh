#!/bin/sh
# Builds retifs Photos for Haiku on Haiku: the core as a library
# (haiku/capi), the BeAPI app over it, as an app folder
# (haiku/build/Photos: the app, lib/libretifs_photos.so).
#
#     haiku/build.sh
#
# Needs: pkgman install rust_bin gcc haiku_devel; retifs next to retifs-photos.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
OUT=$HERE/build/Photos
cd "$ROOT"
cargo build --release --ignore-rust-version -p retifs-photos-capi
mkdir -p "$OUT/lib"
# A new file, not the old one overwritten: a running app has it mapped.
rm -f "${OUT:?}/lib/libretifs_photos.so"
cp target/release/libretifs_photos.so "$OUT/lib/"
g++ -O2 -std=c++17 -Wall -o "$OUT/Photos" "$HERE/app/PhotosApp.cpp" \
	-I "$HERE/capi" -I /boot/system/develop/headers/private/shared \
	-L "$OUT/lib" -lretifs_photos -lbe -lshared -ltranslation \
	-Wl,-rpath,'$ORIGIN/lib'
rc -o "$HERE/build/photos.rsrc" "$HERE/app/photos.rdef"
xres -o "$OUT/Photos" "$HERE/build/photos.rsrc"
mimeset -f "$OUT/Photos"
echo "== $OUT"
ls -la "$OUT" "$OUT/lib"
