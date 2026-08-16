#!/bin/sh
set -e
root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/bin"
copy_into() {
    dir="$1"
    if [ -d "$dir" ]; then
        mkdir -p "$dir/bin"
        cp "$bin/"* "$dir/bin/"
    fi
}
copy_into "$HOME/Library/Application Support/Zed/extensions/work/zed-cl"
copy_into "$HOME/.local/share/zed/extensions/work/zed-cl"
copy_into "${APPDATA:-}/Zed/extensions/work/zed-cl"
