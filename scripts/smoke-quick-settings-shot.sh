#!/usr/bin/env sh
# The Quick Settings driver, run inside the nested niri session by
# scripts/smoke-quick-settings.sh. One scenario per session, named by
# $SMOKE_QS_SCENARIO.
#
# The panel is talking to a UPower and a power-profiles daemon of this run's
# own, on the private session bus (see TOPBAR_SMOKE_POWER in
# visual-smoke-niri.sh). The real ones are on the system bus and are never
# touched: no screenshot is worth changing the developer's charge limit.
#
# Where a scenario needs something a pointer would do, it goes through a debug
# smoke action rather than through the CLI — because the CLI carries
# ChangeSource::Cli, which is exactly the thing the OSD-suppression scenario is
# trying to tell apart from a panel-originated change.
set -eu

. "$(dirname "$0")/smoke-shot.sh"

scenario="${SMOKE_QS_SCENARIO:-panel}"
art="$SMOKE_ARTIFACTS"
sysfs="${SMOKE_POWER_SYSFS:-}"

# The power-profiles fake, driven the way `powerprofilesctl` would drive the
# real one: a plain property write.
set_profile() {
  gdbus call --session \
    --dest org.freedesktop.UPower.PowerProfiles \
    --object-path /org/freedesktop/UPower/PowerProfiles \
    --method org.freedesktop.DBus.Properties.Set \
    org.freedesktop.UPower.PowerProfiles ActiveProfile "<'$1'>" \
    >"$art/set-profile-$1.txt" 2>&1 || echo "[exit $?]" >>"$art/set-profile-$1.txt"
  echo "--- set-profile $1 ---"
  cat "$art/set-profile-$1.txt"
}

# The battery fake's own control interface.
set_battery() {
  gdbus call --session \
    --dest org.freedesktop.UPower \
    --object-path /io/github/trevarj/topbar/FakePower1 \
    --method "io.github.trevarj.topbar.FakePower1.$1" "$2" \
    >"$art/battery-$1.txt" 2>&1 || echo "[exit $?]" >>"$art/battery-$1.txt"
  echo "--- battery $1 $2 ---"
  cat "$art/battery-$1.txt"
}

# What the kernel's own charge-limit files say right now.
show_thresholds() {
  {
    echo "start: $(cat "$sysfs/BAT0/charge_control_start_threshold" 2>&1)"
    echo "end:   $(cat "$sysfs/BAT0/charge_control_end_threshold" 2>&1)"
  } >"$art/$1.txt"
  echo "--- thresholds ($1) ---"
  cat "$art/$1.txt"
}

# Whether an OSD capsule is on screen. `niri msg layers` is the honest test:
# the capsule is a layer surface of its own and it either exists or it does not.
show_osd_surfaces() {
  niri msg layers >"$art/$1-layers.txt" 2>&1 || true
  count=$(grep -c '"topbar-osd"' "$art/$1-layers.txt" || true)
  echo "topbar-osd surfaces mapped: $count" | tee "$art/$1-osd-count.txt"
}

case "$scenario" in
  bar)
    # (a) the button on the bar: the speaker icon and, because a UPower of
    # this run's own is answering, the battery icon beside it.
    shot bar
    ;;

  panel)
    # (b) the panel, opened by TOPBAR_SMOKE_OPEN through the popover registry.
    shot panel topbar-popover
    ;;

  mode)
    # (c) Power Mode expanded: exactly the profiles the daemon reports. Then
    # the daemon is moved from outside — which is what a laptop's own key or
    # `powerprofilesctl` does — and the mark follows it.
    shot mode-balanced topbar-popover
    set_profile performance
    sleep 2
    shot mode-performance topbar-popover
    ;;

  volume)
    # (d) the suppression proof. The smoke action posts the volume with
    # ChangeSource::Ui, exactly as the slider does; the slider and the bar
    # icon both move and no capsule appears. `topbar volume set` would look
    # identical on screen and raise a capsule, which is the distinction.
    shot volume topbar-popover
    show_osd_surfaces volume
    pactl get-sink-volume @DEFAULT_SINK@ >"$art/sink-volume.txt" 2>&1 || true
    echo "--- the sink, according to pactl ---"
    cat "$art/sink-volume.txt"
    ;;

  mic)
    # (e) the microphone slider arrives with the recording and leaves with it.
    shot mic-idle topbar-popover
    parecord --device=topbar_smoke_mic --raw /dev/null >"$art/parecord.log" 2>&1 &
    recorder=$!
    sleep 3
    shot mic-recording topbar-popover
    kill "$recorder" 2>/dev/null || true
    wait "$recorder" 2>/dev/null || true
    sleep 3
    shot mic-stopped topbar-popover
    ;;

  power)
    # (f) the power section: four rows, and one of them painted mid-hold.
    #
    # The fill is painted at a fixed fraction rather than left running. A hold
    # that completed would call logind on the SYSTEM bus — the developer's
    # own — and a fill that was moving would never give `shot` two identical
    # frames to settle on. The genuine press-and-release happens too, on
    # Suspend, and panel.log carries the line proving it cancelled.
    shot power topbar-popover
    ;;

  battery)
    # (g) the health card, and the charge limit actually moving. The write
    # goes to this run's own power-supply tree; the file is read back here.
    show_thresholds thresholds-before
    shot battery-card topbar-popover
    sleep 3
    show_thresholds thresholds-after
    ;;

  low)
    # (h) a battery that is nearly flat and running on itself: the bar icon
    # turns urgent and the pill follows it.
    set_battery SetPercentage 12.0
    set_battery SetState 2
    sleep 2
    shot battery-low topbar-popover
    ;;

  *)
    echo "unknown scenario: $scenario" >&2
    exit 1
    ;;
esac
