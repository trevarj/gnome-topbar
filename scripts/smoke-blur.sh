#!/usr/bin/env sh
# Blur, A against B.
#
#   nix develop -c ./scripts/smoke-blur.sh
#
# Runs the panel three times over the same busy backdrop and photographs the
# same surfaces each time:
#
#   on          theme.blur = true, the live configuration's own setting
#   config-off  theme.blur = false
#   env-off     theme.blur = true with TOPBAR_NO_BLUR set, which is the
#               "the compositor cannot do this" path taken deliberately
#
# Then it measures the three sets of frames against one another. The claim
# being tested is not "the pixels differ" — a screenshot always differs from
# another screenshot — but "the desktop showing through the popover is
# *smeared* in one and *sharp* in the others", so the numbers reported are
# edge energy and colour count inside the popover, plus the RMSE between the
# runs. Blur takes edges away; nothing else here does.
#
# Artifacts land in target/visual-smoke/blur/.
set -eu

trap 'pkill -f "target/debug/topbar-fake-" 2>/dev/null || true' EXIT INT TERM

artifact_root="${1:-target/visual-smoke/blur}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)
repo=$(pwd)

for tool in magick niri grim cargo timeout dbus-run-session notify-send gtk4-demo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

live_config=crates/topbar-core/tests/fixtures/live-config.toml

# The live configuration with the network off the bar: this scenario is about
# surfaces, and a run with no NetworkManager behind it spends its first seconds
# reporting that rather than drawing. Blur stays exactly as the user has it.
config_on="$artifact_root/blur-on.toml"
sed -e 's/^exec = .*/exec = "\/bin\/echo BTC"/' "$live_config" >"$config_on"

config_off="$artifact_root/blur-off.toml"
sed -e 's/^blur = true$/blur = false/' "$config_on" >"$config_off"
grep -q '^blur = false$' "$config_off" || {
  echo "could not switch blur off in the copied config" >&2
  exit 1
}

# One run of the panel, with whatever the scenario needs in its environment.
run() {
  name=$1
  config=$2
  no_blur=$3

  echo "smoke-blur: $name"
  RUST_LOG="info,topbar=debug" \
  TOPBAR_NO_BLUR="$no_blur" \
  TOPBAR_SMOKE_PULSE=1 \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-120}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-blur-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$name" \
    >"$artifact_root/$name.log" 2>&1 ||
    echo "smoke-blur: $name exited non-zero; see $artifact_root/$name.log" >&2
}

run on "$config_on" ""
run config-off "$config_off" ""
run env-off "$config_on" 1

# --- measuring -------------------------------------------------------------
#
# A band inside the control panel, clear of the bar above it and of the screen
# edges. The layout is identical in all three runs — only blur differs — so the
# same rectangle covers the same content in each.
# The band, as fractions of the frame: `magick -crop` reads a percentage size
# but takes its offsets in pixels, so the rectangle is worked out per frame
# rather than written as a percentage geometry — which silently cropped the
# top-left corner of the screen the first time this was written.
band() {
  magick "$1" -format \
    "%[fx:round(w*0.28)]x%[fx:round(h*0.32)]+%[fx:round(w*0.237)]+%[fx:round(h*0.106)]" \
    info:
}

# How much high-frequency detail survives in a crop. A Laplacian answers with
# zero on flat areas and swings hard at every edge, so its standard deviation
# over the region is a direct measure of how sharp what is behind the surface
# still looks.
edges() {
  magick "$1" -crop "$(band "$1")" +repage -colorspace Gray \
    -morphology Convolve Laplacian:0 -format "%[fx:standard_deviation*1000]" info:
}

colours() {
  magick "$1" -crop "$(band "$1")" +repage -format "%k" info:
}

difference() {
  magick compare -metric RMSE "$1" "$2" null: 2>&1 | sed 's/ .*//'
}

{
  echo "--- the popover band, run by run ---"
  printf '%-12s %-14s %-10s\n' run edge-energy colours
  for name in on config-off env-off; do
    frame="$artifact_root/$name/panel.png"
    [ -f "$frame" ] || {
      printf '%-12s %s\n' "$name" "(no frame)"
      continue
    }
    printf '%-12s %-14s %-10s\n' "$name" "$(edges "$frame")" "$(colours "$frame")"
  done

  echo
  echo "--- how far apart the frames are (RMSE, whole screen) ---"
  if [ -f "$artifact_root/on/panel.png" ] && [ -f "$artifact_root/config-off/panel.png" ]; then
    echo "on vs config-off: $(difference "$artifact_root/on/panel.png" \
      "$artifact_root/config-off/panel.png")"
  fi
  if [ -f "$artifact_root/config-off/panel.png" ] && [ -f "$artifact_root/env-off/panel.png" ]; then
    echo "config-off vs env-off: $(difference "$artifact_root/config-off/panel.png" \
      "$artifact_root/env-off/panel.png") (both unblurred: expected to be small)"
  fi

  echo
  echo "--- what each run said about blur ---"
  for name in on config-off env-off; do
    echo "[$name]"
    grep -a "blur:\|topbar is running" "$artifact_root/$name/panel.log" 2>/dev/null ||
      echo "(no log)"
  done
} >"$artifact_root/report.txt" 2>&1

# The crops themselves, so the difference can be looked at rather than only
# read about.
for name in on config-off env-off; do
  frame="$artifact_root/$name/panel.png"
  [ -f "$frame" ] || continue
  magick "$frame" -crop "$(band "$frame")" +repage "$artifact_root/crop-$name.png"
done

cat "$artifact_root/report.txt"
echo "--- frames ---"
find "$artifact_root" -name '*.png' | sort
