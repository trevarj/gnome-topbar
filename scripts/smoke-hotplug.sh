#!/usr/bin/env sh
# A monitor taken away and given back, ten times, under one running panel.
#
#   nix develop -c ./scripts/smoke-hotplug.sh
#
# The bars are keyed by connector name because GDK hands out a fresh
# `GdkMonitor` object across every hotplug, so object identity says nothing
# about which physical output a bar belongs to. This run is what proves the key
# holds: `niri msg output winit off` takes the output away, `on` gives it back,
# and the panel has to end each cycle with exactly one bar — not two, not none,
# and not one built against a monitor that had not finished arriving.
#
# What it watches for, beyond the bar coming back:
#
#   * **Duplicate bars.** A second bar on the same output would be a connector
#     key that changed under the panel. The log line counts them every sync.
#   * **Leaked signal handlers.** One handler leaked per cycle would make the
#     panel reconfigure once per past hotplug — which looks like a slow panel
#     rather than like a leak, and is why the count is in the log at all.
#   * **GTK criticals**, which is what an allocation against a monitor with no
#     geometry yet produces.
#
# Screenshots land in target/visual-smoke/hotplug/.
set -eu

artifact_root="${1:-target/visual-smoke/hotplug}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in niri grim cargo timeout dbus-run-session; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)
live_config="$repo/crates/topbar-core/tests/fixtures/live-config.toml"

# The live configuration with the two widgets that would reach outside the
# sandbox taken off the bar: the weather has no coordinates in the live file,
# and the crypto script is the user's own. What this run is about is the bar
# arriving and leaving, and every remaining widget exercises that.
config="$artifact_root/hotplug-config.toml"
sed -e 's/^left = \["workspaces", "custom-crypto"\]$/left = ["workspaces"]/' \
  -e 's/^center = \["weather", "clock"\]$/center = ["clock"]/' \
  "$live_config" >"$config"

RUST_LOG="info,topbar::bar=debug" \
TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-180}" \
TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-hotplug-shot.sh" \
TOPBAR_VISUAL_CONFIG="$config" \
  "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/session" \
  >"$artifact_root/session.log" 2>&1 ||
  echo "smoke-hotplug: the run exited non-zero; see $artifact_root/session.log" >&2

echo "--- cycles ---"
cat "$artifact_root/session/cycles.txt" 2>/dev/null || echo "(none recorded)"
echo "--- what the panel counted ---"
grep -a "bar(s) active" "$artifact_root/session/panel.log" 2>/dev/null || echo "(nothing)"
echo "--- anything GTK complained about ---"
grep -aE "CRITICAL|WARNING \*\*|panicked" "$artifact_root/session/panel.log" 2>/dev/null ||
  echo "(clean)"
