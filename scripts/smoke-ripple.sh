#!/usr/bin/env sh
# A press ripple, photographed.
#
#   nix develop -c ./scripts/smoke-ripple.sh
#
# The ripple is the one piece of M11 that no ordinary smoke run can catch: it
# needs a pointer the nested session does not have, and it is over in 300ms.
# A debug-only hook paints the frame a press would have produced a third of the way
# through and leaves it on screen, which is the same trick the power card uses
# to photograph its hold fill.
#
# Artifacts land in target/visual-smoke/ripple/.
set -eu

artifact_root="${1:-target/visual-smoke/ripple}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)
repo=$(pwd)

for tool in magick niri grim cargo timeout dbus-run-session; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

config="$artifact_root/ripple.toml"
sed -e 's/^exec = .*/exec = "\/bin\/echo BTC"/' \
  crates/topbar-core/tests/fixtures/live-config.toml >"$config"

RUST_LOG="info,topbar=debug" \
TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-60}" \
TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-ripple-shot.sh" \
TOPBAR_VISUAL_CONFIG="$config" \
  "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/run" \
  >"$artifact_root/run.log" 2>&1 ||
  echo "smoke-ripple: the run exited non-zero; see $artifact_root/run.log" >&2

cat "$artifact_root/run/ripple.txt" 2>/dev/null || true
echo "--- frames ---"
find "$artifact_root" -name '*.png' | sort
