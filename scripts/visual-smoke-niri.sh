#!/usr/bin/env sh
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

timeout 30s niri -- sh -c '
set -eu
export LD_LIBRARY_PATH=${LIBRARY_PATH:-}
"$1" --config "$2" -v &
panel_pid=$!
sleep 2
grim "$3/gnome-topbar.png"
kill "$panel_pid" 2>/dev/null || true
wait "$panel_pid" 2>/dev/null || true
niri msg action quit --skip-confirmation >/dev/null 2>&1 || true
' sh "$binary_abs" "$config_abs" "$artifact_dir_abs"

test -s "$artifact_dir_abs/gnome-topbar.png"
echo "wrote $artifact_dir_abs/gnome-topbar.png"
