#!/usr/bin/env sh
# Per-user install of Wordy on Linux: binary, launcher entry and icon under ~/.local.
# Usage: packaging/linux/install.sh [--build | --uninstall]
#   WORDY_BIN=path/to/wordy  install that binary instead of target/release/wordy
# The .desktop file is named after the window app_id (dev.sam.wordy) so Wayland
# compositors can match the running window to its icon.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
app=dev.sam.wordy
bin_dir=${XDG_BIN_HOME:-$HOME/.local/bin}
data_dir=${XDG_DATA_HOME:-$HOME/.local/share}
apps_dir=$data_dir/applications
icons_dir=$data_dir/icons/hicolor
sizes="16 32 48 64 128 256 512"

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps_dir" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$icons_dir" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin_dir/wordy" "$apps_dir/$app.desktop" "$icons_dir/scalable/apps/$app.svg"
    for s in $sizes; do rm -f "$icons_dir/${s}x${s}/apps/$app.png"; done
    refresh
    echo "removed Wordy from $bin_dir and $data_dir"
    exit 0
fi

# In a release tarball the binary and icon sit next to this script; in a checkout
# they come from target/release and packaging/.
bin=${WORDY_BIN:-}
if [ -z "$bin" ]; then
    if [ -x "$here/wordy" ]; then bin=$here/wordy; else bin=$root/target/release/wordy; fi
fi
if [ ! -x "$bin" ] || [ "${1:-}" = "--build" ]; then
    (cd "$root" && cargo build --release)
fi
svg=$here/wordy.svg
[ -f "$svg" ] || svg=$root/packaging/wordy.svg

install -Dm755 "$bin" "$bin_dir/wordy"
install -Dm644 "$here/$app.desktop" "$apps_dir/$app.desktop"
install -Dm644 "$svg" "$icons_dir/scalable/apps/$app.svg"
for s in $sizes; do
    install -Dm644 "$here/icon-$s.png" "$icons_dir/${s}x${s}/apps/$app.png"
done
refresh

echo "installed: $bin_dir/wordy, $apps_dir/$app.desktop, icons in $icons_dir"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "note: $bin_dir is not on PATH; the launcher entry still works, the 'wordy' command will not" ;;
esac
