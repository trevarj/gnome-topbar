#!/usr/bin/env sh
# The driver for scripts/smoke-qs-pointer.sh. Run inside the nested session.
#
# Every control in Quick Settings, pressed by a synthetic pointer, at the scale
# a real display runs at. Nothing here uses TOPBAR_SMOKE_OPEN to reach a block
# inside the panel: those dispatch the action a click would have dispatched and
# skip the part that keeps turning out to be broken.
#
# Coordinates are never hardcoded. The panel is asked where its controls are —
# `topbar popover show quick-settings-dump` logs a rectangle per control, in
# monitor pixels, with the text on it — and `locate` reads the last dump out of
# panel.log. A driver holding coordinates measured off a screenshot starts
# clicking empty space the first time a padding changes, which looks exactly
# like the dead-control bugs this run exists to catch.
#
# Nothing in here can act on the machine the run is inside:
#
#   - the power rows go to logind on the *system* bus, which no test can put a
#     stand-in in front of, so `Power` refuses every action in a development
#     build (topbar-services/src/power.rs). The hold below therefore runs to
#     completion on purpose, and what lands on screen is the refusal;
#   - the lock button runs a command, so the run is given a config whose lock
#     command is a path that does not exist. `loginctl lock-session` would lock
#     the developer's screen;
#   - NetworkManager and BlueZ are the run's own fakes, on its private bus.
set -eu

. "$(dirname "$0")/smoke-pointer.sh"
. "$(dirname "$0")/smoke-shot.sh"

scenario="${SMOKE_QS_POINTER_SCENARIO:-controls}"

fail=0
# Run a check, remember a failure, and never stop the run: a driver that exits
# at the first failure photographs nothing after it, and the screenshot of what
# went wrong is the most useful thing it could have left behind.
check() {
  "$@" || fail=1
}

# Ask the panel where everything is. Run it after anything that moves a
# control; every coordinate below comes out of the answer.
#
# The wait before asking is not politeness. The nested session renders in
# software and is throttled by the host compositor, so a click can take seconds
# to become a laid-out panel — and a dump taken before that is a dump of the
# panel as it was *before* the click, which had the driver clicking one step
# behind itself for a whole run. The wait after is on evidence rather than the
# clock: the panel has to have finished writing a new block.
dump() {
  sleep "${DUMP_SETTLE:-2}"
  before=$(grep -c "qs-dump: end" "$SMOKE_ARTIFACTS/panel.log" 2>/dev/null || true)
  "$SMOKE_TOPBAR" popover show quick-settings-dump >/dev/null 2>&1 || true
  waited=0
  while [ "$waited" -lt 20 ]; do
    if [ "$(grep -c "qs-dump: end" "$SMOKE_ARTIFACTS/panel.log" 2>/dev/null || true)" -gt "$before" ]; then
      return 0
    fi
    sleep 0.5
    waited=$((waited + 1))
  done
  echo "smoke-qs-pointer: the panel never answered a dump" >&2
  return 1
}

# One control's rectangle, as `<x> <y> <w> <h>`, from the last dump.
#
#   rect_of Caffeine            the pill with that word on it
#   rect_of qs-toggle-expand 2  the second chevron
#   rect_of bar-button          the button on the bar
#
# The pattern is matched against "<GtkType> <classes> <label>", so a class, a
# widget type and the text on a control are all usable and any of them matches
# wherever it appears. Fails loudly when there is no such control, which is a
# failure worth having: it means the thing being clicked is not on screen.
rect_of() {
  python3 - "$SMOKE_ARTIFACTS/panel.log" "$1" "${2:-1}" <<'PY'
import re, sys

log, pattern, index = sys.argv[1], sys.argv[2], int(sys.argv[3])
text = re.sub(r"\x1b\[[0-9;]*m", "", open(log, errors="replace").read())

# The last dump only: the panel is dumped again after every step.
start = text.rfind("qs-dump: begin")
if start < 0:
    sys.exit("no dump in the log")
end = text.find("qs-dump: end", start)
block = text[start : end if end > 0 else len(text)]

line_re = re.compile(
    r'qs-dump: (\S+) \[([^\]]*)\] "([^"]*)" (-?\d+) (-?\d+) (\d+) (\d+)'
)
found = []
for line in block.splitlines():
    match = line_re.search(line)
    if not match:
        continue
    kind, classes, label = match.group(1), match.group(2), match.group(3)
    haystack = kind + " " + classes.replace(".", " ") + " " + label
    if pattern in haystack:
        found.append(match.group(4, 5, 6, 7))

if len(found) < index:
    sys.exit("%s: wanted #%d, found %d" % (pattern, index, len(found)))
print(*found[index - 1])
PY
}

# The centre of a control, as `<x> <y>`.
centre_of() {
  rect=$(rect_of "$@") || return 1
  # shellcheck disable=SC2086
  set -- $rect
  echo "$(($1 + $3 / 2)) $(($2 + $4 / 2))"
}

