#!/bin/bash
# Builds the portable Windows package (folder + ZIP) from a release build. Run it in an MSYS2 UCRT64 shell:
#   MSYSTEM=UCRT64 /c/msys64/usr/bin/bash.exe -lc "cd /c/path/to/titan-control && scripts/package_windows.sh"
# Layout (GTK finds its data relative to the DLLs, so the DLLs sit in bin/ next to the executable):
#   TitanControl-<version>-windows-x64/{bin,lib,share,etc,assets}
set -euo pipefail
cd "$(dirname "$0")/.."

PREFIX="${MINGW_PREFIX:-/ucrt64}"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
NAME="TitanControl-$VERSION-windows-x64"
DIST="${DIST_DIR:-dist}"
OUT="$DIST/$NAME"

cargo build --release
rm -rf "$OUT" "$DIST/$NAME.zip"
mkdir -p "$OUT/bin" "$OUT/lib" "$OUT/share" "$OUT/etc"
cp target/release/titan_control.exe "$OUT/bin/"

# Libraries the executable needs (ldd lists them transitively) ...
copy_deps() {
    ldd "$1" | awk -v p="$PREFIX/" 'index($3, p) == 1 { print $3 }' | sort -u | while read -r dll; do
        [ -f "$OUT/bin/$(basename "$dll")" ] || cp "$dll" "$OUT/bin/"
    done
}
copy_deps target/release/titan_control.exe

# ... and the ones GTK loads at run time: image loaders (SVG icons, PNG).
LOADERS="lib/gdk-pixbuf-2.0/2.10.0"
mkdir -p "$OUT/$LOADERS/loaders"
cp "$PREFIX/$LOADERS/loaders.cache" "$OUT/$LOADERS/" 2>/dev/null || true
for loader in "$PREFIX/$LOADERS"/loaders/*.dll; do
    cp "$loader" "$OUT/$LOADERS/loaders/"
    copy_deps "$loader"
done

# GLib needs the compiled settings schemas (GTK and libadwaita ship theirs).
mkdir -p "$OUT/share/glib-2.0/schemas"
cp "$PREFIX"/share/glib-2.0/schemas/*.gschema.xml "$OUT/share/glib-2.0/schemas/"
glib-compile-schemas "$OUT/share/glib-2.0/schemas"
rm -f "$OUT"/share/glib-2.0/schemas/*.gschema.xml

# Icons (symbolic icons of GTK and libadwaita), translations of the toolkits, fonts configuration.
mkdir -p "$OUT/share/icons"
cp -r "$PREFIX/share/icons/Adwaita" "$OUT/share/icons/"
[ -d "$PREFIX/share/icons/hicolor" ] && cp -r "$PREFIX/share/icons/hicolor" "$OUT/share/icons/"
for lang in "$PREFIX"/share/locale/*/LC_MESSAGES; do
    code=$(basename "$(dirname "$lang")")
    for domain in gtk40 libadwaita glib20; do
        if [ -f "$lang/$domain.mo" ]; then
            mkdir -p "$OUT/share/locale/$code/LC_MESSAGES"
            cp "$lang/$domain.mo" "$OUT/share/locale/$code/LC_MESSAGES/"
        fi
    done
done
[ -d "$PREFIX/etc/fonts" ] && cp -r "$PREFIX/etc/fonts" "$OUT/etc/"

# The app's own pictures and the documentation.
cp -r assets "$OUT/assets"
rm -rf "$OUT/assets/windows"
cp LICENSE README.md FAQ.md "$OUT/"
[ -f CHANGELOG.md ] && cp CHANGELOG.md "$OUT/"

# ZIP (PowerShell is always there on Windows).
WIN_OUT=$(cygpath -m "$PWD/$DIST")
powershell.exe -NoProfile -Command "Compress-Archive -Path '$WIN_OUT/$NAME' -DestinationPath '$WIN_OUT/$NAME.zip' -Force"
echo "Package: $OUT ($(du -sh "$OUT" | cut -f1)), $OUT.zip"
