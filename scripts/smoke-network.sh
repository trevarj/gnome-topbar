#!/usr/bin/env sh
# The network matrix, driven inside the nested niri session.
#
#   nix develop -c ./scripts/smoke-network.sh
#
# Every run brings up a NetworkManager of its own — `topbar-fake-nm` — on the
# run's private session bus, and points the panel at it with
# TOPBAR_SMOKE_NM_BUS. The real NetworkManager is on the *system* bus and IS the
# developer's live network connection. Nothing here may join a network, switch
# a radio, ask a card to scan, or register a secret agent against it: a second
# agent on that bus would sit in the queue for the password prompts the
# session's own panel is waiting for. A debug build with no TOPBAR_SMOKE_NM_BUS
# refuses every one of those by construction — see `network::Access` — and
# scenario (h) is the proof.
#
#   1  bar        the button on the bar: Wi-Fi + VPN badge, then wired
#   2  panel      the panel: the two pills collapsed, and the wired row
#   3  list       the Wi-Fi list expanded, sorted; then a row mid-connect
#   4  password   the password row under a secured network, and the answer
#   5  authfail   a refused password: the caption, and the profile deleted
#   6  vpn        two profiles, one up; then one that hangs and spins
#   7  radiooff   the radio switched off from outside
#   8  real       the REAL system bus, read-only: no scan, no agent, no writes
#
# Screenshots and captured output land in target/visual-smoke/net/<scenario>/.
set -eu

artifact_root="${1:-target/visual-smoke/net}"
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

# What is in range in most scenarios: one strong saved network the card is on,
# a secured stranger, an open stranger, and three more to fill the list.
default_nm='--ap Usadba:82:secured --ap Cafe:58:secured --ap Airport:33:open
  --ap Library:47:secured --ap Neighbour:21:secured --ap Guest:12:open
  --saved Usadba --active Usadba --carrier 1000
  --vpn Work:uuid-work:wireguard
  --vpn Home:uuid-home:vpn:org.freedesktop.NetworkManager.openvpn'

# The same, with the cable out, so the radio is what the bar reports.
wireless_nm='--ap Usadba:82:secured --ap Cafe:58:secured --ap Airport:33:open
  --saved Usadba --active Usadba
  --vpn Work:uuid-work:wireguard'

# One nested session: run <scenario> <smoke-open> [fake-nm args]
run() {
  scenario=$1
  open=$2
  nm_args=${3:-$default_nm}

  echo "smoke-net: $scenario"
  if [ -n "$open" ]; then
    export TOPBAR_SMOKE_OPEN="$open"
  else
    unset TOPBAR_SMOKE_OPEN
  fi

  RUST_LOG="topbar::widgets::quick_settings=debug,topbar::bridge=debug,topbar_services::network=debug" \
  SMOKE_NET_SCENARIO="$scenario" \
  TOPBAR_SMOKE_NM="$nm_args" \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-150}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-network-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$repo/$live_config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$scenario" \
    >"$artifact_root/$scenario.log" 2>&1 ||
    echo "smoke-net: $scenario exited non-zero; see $artifact_root/$scenario.log" >&2
  mv "$artifact_root/$scenario/panel.log" "$artifact_root/$scenario-panel.log" 2>/dev/null || true
}

# The read-only scenario runs with NO fake at all, so the panel finds the real
# system bus and has to keep its hands off it.
run_real() {
  echo "smoke-net: real (read-only against the system bus)"
  export TOPBAR_SMOKE_OPEN=quick-settings-wifi
  RUST_LOG="topbar_services::network=debug,topbar::widgets::quick_settings=debug" \
  SMOKE_NET_SCENARIO=real \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-90}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-network-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$repo/$live_config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/real" \
    >"$artifact_root/real.log" 2>&1 ||
    echo "smoke-net: real exited non-zero; see $artifact_root/real.log" >&2
  mv "$artifact_root/real/panel.log" "$artifact_root/real-panel.log" 2>/dev/null || true
}

export TOPBAR_SMOKE_SSID=Cafe
export TOPBAR_SMOKE_VPN_UUID=uuid-home

run bar "" "$wireless_nm"
run panel quick_settings
run list quick-settings-wifi
run password quick-settings-wifi-connect
# The queued outcome is consumed by the panel's own activation, so the refusal
# is armed before the panel starts rather than raced against it.
run authfail quick-settings-wifi-connect "$default_nm --outcome auth_fail"
run vpn quick-settings-vpn
run radiooff quick-settings-wifi
run_real

echo "--- screenshots ---"
find "$artifact_root" -name '*.png' | sort
echo "--- captured output ---"
for file in "$artifact_root"/*/*.txt; do
  [ -e "$file" ] || continue
  echo "=== $file"
  cat "$file"
done
# The panel logs its level in colour, so the escapes come out before the level
# can be matched on.
echo "--- warnings and errors ---"
sed 's/\x1b\[[0-9;]*m//g' "$artifact_root"/*-panel.log 2>/dev/null |
  grep -E "( WARN | ERROR |Gtk-WARNING|Gtk-CRITICAL)" || echo "none"
# The fake's own recorder is where the password is *supposed* to be: that file
# is the proof it arrived through the secret agent's reply and nowhere else.
echo "--- the password must appear nowhere but the fake's recorder ---"
grep -Rl "topbar-smoke-psk" "$artifact_root" 2>/dev/null |
  grep -vE "psk-audit|-Secrets[.]txt|driver[.]log" ||
  echo "nowhere else (correct)"
echo "--- what the read-only run decided ---"
sed 's/\x1b\[[0-9;]*m//g' "$artifact_root"/real-panel.log 2>/dev/null |
  grep -iE "read-only|secret agent" || echo "none"