# Click a control by name. Located first, so a control that moved is still hit
# and one that vanished is a loud failure rather than a click on the wall.
click_on() {
  where=$(centre_of "$1" "${2:-1}") || {
    echo "smoke-qs-pointer: cannot find $1 #${2:-1}" >&2
    return 1
  }
  echo "smoke-qs-pointer: click $1 #${2:-1} at $where"
  # shellcheck disable=SC2086
  click_at $where
}

# Park the pointer on a control without pressing it, for a hover screenshot.
hover_on() {
  where=$(centre_of "$1" "${2:-1}") || {
    echo "smoke-qs-pointer: cannot find $1 #${2:-1}" >&2
    return 1
  }
  echo "smoke-qs-pointer: hover $1 #${2:-1} at $where"
  # shellcheck disable=SC2086
  pointer_to $where
}

# Scroll over a control, rather than at a coordinate off a ruler.
scroll_on() {
  where=$(centre_of "$1" "${2:-1}") || return 1
  # shellcheck disable=SC2086
  scroll_at $where "$3"
}

# Off the panel entirely, so a screenshot is not taken with the pointer sitting
# on a hover state. The panel is anchored top-right; the far left is desktop.
pointer_park() {
  pointer_to 40 940
}

# Click `<percent>` of the way along a control's track: a GtkScale jumps to
# where it was clicked, which is how a slider is set without dragging one.
click_along() {
  what=$1
  percent=$2
  rect=$(rect_of "$what" "${3:-1}") || return 1
  # shellcheck disable=SC2086
  set -- $rect
  x=$(($1 + $3 * percent / 100))
  y=$(($2 + $4 / 2))
  echo "smoke-qs-pointer: click $what at $percent% of its track ($x $y)"
  click_at "$x" "$y"
}

# The accordion's rule: at most one expandable section is open. Each of them
# contributes a control only it has, so counting those counts open sections.
assert_one_expandable() {
  open=""
  for kind in qs-network-row GtkSwitch qs-vpn-row qs-radio-row qs-limit-button qs-power-row; do
    if rect_of "$kind" 1 >/dev/null 2>&1; then
      open="$open $kind"
    fi
  done
  count=$(echo $open | wc -w)
  if [ "$count" -le 1 ]; then
    echo "smoke-qs-pointer: one expandable open ($open)"
    return 0
  fi
  echo "smoke-qs-pointer: $count expandables are open at once:$open" >&2
  return 1
}

echo "=== output: $(pointer_size) ==="
if [ "$(pointer_size)" != "918 988" ]; then
  echo "smoke-qs-pointer: this run wants the nested output at scale 1.0, where a"
  echo "logical pixel is a device pixel and a screenshot coordinate is a pointer"
  echo "coordinate. Set TOPBAR_SMOKE_SCALE=1.0."
  exit 1
fi

# The panel has to be built before it can say where anything is, and the bar
# button is the only control on screen before it is. One dump, one click, and
# from there every coordinate comes out of the panel itself.
open_panel() {
  dump
  check click_on bar-button
  check assert_mapped topbar-popover "opened by a click on the bar"
  dump
}

