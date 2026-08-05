#!/usr/bin/env sh
# A thousand popover open/close cycles, watching the panel's memory.
#
#   nix develop -c ./scripts/smoke-soak.sh
#
# The regression this exists for is v1's: content rebuilt on every open with a
# CSS provider attached to it, neither ever released, and a panel that grew for
# as long as the session lasted. v2 retains each widget's popover content for
# the widget's lifetime, so the cycle costs a map and an unmap and nothing else.
#
# The gate: after a hundred cycles of warm-up — first paints, icon lookups,
# fonts, the allocator finding its working set — resident memory must not grow
# by more than five per cent over the remaining nine hundred.
#
# Everything is exercised: the control panel, Quick Settings, the resource
# popover, a tray application's menu and the crypto card, each with a blur
# region attached, so this soaks the Wayland protocol objects too.
#
# Artifacts land in target/visual-smoke/soak/.
set -eu

trap 'pkill -f "target/debug/topbar-fake-" 2>/dev/null || true' EXIT INT TERM

artifact_root="${1:-target/visual-smoke/soak}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)
repo=$(pwd)

for tool in niri grim cargo timeout dbus-run-session awk; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

live_config=crates/topbar-core/tests/fixtures/live-config.toml

# The live configuration with three changes, all of them so that every popover
# in the list actually exists: the crypto widget beside the user's script, and
# thresholds low enough that the system monitor is visible (it is an alert-only
# widget, and an invisible one has nothing to open).
config="$artifact_root/soak.toml"
sed -e 's/^exec = .*/exec = "\/bin\/echo BTC"/' \
  -e 's/^left = .*/left = ["workspaces", "custom-crypto", "crypto"]/' \
  -e 's/^cpu_threshold = .*/cpu_threshold = 1/' \
  -e 's/^memory_threshold = .*/memory_threshold = 1/' \
  "$live_config" >"$config"
grep -q '"crypto"' "$config" || {
  echo "could not add the crypto widget to the copied config" >&2
  exit 1
}

RUST_LOG="info,topbar=debug" \
SOAK_CYCLES="${SOAK_CYCLES:-1000}" \
SOAK_SAMPLE="${SOAK_SAMPLE:-100}" \
TOPBAR_SMOKE_TRAY=1 \
TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-3600}" \
TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-soak-shot.sh" \
TOPBAR_VISUAL_CONFIG="$config" \
  "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/run" \
  >"$artifact_root/run.log" 2>&1 ||
  echo "smoke-soak: the run exited non-zero; see $artifact_root/run.log" >&2

series="$artifact_root/run/rss.tsv"
if [ ! -f "$series" ]; then
  echo "smoke-soak: no measurements were taken" >&2
  exit 1
fi

# The verdict, from the warm-up reading against the last one.
awk -F'\t' '
  $1 == 100 { warm = $2 }
  $1 ~ /^[0-9]+$/ && $2 > 0 { last = $2; last_cycle = $1 }
  END {
    if (warm == 0) { print "no warm-up reading; cannot judge"; exit 1 }
    growth = (last - warm) / warm * 100
    printf "warm-up (cycle 100): %d kB\n", warm
    printf "final (cycle %d):    %d kB\n", last_cycle, last
    printf "growth over the remaining cycles: %+.2f%%\n", growth
    if (growth < 5.0) { print "PASS: under the five per cent gate" }
    else { print "FAIL: memory grew past the gate" }
  }
' "$series" | tee "$artifact_root/verdict.txt"

echo
echo "--- the series ---"
cat "$series"
echo
echo "--- protocol objects and anything that went wrong ---"
sed -n '/blur protocol objects/,$p' "$artifact_root/run/soak.txt" 2>/dev/null || true
