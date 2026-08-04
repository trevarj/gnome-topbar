# Screenshot helper for the nested-niri smoke drivers. Source it, do not run
# it: `. "$(dirname "$0")/smoke-shot.sh"`.
#
# `grim` hands back the last frame the nested niri *presented*, and a winit
# session inside a window nobody is looking at is throttled by the host
# compositor — so the frame on disk can be seconds behind the frame the panel
# has drawn. A fixed `sleep` before `grim` is therefore a coin toss, and it has
# come up tails twice now: once for the media card, once for the weather
# popover, both times producing a screenshot of the bar alone while
# `niri msg layers` swore the popover was mapped.
#
# So this waits on evidence instead of on the clock:
#
#   1. if a layer namespace is named, `niri msg layers` has to list it;
#   2. the area below the bar has to actually have something drawn in it —
#      the nested session's background is one flat colour, so "more than one
#      colour down there" is exactly "a surface has been presented";
#   3. two consecutive captures have to be byte-identical, which is how the
#      open animation is allowed to finish and how a dialog still showing
#      "Searching…" is not what ends up on disk.
#
# A capture that never satisfies all three fails loudly rather than leaving a
# stale frame for someone to describe as though it were the real thing.
#
#   shot <name>                     the bar alone, or whatever is on screen
#   shot <name> <layer-namespace>   wait for that surface to be drawn
#
#     topbar-popover        any widget's popover
#     topbar-dialog         the weather location dialog
#
# Environment: SHOT_TIMEOUT (seconds, 30), SHOT_INTERVAL (seconds, 1),
# SHOT_MIN (seconds before a still frame counts as settled, 3), SHOT_BAR
# (pixels of bar to ignore at the top, 45).

SHOT_TIMEOUT="${SHOT_TIMEOUT:-30}"
SHOT_INTERVAL="${SHOT_INTERVAL:-1}"
SHOT_MIN="${SHOT_MIN:-3}"
SHOT_BAR="${SHOT_BAR:-45}"

# How many colours are used below the bar. One means nothing is drawn there.
shot_colours() {
  magick "$1" -crop "100%x100%+0+$SHOT_BAR" +repage -format "%k" info: 2>/dev/null || echo 0
}

# Whether niri has the named layer surface mapped.
shot_mapped() {
  niri msg layers 2>/dev/null | grep -q "\"$1\""
}

shot() {
  name=$1
  expect=${2:-}
  out="$SMOKE_ARTIFACTS/$name.png"
  next="$SMOKE_ARTIFACTS/.$name.next.png"

  waited=0
  drawn=0
  settled=0

  # A surface the compositor has not been told about yet cannot be in a frame.
  if [ -n "$expect" ]; then
    while [ "$waited" -lt "$SHOT_TIMEOUT" ]; do
      shot_mapped "$expect" && break
      sleep "$SHOT_INTERVAL"
      waited=$((waited + SHOT_INTERVAL))
    done
    if ! shot_mapped "$expect"; then
      echo "smoke-shot: $name: no '$expect' surface after ${waited}s" >&2
      grim "$out" 2>/dev/null || true
      return 1
    fi
  fi

  grim "$out"
  while [ "$waited" -lt "$SHOT_TIMEOUT" ]; do
    # Nothing below the bar yet: the popover is mapped but its first frame has
    # not reached the screen. Keep asking.
    if [ -n "$expect" ] && [ "$drawn" -eq 0 ]; then
      if [ "$(shot_colours "$out")" -le 1 ]; then
        sleep "$SHOT_INTERVAL"
        waited=$((waited + SHOT_INTERVAL))
        grim "$out"
        continue
      fi
      drawn=1
    fi

    sleep "$SHOT_INTERVAL"
    waited=$((waited + SHOT_INTERVAL))
    grim "$next"

    if cmp -s "$out" "$next"; then
      rm -f "$next"
      # A frame that has not moved for one interval is settled — but never
      # before SHOT_MIN, because the very first capture of a run is usually a
      # frame the compositor presented before the panel had finished drawing,
      # and two of those in a row look just as still as a finished one.
      if [ "$waited" -ge "$SHOT_MIN" ]; then
        settled=1
        break
      fi
    else
      settled=0
      mv "$next" "$out"
    fi
  done
  rm -f "$next"

  if [ -n "$expect" ] && [ "$drawn" -eq 0 ]; then
    echo "smoke-shot: $name: '$expect' was mapped but never drawn in ${waited}s" >&2
    return 1
  fi
  if [ "$settled" -lt 1 ]; then
    echo "smoke-shot: $name: the frame was still changing after ${waited}s" >&2
    return 1
  fi

  echo "smoke-shot: $name settled after ${waited}s"
}
