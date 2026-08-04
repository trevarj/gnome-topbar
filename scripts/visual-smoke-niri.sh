#!/usr/bin/env sh
# Nested-niri visual smoke test: start the panel inside a headless-ish niri
# session, screenshot it with grim, and archive the PNG.
#
# Local only — niri has no headless backend, so CI cannot run this.
# Run it from the dev shell: nix develop -c ./scripts/visual-smoke-niri.sh
set -eu

artifact_dir="${1:-target/visual-smoke}"
config="${GNOME_TOPBAR_VISUAL_CONFIG:-config.toml}"
mkdir -p "$artifact_dir"

for tool in niri grim cargo timeout; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

cargo build -p gnome-topbar

artifact_dir_abs=$(cd "$artifact_dir" && pwd)
config_abs=$(cd "$(dirname "$config")" && pwd)/$(basename "$config")
binary_abs=$(pwd)/target/debug/gnome-topbar

# niri detaches the stdio of the processes it spawns, so the panel's own log
# (and any GTK CSS warning) is captured to a file instead of the terminal.
timeout 30s niri -- sh -c '
set -eu
"$1" --config "$2" -v >"$3/panel.log" 2>&1 &
panel_pid=$!
sleep 2
grim "$3/gnome-topbar.png"
kill "$panel_pid" 2>/dev/null || true
wait "$panel_pid" 2>/dev/null || true
niri msg action quit --skip-confirmation >/dev/null 2>&1 || true
' sh "$binary_abs" "$config_abs" "$artifact_dir_abs"

test -s "$artifact_dir_abs/gnome-topbar.png"
echo "--- panel log ---"
cat "$artifact_dir_abs/panel.log" 2>/dev/null || true
echo "-----------------"
echo "wrote $artifact_dir_abs/gnome-topbar.png"
