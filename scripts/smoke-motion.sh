#!/usr/bin/env sh
# `animations = false` means zero motion, and this is what proves it.
#
#   nix develop -c ./scripts/smoke-motion.sh
#
# The panel counts every animation run that actually registers a tick callback
# and logs it (debug builds only — see anim::animator::count_run). Runs that
# cannot move never reach that line: `Animation::start` jumps them straight to
# their final state. So the assertion is arithmetic rather than visual.
#
#   animations = true   the same drive produces motion
#   animations = false  it produces exactly none, and everything still works:
#                       popovers open fully, banners appear, the capsule shows,
#                       and hold-to-confirm still takes its 650ms
#
# Artifacts land in target/visual-smoke/motion/.
set -eu

trap 'pkill -f "target/debug/topbar-fake-" 2>/dev/null || true' EXIT INT TERM

artifact_root="${1:-target/visual-smoke/motion}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)
repo=$(pwd)

for tool in niri grim cargo timeout dbus-run-session notify-send; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

live_config=crates/topbar-core/tests/fixtures/live-config.toml

config_on="$artifact_root/motion-on.toml"
sed -e 's/^exec = .*/exec = "\/bin\/echo BTC"/' "$live_config" >"$config_on"

config_off="$artifact_root/motion-off.toml"
sed -e 's/^animations = true$/animations = false/' "$config_on" >"$config_off"
grep -q '^animations = false$' "$config_off" || {
  echo "could not switch animations off in the copied config" >&2
  exit 1
}

run() {
  name=$1
  config=$2

  echo "smoke-motion: $name"
  RUST_LOG="info,topbar=debug" \
  TOPBAR_SMOKE_PULSE=1 \
  TOPBAR_SMOKE_POWER="--active balanced --percent 62 --state 2" \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-140}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-motion-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$name" \
    >"$artifact_root/$name.log" 2>&1 ||
    echo "smoke-motion: $name exited non-zero; see $artifact_root/$name.log" >&2
}

run animated "$config_on"
run still "$config_off"

runs() {
  # `grep -c` prints its zero and *then* exits non-zero, so the count is taken
  # from its output alone — an `|| echo 0` here would print the number twice.
  grep -ac "motion: run" "$artifact_root/$1/panel.log" 2>/dev/null || true
}

animated=$(runs animated)
still=$(runs still)

{
  echo "--- animation runs that ticked ---"
  echo "animations = true : $animated"
  echo "animations = false: $still"
  echo

  if [ "$still" -eq 0 ]; then
    echo "PASS: nothing moved with animations off"
  else
    echo "FAIL: $still animation run(s) started with animations off"
    grep -a "motion: run" "$artifact_root/still/panel.log" | head -20
  fi
  if [ "$animated" -gt 0 ]; then
    echo "PASS: the same drive did move with animations on"
  else
    echo "FAIL: nothing moved with animations on either, so the count proves nothing"
  fi
  echo

  echo "--- and everything still happened ---"
  for name in animated still; do
    echo "[$name]"
    grep -a "smoke: held Suspend" "$artifact_root/$name/panel.log" 2>/dev/null ||
      echo "(the hold never ran)"
    grep -a "banner surface\|OSD capsule" "$artifact_root/$name/panel.log" 2>/dev/null |
      tail -3 || echo "(no banner or capsule)"
  done
} >"$artifact_root/report.txt" 2>&1

cat "$artifact_root/report.txt"
echo "--- frames ---"
find "$artifact_root" -name '*.png' | sort
