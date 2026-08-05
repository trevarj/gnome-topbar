#!/usr/bin/env sh
# The whole bar, from the user's own configuration, in one frame.
#
#   nix develop -c ./scripts/smoke-full-bar.sh
#
# Every other smoke script is about one widget. This one is about all of them
# at once: it runs `crates/topbar-core/tests/fixtures/live-config.toml` — the
# byte-for-byte copy of the configuration this project is written for — with a
# stand-in behind every service that would otherwise reach the developer's own
# session, and takes one screenshot.
#
# What stands in for what, and why:
#
#   weather          scripts/weather-stub.py, so the forecast is the fixture's
#                    and the run needs no network. Coordinates are added to the
#                    config because the live one has none — the user picked
#                    their location in v1's dialog, which lives in state.json.
#   custom-crypto    a fixture script in the artifact directory rather than the
#                    user's real crypto.sh, which needs curl, jq and the
#                    internet. It prints the same shape, so the widget is
#                    exercised through the same contract.
#   headsetcontrol   a fixture on SMOKE_PATH reporting a headset at 45%. The
#                    real tool would report whatever is actually plugged in,
#                    which is nothing repeatable.
#   NetworkManager   topbar-fake-nm, BlueZ topbar-fake-bluez, UPower and
#                    power-profiles topbar-fake-power, PulseAudio a null sink.
#                    All four live on the SYSTEM bus in reality and are the
#                    developer's live network, headphones and battery.
#
# Two widgets are expected to draw *nothing*, and that is the point of having
# them in the frame: the tray, because no SNI application is running, and the
# system monitor, because the machine is healthy. Both are checked against
# `niri msg layers` and the panel log rather than assumed.
#
# Screenshots land in target/visual-smoke/full-bar/.
set -eu

# The fakes do not exit when their private bus dies, so an interrupted run
# strands them. Reap everything this run spawned, whatever happens.
trap 'pkill -f "target/debug/topbar-fake-" 2>/dev/null || true' EXIT INT TERM

artifact_root="${1:-target/visual-smoke/full-bar}"
port="${TOPBAR_SMOKE_STUB_PORT:-18082}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in python3 magick curl niri grim cargo timeout dbus-run-session pulseaudio gdbus; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)
live_config=crates/topbar-core/tests/fixtures/live-config.toml

# --- the crypto script -----------------------------------------------------
#
# The user's own script asks CoinGecko over HTTPS through curl and jq. This one
# prints the same shape — three values, the third a ratio — from nothing, so the
# frame is the same every time and the sandbox needs no network.
bin="$artifact_root/bin"
mkdir -p "$bin"
crypto="$bin/crypto.sh"
cat >"$crypto" <<'SH'
#!/usr/bin/env sh
# Stands in for the user's crypto.sh. Same output shape, no network.
printf 'BTC 103412  ETH 3412  \xe2\x82\xbf0.033\n'
SH
chmod +x "$crypto"

# --- the headset -----------------------------------------------------------
#
# Reads a state file so a later scenario can change the reading underneath a
# running panel; this run leaves it at 45% and discharging.
headset_state="$artifact_root/headset.json"
cat >"$headset_state" <<'JSON'
{"devices":[{"status":"success","device":"Arctis Nova 7",
  "battery":{"status":"BATTERY_AVAILABLE","level":45}}]}
JSON
cat >"$bin/headsetcontrol" <<SH
#!/usr/bin/env sh
# Stands in for headsetcontrol(1). Prints whatever the state file holds.
cat "$headset_state"
SH
chmod +x "$bin/headsetcontrol"

# --- the configuration -----------------------------------------------------
#
# The live file with two changes, both of them about the sandbox rather than
# about the panel: coordinates for the weather, and the crypto script's path.
config="$artifact_root/full-bar-config.toml"
sed -e 's/^\[widgets.weather\]$/[widgets.weather]\nlatitude = 55.75204\nlongitude = 37.61781/' \
  -e "s#^exec = .*#exec = \"$crypto\"#" \
  "$live_config" >"$config"
grep -q '^latitude' "$config" || {
  echo "could not add coordinates to the config" >&2
  exit 1
}
grep -q "^exec = \"$crypto\"$" "$config" || {
  echo "could not point custom-crypto at the fixture script" >&2
  exit 1
}

# --- the weather endpoint --------------------------------------------------
stub_pid=""
stop_stub() {
  if [ -n "$stub_pid" ]; then
    kill "$stub_pid" 2>/dev/null || true
    wait "$stub_pid" 2>/dev/null || true
    stub_pid=""
  fi
}
trap 'stop_stub; pkill -f "target/debug/topbar-fake-" 2>/dev/null || true' EXIT INT TERM

python3 "$repo/scripts/weather-stub.py" --port "$port" --status 200 \
  >"$artifact_root/stub.log" 2>&1 &
stub_pid=$!
waited=0
while [ "$waited" -lt 10 ]; do
  got=$(curl -s -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:$port/forecast?probe=1" 2>/dev/null || echo 000)
  [ "$got" = "200" ] && break
  sleep 1
  waited=$((waited + 1))
done
if [ "$got" != "200" ]; then
  echo "smoke-full-bar: nothing on port $port answers 200 (got '$got')" >&2
  echo "smoke-full-bar: a stray stub is probably holding it — check" >&2
  echo "  pgrep -af weather-stub.py" >&2
  exit 1
fi
echo "smoke-full-bar: weather stub answering on port $port"

export TOPBAR_WEATHER_API="http://127.0.0.1:$port/forecast"
export TOPBAR_GEOCODING_API="http://127.0.0.1:$port/search"

# What the rest of the panel has to have behind it for its indicators to exist:
# a network with a tunnel up, a headset paired over Bluetooth, and a battery.
nm='--ap Usadba:82:secured --ap Cafe:58:secured
  --saved Usadba --active Usadba
  --vpn Work:uuid-work:wireguard --vpn-active uuid-work'
bluez='--device buds|WH-1000XM4|AA:BB:CC:DD:EE:FF|audio-headset|connected|85'
power='--active balanced --percent 62 --state 2 --time-to-empty 8100'

RUST_LOG="info,topbar::bar=debug,topbar::widgets=debug,topbar_services::custom=debug,topbar_services::headset=debug" \
SMOKE_HEADSET_STATE="$headset_state" \
SMOKE_PATH="$bin" \
TOPBAR_SMOKE_PULSE=1 \
TOPBAR_SMOKE_NM="$nm" \
TOPBAR_SMOKE_BLUEZ="$bluez" \
TOPBAR_SMOKE_POWER="$power" \
TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-90}" \
TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-full-bar-shot.sh" \
TOPBAR_VISUAL_CONFIG="$config" \
  "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/bar" \
  >"$artifact_root/bar.log" 2>&1 ||
  echo "smoke-full-bar: the run exited non-zero; see $artifact_root/bar.log" >&2

stop_stub

echo "--- screenshots ---"
find "$artifact_root" -name '*.png' | sort
echo "--- panel log ---"
cat "$artifact_root/bar/panel.log" 2>/dev/null || true
