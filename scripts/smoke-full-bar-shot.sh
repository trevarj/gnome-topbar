#!/usr/bin/env sh
# One screenshot of the whole bar, plus the evidence for the two widgets that
# are supposed to be drawing nothing. Driven by scripts/smoke-full-bar.sh.
#
# A screenshot cannot prove an absence, so the quiet widgets are checked in the
# log instead. The panel says how many widgets it built, and the definition of
# done for this milestone is that the number is the number the configuration
# placed — a tray with no items and a healthy system monitor are *built* and
# drawing nothing, which is different from not being built at all.
set -eu

art="$SMOKE_ARTIFACTS"
. "$(dirname "$0")/smoke-shot.sh"

# The bar alone: no popover, so there is nothing below it to wait to be drawn.
# `shot` still waits for two identical frames, which is what lets the crypto
# script, the first weather fetch and the first headset poll all land first.
shot full-bar

niri msg layers >"$art/layers.txt" 2>&1 || true

# Colour codes are in the log, so every pattern here has to survive them.
{
  echo "--- widgets built ---"
  grep -a "widget(s)" "$art/panel.log" || echo "(the bar never reported a count)"

  echo "--- anything the panel refused to build ---"
  if grep -aE "is not a widget|has no section|no script runner|not implemented" \
    "$art/panel.log"; then
    echo "!! a placed widget was skipped"
  else
    echo "(nothing was skipped)"
  fi

  echo "--- what the script printed ---"
  grep -a "custom-crypto" "$art/panel.log" || echo "(nothing)"

  echo "--- what the headset reported ---"
  grep -a "the headset is now" "$art/panel.log" || echo "(nothing)"

  echo "--- the tray, which has no items to show ---"
  grep -a "tray" "$art/panel.log" || echo "(nothing)"

  echo "--- the system monitor, which has nothing to report ---"
  grep -aE "system_monitor|resources" "$art/panel.log" ||
    echo "(quiet, which is the healthy state)"
} >"$art/widgets.txt" 2>&1

cat "$art/widgets.txt"
