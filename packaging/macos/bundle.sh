#!/usr/bin/env sh
# Build a release binary and wrap it in Wordy.app (unsigned) at target/release/Wordy.app.
# Needs macOS: uses sips and iconutil to make the .icns from packaging/macos/icon-1024.png.
# Usage: packaging/macos/bundle.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)  # [workspace.package]
app="$root/target/release/Wordy.app"

(cd "$root" && cargo build --release)

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/target/release/wordy" "$app/Contents/MacOS/wordy"
sed "s/VERSION/${version:-0.0.0}/g" "$here/Info.plist" > "$app/Contents/Info.plist"

iconset=$(mktemp -d)/wordy.iconset
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
    sips -z "$s" "$s" "$here/icon-1024.png" --out "$iconset/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z "$d" "$d" "$here/icon-1024.png" --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/wordy.icns"
rm -rf "$(dirname "$iconset")"

echo "built $app (unsigned; run 'xattr -dr com.apple.quarantine' if Gatekeeper complains after copying)"
