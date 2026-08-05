# Synthetic pointer and keyboard for the nested-niri smoke drivers. Source it,
# do not run it: `. "$(dirname "$0")/smoke-pointer.sh"`.
#
# Until this existed there was no way to *click* anything in a smoke run. Every
# popover was opened through TOPBAR_SMOKE_OPEN or `topbar popover show`, which
# dispatch the same action a click would have dispatched — so the whole path
# from "the compositor delivered a button event" to "a GTK gesture fired" was
# never once exercised, on any surface, in any run. Two dismissal bugs shipped
# to a real desktop behind a green suite because of it: the click-catcher never
# closed a popover, and the toggle chevrons never opened their sections.
#
# niri advertises zwlr_virtual_pointer_manager_v1 (v2) and
# zwp_virtual_keyboard_manager_v1, and a nested session advertises them too, so
# `wlrctl` can drive one from the inside. The events go into the nested seat
# directly; the host compositor is not involved and the nested winit window
# does not need to be focused, or even visible.
#
#   pointer_to <x> <y>          park the pointer at a logical coordinate
#   click_at <x> <y> [button]   move there, then click (default left)
#   press_at <x> <y> [button]   move there, then press and hold
#   pointer_release [button]    let go
#   scroll_at <x> <y> <amount>  move there, then scroll
#   type_text <text>            type it on the virtual keyboard
#   key_press <key>             one key by xkb name, e.g. Escape
#
# Coordinates are the nested output in *logical* pixels — which is the pixel
# size of the winit window divided by the `output "winit" { scale }` in the
# config visual-smoke-niri.sh writes. `pointer_size` reports them, and
# `grim` captures in device pixels, so a coordinate read off a screenshot has
# to be scaled before it is clicked. `shot_scale` does that.
#
# Every position is absolute, from a known origin: wlrctl can only move the
# pointer *relatively*, so each move parks it at the top-left corner first by
# asking for a move far larger than any screen, which the compositor clamps to
# the corner. That makes a coordinate mean the same thing on every call,
# whatever the previous one did.
#
# Environment: POINTER_SETTLE (seconds to let a move or a click be processed
# before the next command, 0.3).

POINTER_SETTLE="${POINTER_SETTLE:-0.3}"

# Far enough that any compositor clamps it to the corner of the output.
POINTER_FAR=100000

# Whether synthetic input is usable at all.
pointer_available() {
  command -v wlrctl >/dev/null 2>&1
}

# The nested output, in logical pixels: `<width> <height>`.
#
# The JSON carries the logical rectangle the pointer actually moves in, which
# is what a coordinate handed to `click_at` has to be inside.
pointer_size() {
  niri msg --json outputs 2>/dev/null | python3 -c '
import json, sys
outputs = json.load(sys.stdin)
for output in outputs.values():
    logical = output.get("logical")
    if logical:
        print(logical["width"], logical["height"])
        break
'
}

# Device pixels to logical pixels, for a coordinate read off a screenshot.
#
#   shot_scale 800 600   ->  1066 800   at scale 0.75
shot_scale() {
  niri msg --json outputs 2>/dev/null | python3 -c '
import json, sys
outputs = json.load(sys.stdin)
scale = 1.0
for output in outputs.values():
    logical = output.get("logical")
    if logical:
        scale = logical.get("scale", 1.0) or 1.0
        break
print(round(float(sys.argv[1]) / scale), round(float(sys.argv[2]) / scale))
' "$1" "$2"
}

# Park the pointer at the top-left corner of the output.
pointer_home() {
  wlrctl pointer move -$POINTER_FAR -$POINTER_FAR
}

# Put the pointer at a logical coordinate, from the corner every time.
pointer_to() {
  pointer_home
  # A move of zero is not worth a round trip, and wlrctl treats it as one.
  if [ "$1" -ne 0 ] || [ "$2" -ne 0 ]; then
    wlrctl pointer move "$1" "$2"
  fi
  sleep "$POINTER_SETTLE"
}

# Move there and click. The move comes first because a Wayland button event
# carries no coordinates: what is clicked is whatever the last motion entered.
click_at() {
  pointer_to "$1" "$2"
  wlrctl pointer click "${3:-left}"
  sleep "$POINTER_SETTLE"
}

# Move there, press, and hold — for the hold-to-confirm rows.
press_at() {
  pointer_to "$1" "$2"
  wlrctl pointer click "${3:-left}" state:press
  sleep "$POINTER_SETTLE"
}

# Let go of whatever press_at is holding.
pointer_release() {
  wlrctl pointer click "${1:-left}" state:release
  sleep "$POINTER_SETTLE"
}

# Move there and scroll. A positive amount scrolls down.
scroll_at() {
  pointer_to "$1" "$2"
  wlrctl pointer scroll "$3" 0
  sleep "$POINTER_SETTLE"
}

# Type on the virtual keyboard, into whatever holds the keyboard focus.
type_text() {
  wtype "$1"
  sleep "$POINTER_SETTLE"
}

# One key by its xkb name: Escape, Return, Tab.
key_press() {
  wtype -k "$1"
  sleep "$POINTER_SETTLE"
}

# Whether niri has the named layer surface mapped. Also in smoke-shot.sh; a
# driver that only wants to click does not have to source both.
pointer_mapped() {
  niri msg layers 2>/dev/null | grep -q "\"$1\""
}

# Wait for the named surface to appear, up to POINTER_WAIT seconds.
#
# A popover opened by a click is not on screen the instant the click lands: the
# content is built the first time it is asked for, which on a debug build is
# seconds rather than frames. Sleeping a fixed amount before asserting is the
# same coin toss smoke-shot.sh was written to stop losing.
wait_mapped() {
  waited=0
  while [ "$waited" -lt "${POINTER_WAIT:-20}" ]; do
    pointer_mapped "$1" && return 0
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

# Fail loudly if the named surface never appears.
assert_mapped() {
  if wait_mapped "$1"; then
    echo "smoke-pointer: $1 is mapped${2:+ ($2)}"
    return 0
  fi
  echo "smoke-pointer: $1 is NOT mapped${2:+ ($2)}" >&2
  return 1
}

# Fail loudly if the named surface *is* mapped.
assert_unmapped() {
  if pointer_mapped "$1"; then
    echo "smoke-pointer: $1 is still mapped${2:+ ($2)}" >&2
    return 1
  fi
  echo "smoke-pointer: $1 is gone${2:+ ($2)}"
  return 0
}
