#!/usr/bin/env sh
# The tray matrix, driven inside the nested niri session.
#
#   nix develop -c ./scripts/smoke-tray.sh
#
# Every run puts `topbar-fake-sni` applications on the run's own private bus
# rather than looking for real ones, so the icons are reproducible, the menus
# are the fixture's and can be checked against it, and the developer's real
# tray is never touched. One nested session per scenario, because
# TOPBAR_SMOKE_OPEN can only open one surface per start:
#
#   1  basic         three icons: a themed name, an application's own icon
#                    directory, a colour pixmap, and a near-black grayscale
#                    one that has to be lifted to be legible at all
#   2  menu          the dbusmenu popover: labels, a checkmark, a radio
#                    group, a disabled row, separators, a submenu row
#   3  submenu       the same menu, walked into its submenu, with the back row
#   4  attention     a NeedsAttention flip, photographed before and after
#   5  overflow      fourteen icons: eleven and a chevron
#   6  overflow-open the chevron's popover, holding the other three
#   7  churn         an item leaving, and five re-registrations that must not
#                    move anything on the bar
#   8  empty         no applications at all: no tray widget on the bar
#
# Screenshots land in target/visual-smoke/tray/<scenario>/.
set -eu

# The fake applications do not exit when their private bus dies, so an
# interrupted or timed-out run strands them (14 of them once survived nine
# hours). Reap every fake this run spawned, whatever happens.
trap 'pkill -f "target/debug/topbar-fake-sni" 2>/dev/null || true' EXIT INT TERM

artifact_root="${1:-target/visual-smoke/tray}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in magick gdbus niri grim cargo timeout dbus-run-session; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)
live_config=crates/topbar-core/tests/fixtures/live-config.toml

# The live config already puts the tray first in the right-hand section, which
# is exactly where this needs it, so the file is used as it stands.
grep -q '^  "tray",' "$live_config" || {
  echo "the live config no longer lists the tray widget" >&2
  exit 1
}

# The nested session is 918px wide, which is not enough bar for twelve tray
# icons plus everything else on it — GTK says so, loudly, and the chevron ends
# up clipped off the edge. The overflow scenarios therefore run against a copy
# of the live config with a smaller limit: the *rule* being photographed is the
# same one (`max_icons - 1` inline, then a chevron), and the twelve-icon case is
# covered by the split-math tests and by the panel.log line the wide run leaves.
narrow_config="$artifact_root/narrow-config.toml"
{
  cat "$live_config"
  printf '\n[widgets.tray]\nmax_icons = 6\n'
} >"$narrow_config"

# One nested session: run <scenario> [open] [config]
run() {
  scenario=$1
  open=${2:-}
  config=${3:-$live_config}

  echo "smoke-tray: $scenario"
  if [ -n "$open" ]; then
    export TOPBAR_SMOKE_OPEN="$open"
  else
    unset TOPBAR_SMOKE_OPEN
  fi

  # The shared runner passes a single -v, which is INFO; the tray's own debug
  # lines are what "the bar rebuilt once" is checked against, so they are asked
  # for by target rather than by turning everything up to debug.
  RUST_LOG="topbar::widgets::tray=debug,topbar_services::tray=debug" \
  SMOKE_TRAY_SCENARIO="$scenario" \
  TOPBAR_SMOKE_TRAY=1 \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-90}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-tray-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$scenario" \
    >"$artifact_root/$scenario.log" 2>&1 ||
    echo "smoke-tray: $scenario exited non-zero; see $artifact_root/$scenario.log" >&2
  mv "$artifact_root/$scenario/panel.log" "$artifact_root/$scenario-panel.log" 2>/dev/null || true
}

run basic
run menu tray-menu
run submenu tray-submenu
run attention
run overflow "" "$narrow_config"
run overflow-open tray-overflow "$narrow_config"
run churn
run empty

echo "--- screenshots ---"
find "$artifact_root" -name '*.png' | sort
echo "--- rebuild lines (a burst must not add one) ---"
grep -H "tray: rebuilding" "$artifact_root"/*-panel.log 2>/dev/null || true
# The panel logs its level in colour, so the escape codes come out before the
# level can be matched on. An unfiltered grep here once reported a clean run
# that had a warning in it.
echo "--- warnings and errors ---"
sed 's/\x1b\[[0-9;]*m//g' "$artifact_root"/*-panel.log 2>/dev/null |
  grep -E "( WARN | ERROR |Gtk-WARNING|Gtk-CRITICAL)" || echo "none"
