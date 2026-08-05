#!/usr/bin/env sh
# The pointer matrix: the panel driven by synthetic clicks, inside nested niri.
#
#   nix develop -c ./scripts/smoke-input.sh
#
# Every other smoke run opens popovers through TOPBAR_SMOKE_OPEN or
# `topbar popover show`, which dispatch the action a click would have
# dispatched and never touch the path from "the compositor delivered a button
# event" to "a GTK gesture fired". Two bugs walked through that gap onto a real
# desktop with the whole suite green behind them:
#
#   - the click-catcher never dismissed anything, because a GtkWindow whose
#     content paints nothing gives GDK an empty scene and GDK then commits the
#     layer surface with no buffer on it. An unmapped surface is not in the
#     compositor's input routing, so every click went straight through to the
#     window underneath — while `niri msg layers` cheerfully listed the
#     catcher, because it lists layer surfaces once they are configured;
#   - the Wi-Fi and Bluetooth chevrons never opened their lists, because a
#     GtkButton nested inside another GtkButton cannot be clicked: GTK runs the
#     button gesture in the capture phase and the outer button claims the
#     sequence on release, which cancels every gesture below it. Clicking the
#     arrow switched the *radio* instead.
#
# niri offers zwlr_virtual_pointer_manager_v1 and zwp_virtual_keyboard_manager_v1
# to its clients, nested sessions included, so `wlrctl` and `wtype` can drive one
# from the inside. scripts/smoke-pointer.sh wraps them.
#
#   1  click    open from the bar, the chevron, click-away, toggle shut
#   2  escape   open from the bar, dismiss with Escape (the control)
#
# Screenshots and captured output land in target/visual-smoke/input/<scenario>/.
set -eu

artifact_root="${1:-target/visual-smoke/input}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in magick niri grim cargo timeout dbus-run-session wlrctl wtype; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)
live_config="crates/topbar-core/tests/fixtures/live-config.toml"

# A network of the run's own. The real NetworkManager is on the system bus and
# IS the developer's connection; a run that clicks a Wi-Fi list must never be
# pointed at it. What is in range is fixed, so the expanded list is the same
# every time and a screenshot of it means something.
smoke_nm='--ap Usadba:82:secured --ap Cafe:58:secured --ap Airport:33:open
  --ap Library:47:secured --saved Usadba --active Usadba --carrier 1000
  --vpn Work:uuid-work:wireguard
  --vpn Home:uuid-home:vpn:org.freedesktop.NetworkManager.openvpn'

status=0

run() {
  scenario=$1

  echo "smoke-input: $scenario"
  RUST_LOG="topbar::widgets::quick_settings=debug,topbar::bridge=debug,topbar::surfaces=debug" \
  SMOKE_INPUT_SCENARIO="$scenario" \
  TOPBAR_SMOKE_NM="$smoke_nm" \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-200}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-input-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$repo/$live_config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$scenario" \
    >"$artifact_root/$scenario.log" 2>&1 ||
    echo "smoke-input: $scenario exited non-zero; see $artifact_root/$scenario.log" >&2
  mv "$artifact_root/$scenario/panel.log" "$artifact_root/$scenario-panel.log" 2>/dev/null || true
}

run click
run escape

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
# The body of the Wi-Fi pill switches the radio; the chevron opens the list.
# A chevron click that reaches the body says the nesting is back.
echo "--- any radio change (a chevron click must cause none) ---"
sed 's/\x1b\[[0-9;]*m//g' "$artifact_root"/click-panel.log 2>/dev/null |
  grep -iE "set_wifi_enabled|quick_settings.wifi:" || echo "none"

exit "$status"
