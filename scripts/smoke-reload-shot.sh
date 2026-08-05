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

# One capture of the bar as it is now.
#
# A plain `grim` rather than `shot`, deliberately: `shot` returns when two
# consecutive frames are identical, and a bar with a clock on it is never twice
# the same for long. The hard evidence in this scenario is the panel's own log,
# which `await_reload` has already waited for; the frame is what a person looks
# at afterwards. One second of grace lets the rebuild reach the screen.
frame() {
  sleep 1
  grim "$art/$1.png"
  echo "smoke-reload: $1 captured"
}

# Rewrite one key in one section. `sed` ranges, because `interval` and friends
# appear in several sections of this file and rewriting all of them would be
# reloading a different configuration from the one under test.
#
# The delimiter is `|` and not `#`, because half the values here are hex
# colours and `s#...#...#` with a `#` inside it is a broken script rather than
# an edit — which is exactly how the accent step failed the first time.
edit() {
  section=$1
  key=$2
  value=$3
  if [ -n "$section" ]; then
    sed -i -e "/^\[$section\]$/,/^$/ s|^$key = .*|$key = $value|" "$config"
  else
    sed -i -e "s|^$key = .*|$key = $value|" "$config"
  fi
}

# --- 1. the clock format ---------------------------------------------------
#
# v1's oldest reload defect: a changed `format` was ignored until the panel was
# restarted. The label here has to go from a long date to a bare time.
#
# Minutes, not seconds: `shot` waits for two identical frames, and a clock with
# a seconds field never gives it two.
frame reload-1-clock-before
edit 'widgets.clock' format '"%H:%M"'
await_reload || true
frame reload-2-clock-after

# --- 2. a widget that was not there ----------------------------------------
#
# `[widgets.crypto]` has been configured all along and nothing placed it, so
# the crypto service was never started and CoinGecko — here, the stub — has
# heard nothing. Placing the widget is a change to the placement arrays, so
# the bars are rebuilt, and the service has to be started before the new
# widget subscribes to it.
#
# The evidence is the panel's own log on one side and the service's first
# request on the other, so this step records both.
grep -a "the reload started" "$log" >"$art/requests-before.txt" 2>&1 || true
echo "services started before the widget was placed: $(wc -l <"$art/requests-before.txt")" \
  >"$art/requests-before.txt"

edit 'widgets' left '["workspaces", "crypto"]'
await_reload || true
frame reload-3-crypto-placed
grep -a "the reload started" "$log" >"$art/requests-after.txt" 2>&1 ||
  echo "(the reload started no service)" >"$art/requests-after.txt"

# --- 3. the crypto entries -------------------------------------------------
#
# The widget's own section changed, so that widget is rebuilt and nothing else
# is: two coins become three, one of them a pair.
edit 'widgets.crypto' entries '["btc", "eth", "eth/btc"]'
await_reload || true
frame reload-4-crypto-entries

# --- 4. the workspace labels -----------------------------------------------
#
# Dots become numbers. The workspaces widget draws its own geometry from
# `label_type`, so this is a rebuild rather than a redraw.
edit 'widgets.workspaces' label_type '"index"'
await_reload || true
frame reload-5-workspaces-after

# --- 5. the accent colour --------------------------------------------------
#
# No widget is touched at all: the sheet is regenerated and the single provider
# swapped. The active workspace pill and the clock's underline are what change
# colour in the frame.
edit '' accent '"#e01b24"'
await_reload || true
frame reload-6-accent-after

# --- 6. the bar height -----------------------------------------------------
#
# The one edit that has to rebuild the windows: the height is the exclusive
# zone, so the desktop below moves as well.
edit 'bar' size '52'
await_reload || true
frame reload-7-size-after

# --- 7. a file that will not parse -----------------------------------------
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
shot reload-8-broken topbar-toast || grim "$art/reload-8-broken.png"

# The bar is still the bar it was: same height, same accent, same widgets.
sleep 2
frame reload-9-still-running

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
