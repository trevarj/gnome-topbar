#!/usr/bin/env sh
# The Quick Settings matrix, driven inside the nested niri session.
#
#   nix develop -c ./scripts/smoke-quick-settings.sh
#
# Every run brings up three stand-ins of its own, all inside the run's sandbox:
# a PulseAudio with a null sink and a null source, a UPower, and a
# power-profiles daemon. The last two matter most — the real ones live on the
# *system* bus, which nothing here can box, and a screenshot is not worth
# changing the developer's charge limit or CPU governor. The panel is pointed
# at the fakes with TOPBAR_SMOKE_POWER_BUS, and its charge-limit writes land in
# a temporary /sys/class/power_supply tree.
#
# logind is deliberately left alone: the idle inhibitor keeps talking to the
# real one, as it has since M8, which is what makes the Caffeine toggle
# available in these screenshots at all.
#
#   1  bar       the button on the bar: speaker and battery
#   2  panel     the panel: header, sliders, toggle grid
#   3  mode      Power Mode expanded, then the daemon moved from outside
#   4  volume    a panel-originated volume change, and no capsule with it
#   5  mic       the microphone slider arriving with a recording and leaving
#   6  power     the power section, one row painted mid-hold
#   7  battery   the health card, and the charge limit actually moving
#   8  low       a nearly-flat battery: the urgent tint
#
# Screenshots and captured output land in target/visual-smoke/qs/<scenario>/.
set -eu

artifact_root="${1:-target/visual-smoke/qs}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in magick niri grim cargo timeout dbus-run-session pulseaudio pactl parecord gdbus; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)
live_config="crates/topbar-core/tests/fixtures/live-config.toml"

# One nested session: run <scenario> <smoke-open> [extra fake-power args]
run() {
  scenario=$1
  open=$2
  power_args=${3:---active balanced --percent 62 --state 2 --time-to-empty 8100}

  echo "smoke-qs: $scenario"
  if [ -n "$open" ]; then
    export TOPBAR_SMOKE_OPEN="$open"
  else
    unset TOPBAR_SMOKE_OPEN
  fi

  RUST_LOG="topbar::widgets::quick_settings=debug,topbar::bridge=debug,topbar_services::power_profiles=debug,topbar_services::battery=debug" \
  SMOKE_QS_SCENARIO="$scenario" \
  TOPBAR_SMOKE_PULSE=1 \
  TOPBAR_SMOKE_POWER="$power_args" \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-120}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-quick-settings-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$repo/$live_config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$scenario" \
    >"$artifact_root/$scenario.log" 2>&1 ||
    echo "smoke-qs: $scenario exited non-zero; see $artifact_root/$scenario.log" >&2
  mv "$artifact_root/$scenario/panel.log" "$artifact_root/$scenario-panel.log" 2>/dev/null || true
}

run bar ""
run panel quick_settings
run mode quick-settings-mode
run volume quick-settings-volume
run mic quick_settings
run power quick-settings-power
run battery quick-settings-limit
run low quick_settings

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
echo "--- what the panel said about holds ---"
sed 's/\x1b\[[0-9;]*m//g' "$artifact_root"/power-panel.log 2>/dev/null |
  grep -E "smoke:" || echo "none"
echo "--- any power action attempted (there must be none) ---"
sed 's/\x1b\[[0-9;]*m//g' "$artifact_root"/*-panel.log 2>/dev/null |
  grep -iE "PowerOff|Reboot|Suspend\(" || echo "none"
