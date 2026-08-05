#!/usr/bin/env sh
# Drives every animated surface the panel has and counts what moved. Run twice
# by scripts/smoke-motion.sh, once with `animations = true` and once with it
# false; the second run is the one that matters.
set -eu

art="$SMOKE_ARTIFACTS"
. "$(dirname "$0")/smoke-shot.sh"

open_close() {
  "$SMOKE_TOPBAR" popover show "$1" >>"$art/ipc.log" 2>&1 || true
  shot "$2" topbar-popover || true
  "$SMOKE_TOPBAR" popover hide >>"$art/ipc.log" 2>&1 || true
  sleep 2
}

# The control panel and Quick Settings: open, photographed, closed. With motion
# off both have to be fully drawn in the frame rather than caught part-way
# through a grow-in that never happens.
open_close clock panel
open_close quick_settings quick-settings

# Hold to confirm, which keeps its 650ms wait whatever animations are set to.
# The smoke action presses and releases without firing anything.
"$SMOKE_TOPBAR" popover show quick-settings-power >>"$art/ipc.log" 2>&1 || true
sleep 4
"$SMOKE_TOPBAR" popover hide >>"$art/ipc.log" 2>&1 || true
sleep 1

# A banner and the capsule, the two surfaces that animate without a popover.
notify-send -t 12000 "Motion" "A banner either slides in or is simply there." || true
sleep 3
"$SMOKE_TOPBAR" volume set 40 >>"$art/ipc.log" 2>&1 || true
sleep 3

{
  echo "--- what the panel was told about motion ---"
  grep -a "topbar is running" "$art/panel.log" || echo "(no start-up line)"

  echo "--- animation runs that actually ticked ---"
  runs=$(grep -ac "motion: run" "$art/panel.log" || true)
  echo "count: $runs"
  grep -a "motion: run" "$art/panel.log" | tail -20 || true

  echo "--- hold to confirm ---"
  grep -a "smoke: held Suspend" "$art/panel.log" || echo "(the hold never ran)"

  echo "--- did anything fire ---"
  grep -aiE "suspend|power off|reboot" "$art/panel.log" | grep -av "smoke: held" ||
    echo "(nothing did, which is the point)"
} >"$art/motion.txt" 2>&1

cat "$art/motion.txt"
