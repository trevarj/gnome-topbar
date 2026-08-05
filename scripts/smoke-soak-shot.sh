#!/usr/bin/env sh
# Opens and closes every popover the panel has, a thousand times, and watches
# the panel's resident memory while it does. Driven by scripts/smoke-soak.sh.
#
# This is v1's style-accumulation leak as a regression test. In v1 each open
# built its content afresh and attached a CSS provider to it, and neither was
# ever released; the panel grew for as long as it ran. v2 builds a widget's
# popover content once and hands the same tree back on every open, so the
# series below should be flat.
#
# With blur attached to each of those surfaces this is also a protocol-object
# soak: a surface that is hidden and shown again is a new wl_surface and needs a
# new effect object, so the count *created* climbs with the cycles, but the
# count *alive* has to come back down every time.
set -eu

art="$SMOKE_ARTIFACTS"
cycles="${SOAK_CYCLES:-1000}"
sample="${SOAK_SAMPLE:-100}"

# One tray application, so the tray has a menu to open.
if [ -n "${SMOKE_FAKE_SNI:-}" ]; then
  "$SMOKE_FAKE_SNI" --id soak --title "Soak" --items "One,Two,Three" \
    >"$art/fake-sni.log" 2>&1 &
  sleep 2
fi

rss() {
  awk '/VmRSS/ {print $2}' "/proc/$SMOKE_PANEL_PID/status" 2>/dev/null || echo 0
}

printf 'cycle\tvmrss_kb\n' >"$art/rss.tsv"
printf '0\t%s\n' "$(rss)" >>"$art/rss.tsv"

cycle=0
while [ "$cycle" -lt "$cycles" ]; do
  cycle=$((cycle + 1))
  case $((cycle % 5)) in
    0) widget=clock ;;
    1) widget=quick_settings ;;
    2) widget=system_monitor ;;
    3) widget=tray-menu ;;
    *) widget=crypto ;;
  esac

  "$SMOKE_TOPBAR" popover show "$widget" >/dev/null 2>&1 || true
  # Long enough for the surface to map and start growing in, which is what puts
  # the churn in this test; the close then reverses it mid-flight.
  sleep 0.05
  "$SMOKE_TOPBAR" popover hide >/dev/null 2>&1 || true

  if [ $((cycle % sample)) -eq 0 ]; then
    printf '%s\t%s\n' "$cycle" "$(rss)" >>"$art/rss.tsv"
    echo "soak: $cycle cycles, VmRSS $(rss) kB"
  fi
done

# A last reading after everything has been given a moment to settle, and a
# photograph of the bar: a panel that leaked its way through a thousand cycles
# tends to look wrong as well as weigh more.
sleep 3
printf 'final\t%s\n' "$(rss)" >>"$art/rss.tsv"
grim "$art/after.png" || true

{
  echo "--- resident memory, kB ---"
  cat "$art/rss.tsv"

  echo "--- blur protocol objects ---"
  grep -a "effect object created" "$art/panel.log" | tail -1 || echo "(none created)"
  grep -ac "effect object created" "$art/panel.log" || true
  echo "(created in total, above)"
  grep -a "effect object destroyed" "$art/panel.log" | tail -1 || echo "(none destroyed)"
  echo "--- the most that were ever alive at once ---"
  grep -ao "([0-9]* live)" "$art/panel.log" | tr -dc '0-9\n' | sort -n | tail -1 ||
    echo "(nothing to count)"

  echo "--- anything that went wrong ---"
  grep -aiE "protocol error|panicked|BorrowMutError|GLib-GObject-(WARNING|CRITICAL)" \
    "$art/panel.log" || echo "(nothing did)"
} >"$art/soak.txt" 2>&1

cat "$art/soak.txt"
