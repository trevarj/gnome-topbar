#!/usr/bin/env sh
# The Bluetooth matrix, driven inside the nested niri session.
#
#   nix develop -c ./scripts/smoke-bluetooth.sh
#
# Every run brings up a BlueZ of its own — `topbar-fake-bluez` — on the run's
# private session bus, and points the panel at it with TOPBAR_SMOKE_BLUEZ_BUS.
# The real BlueZ is on the *system* bus and IS the developer's headphones.
# Nothing here may switch that radio off, disconnect what is playing, or
# register a pairing agent against it — a second agent would take the pairing
# prompts the session's own desktop is waiting for. A debug build with no
# TOPBAR_SMOKE_BLUEZ_BUS refuses all three by construction (see
# `network::Access`), and scenario (e) is the proof.
#
# A fake NetworkManager and a fake UPower come up alongside, because the point
# of scenario (a) is the *complete* GNOME 45 grid — five toggles at once — and
# three of them need a daemon to be present at all.
#
#   0  bar        the button itself, with every indicator it can carry at once
#   1  grid       all five toggles: Wi-Fi, Bluetooth, VPN, Caffeine, Power Mode
#   2  devices    three paired devices; one connected with its battery, one
#                 mid-connect with a spinner, one idle
#   3  fail       a connect that fails: the inline caption, the switch reverted
#   4  pairing    a pairing this panel did not start: the code, then confirmed
#   5  off        the radio switched off: the caption, and the indicator gone
#   6  real       the REAL system bus, read-only: no agent, no writes
#
# Screenshots and captured output land in target/visual-smoke/bt/<scenario>/.
set -eu

# The fakes do not exit when their private bus dies, so an interrupted or
# timed-out run strands them — the tray run once left fourteen alive for nine
# hours. Reap every fake this run spawned, whatever happens. The real BlueZ,
# NetworkManager and UPower are on the system bus and are not matched by any
# of these patterns.
trap 'for fake in bluez nm power; do
  pkill -f "target/debug/topbar-fake-" 2>/dev/null || true
done' EXIT INT TERM

artifact_root="${1:-target/visual-smoke/bt}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in magick niri grim cargo timeout dbus-run-session gdbus; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)
live_config="crates/topbar-core/tests/fixtures/live-config.toml"

# Which device the connect scenarios act on. Read by the panel at start-up, so
# it has to be in the environment before the session is brought up rather than
# exported inside the driver.
export TOPBAR_SMOKE_BT_DEVICE=/org/bluez/hci0/dev_kb

# What is paired in most scenarios: a headset that is connected and reports a
# battery, a mouse, and a keyboard.
default_bt='--device buds|WH-1000XM4|AA:BB:CC:DD:EE:FF|audio-headset|connected|85
  --device mouse|MX_Master_3S|11:22:33:44:55:66|input-mouse
  --device kb|Magic_Keyboard|99:88:77:66:55:44|input-keyboard'

# Enough of a network for the Wi-Fi and VPN pills to exist. The bar scenario
# raises the tunnel as well, so the badge is on the button.
default_nm='--ap Usadba:82:secured --ap Cafe:58:secured
  --saved Usadba --active Usadba
  --vpn Work:uuid-work:wireguard --vpn-active uuid-work'

default_power='--active balanced --percent 62 --state 2 --time-to-empty 8100'

# One nested session: run <scenario> <smoke-open> [fake-bluez args]
run() {
  scenario=$1
  # Empty by default: the `bar` scenario opens nothing, and `set -u` turns a
  # missing argument into a run that dies before it starts.
  open=${2:-}
  bt_args=${3:-$default_bt}

  echo "smoke-bt: $scenario"
  if [ -n "$open" ]; then
    export TOPBAR_SMOKE_OPEN="$open"
  else
    unset TOPBAR_SMOKE_OPEN
  fi

  RUST_LOG="topbar::widgets::quick_settings=debug,topbar::bridge=debug,topbar_services::bluetooth=debug" \
  SMOKE_BT_SCENARIO="$scenario" \
  TOPBAR_SMOKE_BLUEZ="$bt_args" \
  TOPBAR_SMOKE_NM="$default_nm" \
  TOPBAR_SMOKE_POWER="$default_power" \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-120}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-bluetooth-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$repo/$live_config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$scenario" \
    >"$artifact_root/$scenario.log" 2>&1 ||
    echo "smoke-bt: $scenario exited non-zero; see $artifact_root/$scenario.log" >&2
  mv "$artifact_root/$scenario/panel.log" "$artifact_root/$scenario-panel.log" 2>/dev/null || true
  mv "$artifact_root/$scenario/fake-bluez.log" "$artifact_root/$scenario-bluez.log" 2>/dev/null || true
}

# The same, against the machine's real BlueZ. No TOPBAR_SMOKE_BLUEZ at all, so
# no override reaches the service and the access policy is what is on trial.
run_real() {
  echo "smoke-bt: real (read-only against the system bus)"
  export TOPBAR_SMOKE_OPEN=quick-settings-bluetooth

  RUST_LOG="topbar_services::bluetooth=debug" \
  SMOKE_BT_SCENARIO=real \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-90}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-bluetooth-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$repo/$live_config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/real" \
    >"$artifact_root/real.log" 2>&1 ||
    echo "smoke-bt: real exited non-zero; see $artifact_root/real.log" >&2
  mv "$artifact_root/real/panel.log" "$artifact_root/real-panel.log" 2>/dev/null || true
}

# The button on the bar, with a sound server of its own so a recording can
# raise the microphone dot beside everything else.
TOPBAR_SMOKE_PULSE=1 run bar
run grid quick_settings
run devices quick-settings-bluetooth
# The queued outcome is what makes the third device's row spin long enough to
# be photographed, and the second run's `fail` is what reverts a switch.
run spinner quick-settings-bluetooth "$default_bt --outcome slow"
run fail quick-settings-bluetooth-connect "$default_bt --outcome fail"
run pairing quick-settings-bluetooth
run off quick-settings-bluetooth-off
run_real

echo "--- screenshots ---"
find "$artifact_root" -name '*.png' | sort
