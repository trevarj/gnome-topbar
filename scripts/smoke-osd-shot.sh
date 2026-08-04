#!/usr/bin/env sh
# The OSD/IPC driver, run inside the nested niri session by
# scripts/smoke-osd.sh. One scenario per session, named by $SMOKE_OSD_SCENARIO.
#
# Every scenario drives the *real* CLI (`$SMOKE_TOPBAR volume set 30`, …)
# against the *real* panel over the *real* socket. The sound server is this
# run's own — see TOPBAR_SMOKE_PULSE in visual-smoke-niri.sh — so the volume
# being changed is a null sink nobody can hear.
set -eu

. "$(dirname "$0")/smoke-shot.sh"

topbar="${SMOKE_TOPBAR:?the runner exports the panel binary}"
scenario="${SMOKE_OSD_SCENARIO:-set}"
art="$SMOKE_ARTIFACTS"

# Run a command, recording what it printed and what it exited with. The
# captured text is the evidence: several scenarios are about the message a
# user sees rather than about a picture.
say() {
  label=$1
  shift
  {
    echo "\$ topbar $*"
    "$topbar" "$@" 2>&1 || echo "[exit $?]"
  } >"$art/$label.txt" 2>&1
  echo "--- $label ---"
  cat "$art/$label.txt"
}

# Whether the area below the bar is one flat colour, i.e. nothing is drawn.
# The same measurement `shot` waits on, asked in the negative.
empty_below_bar() {
  grim "$art/.probe.png"
  colours=$(shot_colours "$art/.probe.png")
  rm -f "$art/.probe.png"
  [ "$colours" -le 1 ]
}

# Wait for the capsule to time out, and prove it did.
wait_until_gone() {
  waited=0
  while [ "$waited" -lt 15 ]; do
    if empty_below_bar; then
      grim "$art/$1.png"
      echo "smoke-osd: the capsule was gone after ${waited}s"
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  grim "$art/$1.png"
  echo "smoke-osd: the capsule was still up after ${waited}s" >&2
  return 1
}

case "$scenario" in
  set)
    # (a) one volume change: the capsule appears, then goes.
    say volume-set volume set 30
    shot capsule-30 topbar-osd
    wait_until_gone capsule-gone
    ;;

  retarget)
    # (b) two changes in quick succession: one capsule, refilled. `niri msg
    # layers` counting exactly one topbar-osd surface is the half a screenshot
    # cannot show.
    "$topbar" volume set 30
    "$topbar" volume set 70
    shot capsule-70 topbar-osd
    niri msg layers >"$art/layers.txt" 2>&1 || true
    surfaces=$(grep -c '"topbar-osd"' "$art/layers.txt" || true)
    echo "topbar-osd surfaces mapped: $surfaces" | tee "$art/surface-count.txt"
    ;;

  mute)
    # (c) muted: the crossed icon and an empty bar.
    say volume-toggle volume toggle-mute
    shot capsule-muted topbar-osd
    ;;

  brightness)
    # (d) `brightness get` only. The backlight is the *host's* — logind is on
    # the system bus, which no sandbox here can box — so setting it would
    # change the screen the developer is looking at. The capsule's brightness
    # path is driven by the debug smoke action instead, which feeds it a
    # synthetic event and touches nothing.
    say brightness-get brightness get
    shot capsule-brightness topbar-osd
    ;;

  inhibit)
    # (e) the icon-only capsule: no bar at all, just the state the toggle
    # landed on. logind is on the *system* bus, which no sandbox here can box,
    # so this takes a real inhibitor for the second or two the session lasts
    # and the kernel drops it when the panel is killed.
    say inhibit-toggle inhibit toggle
    shot capsule-inhibit topbar-osd
    say inhibit-again inhibit toggle
    ;;

  popover)
    # (f) the M3 registry, finally driven by real IPC.
    say popover-show popover show clock
    shot popover-open topbar-popover
    say popover-toggle popover toggle clock
    sleep 2
    niri msg layers >"$art/layers-after-close.txt" 2>&1 || true
    grim "$art/popover-closed.png"
    ;;

  dump)
    # (g) the effective configuration and a summary of every service.
    say dump-config dump config
    say dump-state dump state
    say dump-json dump --json
    say bar-toggle bar toggle
    sleep 1
    grim "$art/bar-hidden.png"
    say bar-show bar show
    sleep 1
    grim "$art/bar-shown.png"
    ;;

  second)
    # (h) a second panel finds the lock taken and says so.
    {
      echo "\$ topbar --config $SMOKE_CONFIG"
      "$topbar" --config "$SMOKE_CONFIG" 2>&1 || echo "[exit $?]"
    } >"$art/second-instance.txt" 2>&1
    echo "--- second-instance ---"
    cat "$art/second-instance.txt"
    grim "$art/still-one-bar.png"
    ;;

  panel-down)
    # (i) the resilience contract: the panel is killed and the volume key still
    # changes the volume, silently, exiting zero.
    kill "$SMOKE_PANEL_PID" 2>/dev/null || true
    sleep 1
    say volume-no-panel volume set 40
    pactl get-sink-volume @DEFAULT_SINK@ >"$art/pactl-after.txt" 2>&1 || true
    echo "--- the sink, according to pactl ---"
    cat "$art/pactl-after.txt"
    # The runner insists on a picture; this one is of a desktop with no panel
    # on it, which is the point of the scenario.
    grim "$art/no-panel.png"
    ;;

  *)
    echo "unknown scenario: $scenario" >&2
    exit 1
    ;;
esac
