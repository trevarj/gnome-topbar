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
#                         $SMOKE_ARTIFACTS, $SMOKE_PANEL_PID (for reading
#                         VmRSS out of /proc) and, when it was built,
#                         $SMOKE_FAKE_PLAYER. It may call notify-send,
#                         gdbus, and grim against the private bus, which is
#                         how the notification and media matrices are driven.
#   TOPBAR_SMOKE_PLAYERS  build `topbar-fake-player` and hand the driver its
#                         path in $SMOKE_FAKE_PLAYER. Off by default: it is
#                         a second binary to link and only the media driver
#                         wants it.
#   TOPBAR_SMOKE_STATE    a state.json copied into the sandboxed
#                         $XDG_STATE_HOME/topbar before the panel starts, so a
#                         run can begin from state a previous session
#                         remembered — a saved weather location, say. The copy
#                         lands inside the sandbox and nowhere else.
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
mkdir -p "$XDG_STATE_HOME" "$XDG_CACHE_HOME" "$XDG_CONFIG_HOME/niri"

if [ -n "${TOPBAR_SMOKE_STATE:-}" ]; then
  mkdir -p "$XDG_STATE_HOME/topbar"
  cp "$TOPBAR_SMOKE_STATE" "$XDG_STATE_HOME/topbar/state.json"
fi

# The nested compositor gets a config of its own, for one reason: niri's
# "Important Hotkeys" overlay opens on top of everything at startup and sits
# in the middle of every screenshot, which is precisely where the panel's
# popovers are.
cat >"$XDG_CONFIG_HOME/niri/config.kdl" <<'KDL'
// Written by scripts/visual-smoke-niri.sh for the nested session.
hotkey-overlay {
    skip-at-startup
}
KDL

for tool in niri grim cargo timeout dbus-run-session; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

cargo build -p topbar

player_abs=""
if [ -n "${TOPBAR_SMOKE_PLAYERS:-}" ]; then
  cargo build -p topbar-services --features fake-player --bin topbar-fake-player
  player_abs=$(pwd)/target/debug/topbar-fake-player
fi

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
export SMOKE_FAKE_PLAYER="$5"
"$1" --config "$2" -v >"$3/panel.log" 2>&1 &
panel_pid=$!
# The driver reads /proc/$SMOKE_PANEL_PID/status to watch the panel grow.
export SMOKE_PANEL_PID="$panel_pid"
sleep 2
if [ -n "$4" ]; then
  sh "$4" || echo "smoke driver failed with status $?" >&2
else
  grim "$3/topbar.png"
fi
kill "$panel_pid" 2>/dev/null || true
wait "$panel_pid" 2>/dev/null || true
niri msg action quit --skip-confirmation >/dev/null 2>&1 || true
' sh "$binary_abs" "$config_abs" "$artifact_dir_abs" "$driver_abs" "$player_abs"

echo "--- panel log ---"
cat "$artifact_dir_abs/panel.log" 2>/dev/null || true
echo "-----------------"
ls -1 "$artifact_dir_abs"/*.png 2>/dev/null || {
  echo "no screenshots were taken" >&2
  exit 1
}
