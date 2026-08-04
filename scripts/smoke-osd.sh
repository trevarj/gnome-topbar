#!/usr/bin/env sh
# The OSD, IPC and CLI matrix, driven inside the nested niri session.
#
#   nix develop -c ./scripts/smoke-osd.sh
#
# Every run brings up a PulseAudio of its own, inside the run's sandbox, with a
# null sink to change the volume of. The developer's real sound server is never
# connected to: PULSE_SERVER points at the sandbox socket for the panel and for
# every CLI command the driver runs.
#
#   1  set         `volume set 30` → the capsule, then its absence
#   2  retarget    30 then 70 → one capsule, refilled, one surface
#   3  mute        `toggle-mute` → the crossed icon and an empty bar
#   4  brightness  no backlight: a clean failure, plus the capsule's own
#                  brightness path driven by the debug smoke action
#   5  inhibit     `inhibit toggle` with no logind: the message it prints
#   6  popover     `popover show clock` → the control panel; toggle closes it
#   7  dump        the effective config, the state summary, bar show/hide
#   8  second      a second panel meets the lock
#   9  panel-down  the panel is killed and the volume key still works
#
# Screenshots and captured output land in target/visual-smoke/osd/<scenario>/.
set -eu

artifact_root="${1:-target/visual-smoke/osd}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in magick niri grim cargo timeout dbus-run-session pulseaudio pactl; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)

# The live config, with one value changed: the capsule stays up for eight
# seconds rather than the live 1500ms.
#
# Not a fudge — a measurement. The enter fade is frame-clock driven, and the
# nested niri is a winit window nobody is looking at, which the host compositor
# throttles to a frame or two a second. A 150ms fade therefore takes seconds of
# wall time *here* and 150ms everywhere else, while the auto-hide timer is wall
# clock either way. Photographing the capsule at all needs the two to be told
# apart. Disappearance is still proved, eight seconds later.
smoke_config="$artifact_root/osd-config.toml"
sed -e 's/^timeout_ms = 1500$/timeout_ms = 8000/' \
    -e 's/^show_value = false$/show_value = true/' \
    crates/topbar-core/tests/fixtures/live-config.toml >"$smoke_config"
grep -q '^timeout_ms = 8000$' "$smoke_config" || {
  echo "the live config's [osd] timeout is no longer 1500ms" >&2
  exit 1
}
live_config="$smoke_config"

# One nested session: run <scenario> [smoke-open]
run() {
  scenario=$1
  open=${2:-}

  echo "smoke-osd: $scenario"
  if [ -n "$open" ]; then
    export TOPBAR_SMOKE_OPEN="$open"
  else
    unset TOPBAR_SMOKE_OPEN
  fi

  RUST_LOG="topbar::surfaces::osd=debug,topbar::control=debug,topbar_services::ipc=debug,topbar_services::audio=debug" \
  SMOKE_OSD_SCENARIO="$scenario" \
  TOPBAR_SMOKE_PULSE=1 \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-90}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-osd-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$live_config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$scenario" \
    >"$artifact_root/$scenario.log" 2>&1 ||
    echo "smoke-osd: $scenario exited non-zero; see $artifact_root/$scenario.log" >&2
  mv "$artifact_root/$scenario/panel.log" "$artifact_root/$scenario-panel.log" 2>/dev/null || true
}

run set
run retarget
run mute
run brightness osd-brightness
run inhibit
run popover
run dump
run second
run panel-down

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
