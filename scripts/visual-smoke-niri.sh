#!/usr/bin/env sh
# Nested-niri visual smoke test: start the panel inside a headless-ish niri
# session, screenshot it with grim, and archive the PNG.
#
# Local only — niri has no headless backend, so CI cannot run this.
# Run it from the dev shell: nix develop -c ./scripts/visual-smoke-niri.sh
#
# EVERYTHING runs inside `dbus-run-session`, on a private bus that exists only
# for the length of the run. This is not optional: from M4 the panel takes
# `org.freedesktop.Notifications` with ReplaceExisting, and a nested panel on
# the developer's real session bus would take the desktop's notifications away
# from whatever is actually serving them.
#
# Environment:
#   TOPBAR_VISUAL_CONFIG  panel config to run (default ./config.toml)
#   TOPBAR_SMOKE_OPEN     open a widget's popover without a pointer.
#                         There is no synthetic input in the dev shell, so
#                         this is how an *open* popover gets screenshotted
#                         before M8's `topbar popover show` exists. Debug
#                         builds only, which is what this script builds.
#
#                           clock    open it a second in, leave it open
#                           clock:6  six toggles 1.5s apart. An even count
#                                    ends closed (check teardown: `niri msg
#                                    layers` should list only the bar); an
#                                    odd one ends reopened onto retained
#                                    content.
#
#   TOPBAR_SMOKE_DRIVER   a shell script run inside the nested session once
#                         the panel is up, instead of the default "wait,
#                         then take one screenshot". It is given
#                         $SMOKE_ARTIFACTS and may call notify-send, gdbus,
#                         and grim against the private bus, which is how the
#                         notification matrix is driven.
#   TOPBAR_SMOKE_TIMEOUT  seconds before the session is killed (30).
set -eu

artifact_dir="${1:-target/visual-smoke}"
config="${TOPBAR_VISUAL_CONFIG:-config.toml}"
driver="${TOPBAR_SMOKE_DRIVER:-}"
timeout_s="${TOPBAR_SMOKE_TIMEOUT:-30}"
mkdir -p "$artifact_dir"

bus_config=$(pwd)/scripts/smoke-session.conf

# Sandbox every XDG write path. The panel migrates/creates state dirs and will
# grow cache use over time; a smoke run must never touch the developer's real
# ~/.local/state (this bit us once: the state-dir migration renamed the live
# v1 panel's state directory). Config is passed with --config explicitly, but
# XDG_CONFIG_HOME is boxed too so the legacy-path fallback can't find a real
# user config and add warning noise to panel.log.
xdg_box=$(mktemp -d "${TMPDIR:-/tmp}/topbar-smoke-xdg.XXXXXX")
trap 'rm -rf "$xdg_box"' EXIT INT TERM
export XDG_STATE_HOME="$xdg_box/state"
export XDG_CACHE_HOME="$xdg_box/cache"
export XDG_CONFIG_HOME="$xdg_box/config"
mkdir -p "$XDG_STATE_HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME"

for tool in niri grim cargo timeout dbus-run-session; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

cargo build -p topbar

artifact_dir_abs=$(cd "$artifact_dir" && pwd)
config_abs=$(cd "$(dirname "$config")" && pwd)/$(basename "$config")
binary_abs=$(pwd)/target/debug/topbar
driver_abs=""
if [ -n "$driver" ]; then
  driver_abs=$(cd "$(dirname "$driver")" && pwd)/$(basename "$driver")
fi

# niri detaches the stdio of the processes it spawns, so the panel's own log
# (and any GTK CSS warning) is captured to a file instead of the terminal.
timeout "${timeout_s}s" dbus-run-session --config-file="$bus_config" -- niri -- sh -c '
set -eu
export SMOKE_ARTIFACTS="$3"
"$1" --config "$2" -v >"$3/panel.log" 2>&1 &
panel_pid=$!
sleep 2
if [ -n "$4" ]; then
  sh "$4" || echo "smoke driver failed with status $?" >&2
else
  grim "$3/topbar.png"
fi
kill "$panel_pid" 2>/dev/null || true
wait "$panel_pid" 2>/dev/null || true
niri msg action quit --skip-confirmation >/dev/null 2>&1 || true
' sh "$binary_abs" "$config_abs" "$artifact_dir_abs" "$driver_abs"

echo "--- panel log ---"
cat "$artifact_dir_abs/panel.log" 2>/dev/null || true
echo "-----------------"
ls -1 "$artifact_dir_abs"/*.png 2>/dev/null || {
  echo "no screenshots were taken" >&2
  exit 1
}
