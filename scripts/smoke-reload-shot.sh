# Edits the configuration underneath a running panel and photographs what it
# does about it. Driven by scripts/smoke-reload.sh; not useful alone.
#
# The panel is never restarted here — that is the whole point. Each step edits
# $SMOKE_CONFIG, which is the file the panel was started with and is watching,
# waits for the panel to say it reloaded, and takes a frame. Waiting on the log
# rather than on a clock is the same discipline `shot` uses for surfaces: the
# 250ms debounce plus a parse off the main thread has no fixed duration, and a
# fixed sleep would be a coin toss.
set -eu

art="$SMOKE_ARTIFACTS"
config="$SMOKE_CONFIG"
log="$art/panel.log"
. "$(dirname "$0")/smoke-shot.sh"

# Wait until the panel's log has said `reloaded` one more time than it had.
# Returns non-zero if it never does, which leaves the frame that follows as
# evidence of what it did instead.
reloads=0
await_reload() {
  want=$((reloads + 1))
  waited=0
  while [ "$waited" -lt 20 ]; do
    got=$(grep -ac "reloading\|reloaded" "$log" 2>/dev/null || echo 0)
    if [ "$got" -ge "$want" ]; then
      reloads=$got
      echo "smoke-reload: reload $want seen after ${waited}s"
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  echo "smoke-reload: no reload after ${waited}s" >&2
  return 1
}

# Rewrite one key in one section. `sed` ranges, because `interval` and friends
# appear in several sections of this file and rewriting all of them would be
# reloading a different configuration from the one under test.
edit() {
  section=$1
  key=$2
  value=$3
  if [ -n "$section" ]; then
    sed -i -e "/^\[$section\]$/,/^$/ s#^$key = .*#$key = $value#" "$config"
  else
    sed -i -e "s#^$key = .*#$key = $value#" "$config"
  fi
}

# --- 1. the clock format ---------------------------------------------------
#
# v1's oldest reload defect: a changed `format` was ignored until the panel was
# restarted. The label here has to go from a long date to a bare time.
still reload-1-clock-before
edit 'widgets.clock' format '"%H:%M:%S"'
await_reload || true
still reload-2-clock-after

# --- 2. the crypto entries -------------------------------------------------
#
# The widget's own section changed, so that widget is rebuilt and nothing else
# is: two coins become three, one of them a pair.
edit 'widgets.crypto' entries '["btc", "eth", "eth/btc"]'
await_reload || true
still reload-3-crypto-after

# --- 3. the workspace labels -----------------------------------------------
#
# Dots become numbers. The workspaces widget draws its own geometry from
# `label_type`, so this is a rebuild rather than a redraw.
edit 'widgets.workspaces' label_type '"index"'
await_reload || true
still reload-4-workspaces-after

# --- 4. the accent colour --------------------------------------------------
#
# No widget is touched at all: the sheet is regenerated and the single provider
# swapped. The active workspace pill and the clock's underline are what change
# colour in the frame.
edit '' accent '"#e01b24"'
await_reload || true
still reload-5-accent-after

# --- 5. the bar height -----------------------------------------------------
#
# The one edit that has to rebuild the windows: the height is the exclusive
# zone, so the desktop below moves as well.
edit 'bar' size '52'
await_reload || true
still reload-6-size-after

# --- 6. a file that will not parse -----------------------------------------
#
# The panel must keep running on the configuration it already has, say so once,
# and change nothing. The banner is a notification like any other, so it is
# photographed as the toast layer.
cp "$config" "$art/last-good-config.toml"
printf '\nthis is not = = toml\n' >>"$config"
waited=0
while [ "$waited" -lt 20 ]; do
  grep -aq "config error" "$log" && break
  sleep 1
  waited=$((waited + 1))
done
shot reload-7-broken topbar-toast || grim "$art/reload-7-broken.png"

# The bar is still the bar it was: same height, same accent, same widgets.
sleep 2
still reload-8-still-running

niri msg layers >"$art/layers.txt" 2>&1 || true

{
  echo "--- what the panel reloaded ---"
  grep -a "reloading\|reloaded\|config error" "$log" || echo "(nothing)"
  echo "--- what it rebuilt ---"
  grep -a "widget(s)\|bar(s) active" "$log" || echo "(nothing)"
  echo "--- anything GTK complained about ---"
  grep -aE "CRITICAL|WARNING \*\*|panicked" "$log" || echo "(clean)"
} >"$art/notes.txt" 2>&1
cat "$art/notes.txt"
