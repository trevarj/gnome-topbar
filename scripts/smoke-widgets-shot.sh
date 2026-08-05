#!/usr/bin/env sh
# Drives one M10 widget scenario inside the nested session and photographs it.
# Driven by scripts/smoke-widgets.sh; not useful alone.
#
# Nothing here waits on a clock where it can wait on evidence instead: `shot`
# returns when two consecutive frames are identical, which is how a widget
# fading in over 150ms never lands half-drawn on disk.
set -eu

art="$SMOKE_ARTIFACTS"
scenario="${SMOKE_WIDGET_SCENARIO:-plain}"
. "$(dirname "$0")/smoke-shot.sh"

# What the widget row looks like right now, whatever is or is not in it. The
# bar is the only surface in these runs, so a plain capture is the whole story.
frame() {
  grim "$art/$1.png"
  echo "smoke-widgets: $1 captured"
}

# The bar with nothing moving in it.
still() {
  shot "$1"
}

case "$scenario" in
  monitor)
    # Healthy: the widget is not on the bar at all.
    still monitor-hidden

    # One spinner, from SMOKE_PATH, for six seconds. `cpu_threshold` is 5 in
    # this scenario's config, so one busy shell crosses it — saturating the
    # developer's cores against the real threshold of 90 is not on. It stops on
    # its own deadline as well as being killed here.
    topbar-smoke-spinner 6 &
    spinner=$!
    trap 'kill "$spinner" 2>/dev/null || true' EXIT INT TERM

    # Two samples at one second each to cross, plus the fade.
    sleep 3
    frame monitor-warning

    wait "$spinner" 2>/dev/null || true
    # Two samples below threshold − 5 to let go again.
    sleep 4
    still monitor-gone
    ;;

  headset)
    still headset-discharging

    cat >"$SMOKE_HEADSET_STATE" <<'JSON'
{"devices":[{"status":"success","device":"Arctis Nova 7",
  "battery":{"status":"BATTERY_CHARGING","level":45}}]}
JSON
    # The poll interval is the live config's five seconds.
    sleep 7
    still headset-charging

    # The tool goes away, which is what an unplugged dongle or an uninstalled
    # headsetcontrol looks like from the panel's side.
    rm -f "$SMOKE_HEADSET_TOOL"
    sleep 7
    still headset-gone
    ;;

  offline)
    # NetworkManager says DISCONNECTED. The optimistic first run has already
    # happened — that is deliberate, see the parent script — so the label reads
    # "run 1", and the point is that ten seconds of three-second ticks leave it
    # reading "run 1".
    sleep 10
    still custom-offline
    grep -a "the run is deferred" "$art/panel.log" >"$art/deferred.txt" 2>&1 || true
    echo "--- ticks deferred while offline ---"
    wc -l <"$art/deferred.txt"

    # Reconnect, the way `nmcli` would from outside the panel.
    gdbus call --session \
      --dest org.freedesktop.NetworkManager \
      --object-path /io/github/trevarj/topbar/FakeNm1 \
      --method io.github.trevarj.topbar.FakeNm1.SetState 70 \
      >"$art/reconnect.txt" 2>&1 || echo "[exit $?]" >>"$art/reconnect.txt"

    # A plain frame rather than a settled one: the widget resumes its normal
    # three-second schedule the moment it is back, so no two frames are ever
    # identical again and `shot` would wait for a stillness that cannot come.
    # What the frame has to show is that the counter moved at all; that it
    # moved exactly once *on the reconnect* is the log line below, which must
    # appear exactly once however many ordinary ticks follow it.
    sleep 2
    frame custom-online
    grep -a "back online" "$art/panel.log" >"$art/fired.txt" 2>&1 || true
    echo "--- runs fired by the reconnect ---"
    wc -l <"$art/fired.txt"
    ;;

  *)
    still "custom-$scenario"
    ;;
esac

niri msg layers >"$art/layers.txt" 2>&1 || true

{
  echo "--- what the panel built ---"
  grep -a "widget(s)" "$art/panel.log" || echo "(no count)"
  echo "--- the scripts and the readings ---"
  grep -aE "custom-|the headset is now|offline|back online" "$art/panel.log" ||
    echo "(nothing)"
} >"$art/notes.txt" 2>&1
cat "$art/notes.txt"
