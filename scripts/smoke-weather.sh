#!/usr/bin/env sh
# The weather matrix, driven inside the nested niri session.
#
# Every run points the panel at scripts/weather-stub.py rather than the real
# Open-Meteo, so the screenshots are reproducible and the run needs no network:
#
#   nix develop -c ./scripts/smoke-weather.sh
#
# It stands the stub up, then runs the nested session once per state, because
# TOPBAR_SMOKE_OPEN can only open one surface per start:
#
#   1  ready        bar label and the weather popover, five days of forecast
#   2  panel        the control panel, forecast card filled, no media card
#   3  configure    cold start with no location: the "Configure…" label
#   4  setup        the location dialog, with results from a seeded search
#   5  saved        the same config as (3), but with a location in state.json,
#                   which is what a panel started after a Save sees
#   6  unavailable  the stub answering 429, so the empty state is drawn
#
# Screenshots land in target/visual-smoke/weather/<state>/.
set -eu

artifact_root="${1:-target/visual-smoke/weather}"
port="${TOPBAR_SMOKE_STUB_PORT:-18080}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in python3 niri grim cargo timeout dbus-run-session; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)
live_config=crates/topbar-core/tests/fixtures/live-config.toml

# The live config has no coordinates in it — the user picked their location in
# v1's dialog — so a second config with some is written for the runs that need
# a forecast on screen. Moscow, which is what the fixtures are for.
located_config="$artifact_root/located-config.toml"
sed 's/^\[widgets.weather\]$/[widgets.weather]\nlatitude = 55.75204\nlongitude = 37.61781/' \
  "$live_config" >"$located_config"
grep -q '^latitude' "$located_config" || {
  echo "could not add coordinates to the config" >&2
  exit 1
}

stub_pid=""
stop_stub() {
  if [ -n "$stub_pid" ]; then
    kill "$stub_pid" 2>/dev/null || true
    wait "$stub_pid" 2>/dev/null || true
    stub_pid=""
  fi
}
trap stop_stub EXIT INT TERM

start_stub() {
  stop_stub
  python3 "$repo/scripts/weather-stub.py" --port "$port" --status "$1" \
    >"$artifact_root/stub.log" 2>&1 &
  stub_pid=$!
  # The panel asks for the forecast within a second of starting, so the
  # listener has to be up before the session is.
  sleep 1
}

export TOPBAR_WEATHER_API="http://127.0.0.1:$port/forecast"
export TOPBAR_GEOCODING_API="http://127.0.0.1:$port/search"

# What the setup dialog writes when a location is saved. Seeding a run with it
# is the same thing as starting the panel again after a Save.
saved_state="$artifact_root/saved-state.json"
cat >"$saved_state" <<'JSON'
{
  "weather": {
    "location": {
      "label": "Moscow — Moscow, Russia",
      "latitude": 55.75222,
      "longitude": 37.61556
    }
  }
}
JSON

# One nested session: run <name> <config> [open] [query] [state]
run() {
  name=$1
  config=$2
  open=${3:-}
  query=${4:-}
  state=${5:-}

  if [ -n "$state" ]; then
    export TOPBAR_SMOKE_STATE="$state"
  else
    unset TOPBAR_SMOKE_STATE
  fi

  echo "smoke-weather: $name"
  # Unset rather than empty: an empty TOPBAR_SMOKE_OPEN asks the panel to open
  # a widget called "", which it rightly complains about in panel.log.
  if [ -n "$open" ]; then
    export TOPBAR_SMOKE_OPEN="$open"
  else
    unset TOPBAR_SMOKE_OPEN
  fi
  if [ -n "$query" ]; then
    export TOPBAR_SMOKE_QUERY="$query"
  else
    unset TOPBAR_SMOKE_QUERY
  fi

  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-40}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-weather-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$name" >"$artifact_root/$name.log" 2>&1 ||
    echo "smoke-weather: $name exited non-zero; see $artifact_root/$name.log" >&2
  mv "$artifact_root/$name/panel.log" "$artifact_root/$name-panel.log" 2>/dev/null || true
}

start_stub 200
run ready "$located_config" weather
run panel "$located_config" clock
run configure "$live_config" ""
run setup "$live_config" weather-setup moscow
run saved "$live_config" weather "" "$saved_state"

# The endpoint goes down: with nothing ever fetched the card has nothing to
# keep, so it draws the unavailable state rather than a blank.
start_stub 429
run unavailable "$located_config" clock

stop_stub

echo "--- screenshots ---"
find "$artifact_root" -name '*.png' | sort
echo "--- panel logs ---"
for log in "$artifact_root"/*-panel.log; do
  [ -f "$log" ] || continue
  echo "=== $log ==="
  cat "$log"
done
