#!/usr/bin/env sh
# The driver for scripts/smoke-input.sh. Run inside the nested session.
#
# Everything here is done with a synthetic pointer or keyboard, never with
# TOPBAR_SMOKE_OPEN or `topbar popover show`. That is the whole point: those
# dispatch the same action a click would have dispatched and skip the part that
# was broken, which is how the panel shipped with click-away dismissal and the
# toggle chevrons both dead.
#
# Everything this prints lands in the run's driver.log, which is what
# smoke-input.sh reads the verdict out of. Nothing is written to a file of this
# script's own on purpose: `shot` uses a variable called `out` for the PNG it is
# taking, and a driver that kept its report path in a variable of the same name
# quietly appended the rest of its report to a screenshot.
#
# Coordinates are logical pixels on the nested output, which is 1224x1317 at
# the scale visual-smoke-niri.sh writes. They are checked rather than assumed:
# a click at the wrong coordinate lands on nothing and looks exactly like the
# bug this run exists to catch.
set -eu

. "$(dirname "$0")/smoke-pointer.sh"
. "$(dirname "$0")/smoke-shot.sh"

scenario="${SMOKE_INPUT_SCENARIO:-click}"

fail=0
# Run a check, remember a failure, and never stop the run: a driver that exits
# at the first failure photographs nothing after it, and the screenshot of what
# went wrong is the most useful thing it could have left behind.
check() {
  "$@" || fail=1
}

# The bar button that opens Quick Settings, and the Wi-Fi pill chevron. Both
# measured on the 1224x1317 output with the live config.
qs_button_x=1128
qs_button_y=18
wifi_chevron_x=1005
wifi_chevron_y=228
# Well clear of the bar and of the panel, which is anchored top-right.
desktop_x=400
desktop_y=900
# Inside the panel, on the System card, where a click changes nothing but does
# hand the surface the keyboard focus it takes on demand.
panel_x=1035
panel_y=690

echo "=== output: $(pointer_size) ==="
if [ "$(pointer_size)" != "1224 1317" ]; then
  echo "smoke-input: the output is not the size these coordinates were measured"
  echo "on, so the clicks below would land somewhere nobody meant. Re-measure."
  exit 1
fi

case "$scenario" in
  click)
    # 1. The panel opens from a real pointer click on a real bar button. If
    #    this fails nothing below means anything, so it is checked first.
    echo "--- 1. click the Quick Settings button on the bar"
    click_at "$qs_button_x" "$qs_button_y"
    check assert_mapped topbar-popover "opened by a click"
    check assert_mapped topbar-click-catcher "the catcher came up with it"
    check shot 1-panel topbar-popover

    # 2. The chevron opens the network list, and the pill body does NOT switch
    #    the radio: the two halves of a split pill mean different things, and a
    #    chevron nested inside the body button fires the body instead. The
    #    screenshot is the evidence the list came down; smoke-input.sh greps
    #    the panel log for the radio change that must not have happened.
    echo "--- 2. click the Wi-Fi chevron"
    click_at "$wifi_chevron_x" "$wifi_chevron_y"
    sleep 2
    check shot 2-wifi-list

    # 3. A click anywhere else dismisses. This is the one the catcher exists
    #    for, and it needs the catcher to be a *mapped* surface — a layer
    #    surface with no buffer on it is listed by `niri msg layers` and is
    #    still not in the compositor's input routing at all.
    echo "--- 3. click the desktop"
    click_at "$desktop_x" "$desktop_y"
    sleep 2
    check assert_unmapped topbar-popover "click-away dismissed it"
    check assert_unmapped topbar-click-catcher "and took the catcher with it"
    check shot 3-dismissed

    # 4. The bar stays live underneath: the catcher asks for an exclusive zone
    #    of zero so the compositor leaves the bar uncovered, and the button
    #    that opened a popover has to be able to close it again.
    echo "--- 4. click the same bar button twice, open then closed"
    click_at "$qs_button_x" "$qs_button_y"
    check assert_mapped topbar-popover "reopened from the bar"
    click_at "$qs_button_x" "$qs_button_y"
    sleep 2
    check assert_unmapped topbar-popover "and toggled shut from the bar"
    ;;

  escape)
    # Escape has to keep working, and it is the one dismissal that was never
    # broken — so it is the control in this experiment.
    echo "--- open with a click, dismiss with Escape"
    click_at "$qs_button_x" "$qs_button_y"
    check assert_mapped topbar-popover "opened by a click"
    # The popover takes the keyboard on demand, so it has to be clicked before
    # it holds focus: a key pressed at a surface that has none goes to whatever
    # does.
    click_at "$panel_x" "$panel_y"
    sleep 1
    key_press Escape
    sleep 2
    check assert_unmapped topbar-popover "Escape dismissed it"
    check shot escape-dismissed
    ;;
esac

if [ "$fail" -eq 0 ]; then
  echo "--- result: PASS"
else
  echo "--- result: FAIL"
fi
exit "$fail"
