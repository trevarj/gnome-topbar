# Takes the nested session's only output away and gives it back, ten times.
# Driven by scripts/smoke-hotplug.sh; not useful alone.
#
# Each cycle is checked rather than assumed: after `off` the panel's layer
# surface has to be gone from `niri msg layers`, and after `on` it has to be
# back. A cycle that fails either half is recorded and the loop carries on, so
# the report says "9 of 10" instead of stopping at the first one.
set -eu

art="$SMOKE_ARTIFACTS"
cycles="$art/cycles.txt"
output="${SMOKE_HOTPLUG_OUTPUT:-winit}"
rounds="${SMOKE_HOTPLUG_ROUNDS:-10}"

# Whether the panel has a bar mapped on any output.
bar_mapped() {
  niri msg layers 2>/dev/null | grep -q '"topbar"'
}

# Wait up to `$2` seconds for the bar to be mapped (`$1` = yes) or gone.
wait_for_bar() {
  want=$1
  left=${2:-10}
  while [ "$left" -gt 0 ]; do
    if bar_mapped; then
      [ "$want" = yes ] && return 0
    else
      [ "$want" = no ] && return 0
    fi
    sleep 1
    left=$((left - 1))
  done
  return 1
}

: >"$cycles"
grim "$art/hotplug-before.png" 2>/dev/null || true

# The first cycle is also the check that this backend can do it at all. A
# compositor that refuses to disable its only output would otherwise produce
# ten identical "failures" that say nothing about the panel.
niri msg output "$output" off >>"$cycles" 2>&1 || true
if wait_for_bar no 10; then
  supported=yes
else
  supported=no
  echo "the bar is still mapped after \`output $output off\`" >>"$cycles"
fi
niri msg output "$output" on >>"$cycles" 2>&1 || true
wait_for_bar yes 15 || echo "the bar did not come back after the first cycle" >>"$cycles"

if [ "$supported" = no ]; then
  # niri's winit backend will not disable the window it is running in, which
  # is the only output a nested session has. Taking a monitor away therefore
  # belongs on the live-session checklist.
  #
  # What *can* be driven here is the other half of the same code path, and it
  # is the half that produced the bugs: a burst of monitor-list signals, a
  # geometry that changes underneath the bars, and the debounced sync that has
  # to answer all of it exactly once. Ten scale changes in a row is a
  # reconfigure storm, and the assertions afterwards are the same ones —
  # one bar, two handlers, no criticals, no duplicate bars.
  echo "hotplug: this backend keeps its only output; churning its geometry instead" >>"$cycles"
  round=0
  while [ "$round" -lt "$rounds" ]; do
    round=$((round + 1))
    case $((round % 2)) in
      0) scale=0.75 ;;
      *) scale=1.0 ;;
    esac
    niri msg output "$output" scale "$scale" >/dev/null 2>&1 || true
    sleep 1
    if bar_mapped; then
      echo "cycle $round: scale $scale, bar still mapped" >>"$cycles"
    else
      echo "cycle $round: scale $scale, THE BAR IS GONE" >>"$cycles"
    fi
  done
  niri msg output "$output" scale 0.75 >/dev/null 2>&1 || true
  sleep 2
  grim "$art/hotplug-after.png" 2>/dev/null || true
  {
    echo "--- the last thing the panel counted ---"
    grep -a "bar(s) active" "$art/panel.log" | tail -3
    echo "--- how many bars were ever built ---"
    grep -ac "bar on " "$art/panel.log" || true
    echo "--- anything GTK complained about ---"
    grep -aE "CRITICAL|WARNING \*\*|panicked" "$art/panel.log" || echo "(clean)"
  } >>"$cycles" 2>&1
  cat "$cycles"
  exit 0
fi

# Cycle 1 is already done and counted.
good=1
round=1
while [ "$round" -lt "$rounds" ]; do
  round=$((round + 1))
  niri msg output "$output" off >/dev/null 2>&1 || true
  torn_down=no
  wait_for_bar no 10 && torn_down=yes

  niri msg output "$output" on >/dev/null 2>&1 || true
  came_back=no
  wait_for_bar yes 15 && came_back=yes

  if [ "$torn_down" = yes ] && [ "$came_back" = yes ]; then
    good=$((good + 1))
    echo "cycle $round: torn down and back" >>"$cycles"
  else
    echo "cycle $round: torn down=$torn_down back=$came_back" >>"$cycles"
  fi
done

echo "$good of $rounds cycles returned the bar" >>"$cycles"

# The panel's own count, after the dust settles: one bar, and the same two
# monitor handlers it started with. A handler leaked per cycle would read
# twenty here.
sleep 2
grim "$art/hotplug-after.png" 2>/dev/null || true
{
  echo "--- the last thing the panel counted ---"
  grep -a "bar(s) active" "$art/panel.log" | tail -3
  echo "--- how many bars were ever built ---"
  grep -ac "bar on " "$art/panel.log" || true
  echo "--- anything GTK complained about ---"
  grep -aE "CRITICAL|WARNING \*\*|panicked" "$art/panel.log" || echo "(clean)"
} >>"$cycles" 2>&1

cat "$cycles"