case "$scenario" in
  # The header: the battery pill, both charge-limit buttons, the lock button
  # and the power section's four rows.
  header)
    echo "--- open"
    open_panel
    pointer_park
    check shot 01-open topbar-popover

    echo "--- the battery pill opens the health card"
    check hover_on qs-battery-pill
    check shot 02-pill-hover
    check click_on qs-battery-pill
    dump
    pointer_park
    check shot 03-battery-card

    echo "--- both charge-limit buttons"
    check click_on qs-limit-button 2
    dump
    pointer_park
    check shot 04-limit-80
    check click_on qs-limit-button 1
    dump
    pointer_park
    check shot 05-limit-full

    echo "--- the lock button reports inline (its command does not exist)"
    check click_on qs-round-button 1
    sleep 2
    pointer_park
    check shot 06-lock-error

    echo "--- the power button opens the power section"
    check click_on qs-round-button 2
    dump
    check assert_one_expandable
    pointer_park
    check shot 07-power-section

    echo "--- hover a power row"
    check hover_on qs-power-row 1
    check shot 08-power-hover

    echo "--- press and let go of Shut Down: an early release must cancel"
    # Not a hold. Every wlrctl call is its own Wayland client and the button
    # comes back up when it exits, so this is a press and a release with a
    # third of a second between them — which is exactly the contract worth
    # checking here, because a row that fired on a press would be a row that
    # shut the machine down by being brushed against.
    if where=$(centre_of "Shut Down"); then
      # shellcheck disable=SC2086
      press_at $where
      pointer_release
      sleep 2
      pointer_park
      check shot 09-press-cancelled
    else
      fail=1
    fi

    echo "--- and hold it properly, from the keyboard, until it fires"
    # The keyboard is the one synthetic input that can hold something down:
    # wtype takes press, wait and release in a single invocation. The popover
    # has had the keyboard since the first click landed on it, and focus is on
    # the power button that opened the section, so one Tab reaches the first
    # row inside it.
    key_press Tab
    pointer_park
    check shot 10-focus-on-a-row
    hold_key Return 900
    sleep 2
    pointer_park
    check shot 11-hold-fired
    ;;

  # The sliders block and the two plain pills.
  sliders)
    echo "--- open"
    open_panel
    pointer_park
    check shot 01-open

    echo "--- the output slider jumps to where it is clicked"
    check click_along GtkScale 25 1
    dump
    pointer_park
    check shot 02-volume-quarter
    check click_along GtkScale 80 1
    dump
    pointer_park
    check shot 03-volume-loud

    echo "--- the speaker icon mutes, and unmutes"
    check click_on qs-slider-icon 1
    dump
    pointer_park
    check shot 04-muted
    check click_on qs-slider-icon 1
    dump

    echo "--- the chooser chevron opens the output list"
    check hover_on qs-chooser
    check shot 05-chooser-hover
    check click_on qs-chooser
    dump
    pointer_park
    check shot 06-output-list
    echo "--- and a row in it picks an output"
    check click_on qs-device-row 1
    dump
    pointer_park
    check shot 07-output-picked
    check click_on qs-chooser
    dump

    echo "--- Caffeine, on and off"
    check click_on Caffeine
    dump
    pointer_park
    check shot 08-caffeine-on
    check click_on Caffeine
    dump
    pointer_park
    check shot 09-caffeine-off
    ;;

  # The Wi-Fi list: rows, the password box, scrolling, and the radio switch.
  wifi)
    echo "--- open"
    open_panel
    pointer_park
    check shot 01-open

    echo "--- the chevron opens the list; the body must not switch the radio"
    check click_on qs-toggle-expand 1
    dump
    pointer_park
    check shot 02-wifi-list

    echo "--- hover a network row, then join the open one"
    check hover_on Airport
    check shot 03-network-hover
    check click_on Airport
    dump
    pointer_park
    check snap 04-network-joining

    echo "--- a secured network asks for a password"
    check click_on Cafe
    sleep 3
    dump
    pointer_park
    check snap 05-password-prompt
    type_text "wrong-key-on-purpose"
    check snap 06-password-typed
    echo "--- and Connect sends it, which the fake refuses"
    check click_on Connect
    sleep 3
    dump
    pointer_park
    check snap 07-password-refused
    check click_on Cancel
    sleep 2
    dump

    echo "--- scrolling: twelve networks is taller than the panel may be"
    check scroll_on qs-network-row 1 5
    check shot 08-scrolled
    check scroll_on qs-network-row 1 -5
    check shot 09-scrolled-back

    echo "--- the pill body switches the radio off, which closes the list"
    check click_on qs-toggle 1
    sleep 3
    dump
    pointer_park
    check shot 10-wifi-off
    check click_on qs-toggle 1
    sleep 3
    dump
    ;;

  # Bluetooth, VPN and Power Mode.
  devices)
    echo "--- open"
    open_panel
    pointer_park
    check shot 01-open

    echo "--- the Bluetooth chevron opens the device list"
    check click_on qs-toggle-expand 2
    dump
    check assert_one_expandable
    pointer_park
    check shot 02-bluetooth-list

    echo "--- a device switch connects"
    check click_on GtkSwitch 1
    sleep 2
    dump
    pointer_park
    check snap 03-bluetooth-connecting
    sleep 6
    dump
    pointer_park
    check shot 04-bluetooth-settled

    echo "--- opening the VPN list closes the Bluetooth one"
    check click_on qs-toggle-expand 3
    dump
    check assert_one_expandable
    pointer_park
    check shot 05-vpn-list

    echo "--- a VPN row switches its tunnel"
    check click_on qs-vpn-row 1
    sleep 3
    dump
    pointer_park
    check snap 06-vpn-up

    echo "--- Power Mode expands from its body, not only its chevron"
    check click_on "Power Mode"
    dump
    check assert_one_expandable
    pointer_park
    check shot 07-power-mode
    echo "--- and each radio row picks a profile"
    check click_on qs-radio-row 1
    dump
    pointer_park
    check shot 08-power-saver
    check click_on qs-radio-row 3
    dump
    pointer_park
    check shot 09-performance
    ;;
esac

echo "--- dismiss"
click_at 200 940
sleep 2
check assert_unmapped topbar-popover "click-away dismissed it"

if [ "$fail" -eq 0 ]; then
  echo "--- result: PASS"
else
  echo "--- result: FAIL"
fi
exit "$fail"
