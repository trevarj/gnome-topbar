#!/usr/bin/env sh
# Quick Settings under a real pointer, at the scale a real display runs at.
#
#   nix develop -c ./scripts/smoke-qs-pointer.sh
#
# smoke-quick-settings.sh photographs the panel; this one *uses* it. Every
# control is located from the panel's own dump and pressed by a synthetic
# pointer, which is the only way the path from "the compositor delivered a
# button event" to "a GTK gesture fired" is exercised at all. Two dead controls
# shipped to a real desktop behind a green suite before there was any way to
# click one, and a third — the output chooser's arrow — was found by pointing
# this at it.
#
# It runs at TOPBAR_SMOKE_SCALE=1.0 on purpose. The other drivers run the
# nested output at 0.75, where three device pixels stand in for four logical
# ones and the resampling averages away precisely the fringing, the half-pixel
# baselines and the icon misalignment a refinement pass exists to find.
#
#   1  header    battery pill, charge limits, lock, power section, a real hold
#   2  sliders   volume by click, mute, the output chooser, Caffeine
#   3  wifi      the list, a join, the password box, scrolling, the radio
#   4  devices   Bluetooth switches, VPN rows, Power Mode's radio rows
#
# Screenshots and captured output land in target/visual-smoke/qs-pointer/.
set -eu

artifact_root="${1:-target/visual-smoke/qs-pointer}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in magick niri grim cargo timeout dbus-run-session wlrctl wtype pulseaudio; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)

# The live config, with one line changed. Its Quick Settings section binds the
# lock command, and this run *clicks the lock button*: with the real one bound
# the driver would lock the developer's screen half way through a screenshot.
# The replacement is a path that does not exist, so the click takes the whole
# inline-failure path instead — which is the thing worth photographing anyway.
#
# The second edit takes `update_count_command` out. The live config names a
# guix helper that has not existed since the NixOS migration, and a configured
# command always beats the distribution the panel deduced — so the updates card
# spent every run reporting exit 127 and staying hidden, whatever fixtures were
# put on PATH for it. Without the line, the Arch `/etc/os-release` and the
# `checkupdates` below are what the card counts.
config="$artifact_root/config.toml"
sed -e 's|^on_click_right = .*|on_click_right = "/nonexistent/loginctl lock-session"|' \
  -e '/^update_count_command = /d' \
  "$repo/crates/topbar-core/tests/fixtures/live-config.toml" >"$config"
if grep -q "^update_count_command" "$config"; then
  echo "smoke-qs-pointer: the updates command is still configured" >&2
  exit 1
fi

# A network of the run's own. The real NetworkManager is on the system bus and
# IS the developer's connection; a run that clicks a Wi-Fi list must never be
# pointed at it.
#
# Twelve access points, not four: a list that fits inside the panel proves
# nothing about a panel that has to scroll, and a real café has a dozen. One of
# them is open, and one has a name far longer than half a panel — an SSID is 32
# bytes of whatever its owner felt like and the row has to ellipsize rather than
# push the padlock off the end.
smoke_nm='--ap Usadba:82:secured --ap Cafe:58:secured --ap Airport:33:open
  --ap Library:47:secured
  --ap Mokrinskiy_Guest_Network_2_4GHz:64:secured
  --ap Neighbour:71:secured --ap Rostelecom_5G:39:secured
  --ap MGTS-GPON-4471:52:secured --ap Beeline_Home:28:secured
  --ap iPhone:88:secured --ap Printer_Direct:19:open
  --ap Kvartira_44:45:secured
  --saved Usadba --active Usadba --carrier 1000
  --vpn Work:uuid-work:wireguard
  --vpn Home:uuid-home:vpn:org.freedesktop.NetworkManager.openvpn'

# Likewise for BlueZ: three paired devices, one connected and reporting a
# battery, so the switch, the spinner and the percentage all have a row.
smoke_bluez='--device buds|WH-1000XM4|AA:BB:CC:DD:EE:FF|audio-headset|connected|85
  --device mouse|MX_Master_3S|11:22:33:44:55:66|input-mouse
  --device kb|Magic_Keyboard|99:88:77:66:55:44|input-keyboard'

# An Arch with updates pending, so the updates card is in every screenshot.
# The card is absent on a machine with nothing to say, which is correct and
# means a run on this NixOS would never photograph it — and it is one of the
# two cards at the foot of the panel whose height is worth looking at.
fixtures="$artifact_root/fixtures"
mkdir -p "$fixtures/bin"
cat >"$fixtures/os-release" <<'EOF'
NAME="Arch Linux"
PRETTY_NAME="Arch Linux"
ID=arch
BUILD_ID=rolling
EOF
cat >"$fixtures/bin/checkupdates" <<'EOF'
#!/usr/bin/env sh
cat <<'PACKAGES'
linux 6.12.4.arch1-1 -> 6.12.5.arch1-1
mesa 1:24.3.1-1 -> 1:24.3.2-1
firefox 133.0-1 -> 133.0.3-1
sqlite 3.47.1-1 -> 3.47.2-1
systemd 257.1-1 -> 257.2-1
PACKAGES
EOF
chmod +x "$fixtures/bin/checkupdates"

status=0

run() {
  scenario=$1

  echo "smoke-qs-pointer: $scenario"
  RUST_LOG="topbar::widgets::quick_settings=debug,topbar::bridge=debug" \
  SMOKE_QS_POINTER_SCENARIO="$scenario" \
  TOPBAR_SMOKE_SCALE=1.0 \
  TOPBAR_SMOKE_PULSE=2 \
  TOPBAR_SMOKE_NM="$smoke_nm" \
  TOPBAR_SMOKE_BLUEZ="$smoke_bluez" \
  TOPBAR_SMOKE_POWER="--active balanced --percent 62 --state 2 --time-to-empty 8100" \
  TOPBAR_SMOKE_OSRELEASE="$fixtures/os-release" \
  SMOKE_PATH="$fixtures/bin" \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-400}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-qs-pointer-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$scenario" \
    >"$artifact_root/$scenario.log" 2>&1 ||
    echo "smoke-qs-pointer: $scenario exited non-zero; see $artifact_root/$scenario.log" >&2
  mv "$artifact_root/$scenario/panel.log" "$artifact_root/$scenario-panel.log" 2>/dev/null || true
}

for scenario in ${SMOKE_QS_POINTER_ONLY:-header sliders wifi devices}; do
  run "$scenario"
done

echo "--- screenshots ---"
find "$artifact_root" -name '*.png' | sort
echo "--- what the driver saw ---"
for file in "$artifact_root"/*/driver.log; do
  [ -e "$file" ] || continue
  echo "=== $file"
  cat "$file"
  grep -q "result: PASS" "$file" || status=1
done
# The panel logs its level in colour, so the escapes come out before the level
# can be matched on.
echo "--- warnings and errors ---"
sed 's/\x1b\[[0-9;]*m//g' "$artifact_root"/*-panel.log 2>/dev/null |
  grep -E "( WARN | ERROR |Gtk-WARNING|Gtk-CRITICAL)" || echo "none"
# A hold that reached logind would be a hold that acted on the machine this run
# is inside. The build refuses before it connects; this is the proof.
echo "--- any power action that reached the bus (there must be none) ---"
sed 's/\x1b\[[0-9;]*m//g' "$artifact_root"/header-panel.log 2>/dev/null |
  grep -E "no system bus|no logind" || echo "none"

exit "$status"
