#!/usr/bin/env sh
# Photographs a press ripple on a panel widget. Driven by
# scripts/smoke-ripple.sh.
#
# Three frames, because the ripple is faint by design and the hover fill it is
# drawn on top of is not: at rest, hovered, and hovered with a press ripple
# part-way through it. The last two differ by the ripple alone.
#
# There is no synthetic pointer in the nested session, and a ripple is over in
# 300ms — far too fast for a helper that waits for a still frame. So the panel
# is asked to paint the frame a press would have produced and leave it there.
set -eu

art="$SMOKE_ARTIFACTS"
. "$(dirname "$0")/smoke-shot.sh"

shot rest

"$SMOKE_TOPBAR" popover show clock-hover >>"$art/ipc.log" 2>&1 || true
sleep 2
shot hover

"$SMOKE_TOPBAR" popover show clock-ripple >>"$art/ipc.log" 2>&1 || true
sleep 2
shot ripple

# The clock's own pill, magnified, and the ripple on its own.
band() {
  magick "$1" -format '%[fx:round(w*0.31)]x40+%[fx:round(w*0.34)]+0' info:
}
for name in rest hover ripple; do
  magick "$art/$name.png" -crop "$(band "$art/$name.png")" +repage \
    -filter point -resize 300% "$art/zoom-$name.png"
done

# The press, isolated: everything the hover already did subtracted away, then
# amplified, because a circle at 12% white over a fill at 10% white is a
# difference of a dozen values out of 255 and no screenshot shows that off.
magick "$art/hover.png" "$art/ripple.png" -compose difference -composite \
  -crop "$(band "$art/hover.png")" +repage -evaluate multiply 8 \
  -filter point -resize 300% "$art/zoom-press.png"

{
  echo "--- the pill, frame by frame ---"
  for name in rest hover ripple; do
    printf '%-8s colours=%-6s mean=%s\n' "$name" \
      "$(magick "$art/$name.png" -crop "$(band "$art/$name.png")" +repage \
        -format '%k' info:)" \
      "$(magick "$art/$name.png" -crop "$(band "$art/$name.png")" +repage \
        -colorspace Gray -format '%[fx:mean*255]' info:)"
  done

  echo
  echo "--- hover against press: the ripple on its own ---"
  magick "$art/hover.png" "$art/ripple.png" -compose difference -composite \
    -crop "$(band "$art/hover.png")" +repage \
    -format 'brightest pixel of the difference: %[fx:maxima*255]
mean difference: %[fx:mean*255]
' info:
} >"$art/ripple.txt" 2>&1

cat "$art/ripple.txt"
