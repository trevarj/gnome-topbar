#!/usr/bin/env sh
# Hot reload, photographed: one nested session, six edits to the configuration
# file underneath a running panel, and a frame on each side of every one.
#
#   nix develop -c ./scripts/smoke-reload.sh
#
# This is the M12 headline and its definition of done. Nothing here restarts the
# panel: the driver edits the file the panel was started with and the panel's
# own watcher notices, exactly as it does when the user saves in an editor.
#
#   clock       [widgets.clock] format changes; the label changes with it
#   placed      the crypto widget is *added* to the bar. The panel starts
#               without one, so the crypto service is not started either and
#               the price endpoint sees no requests at all; the reload has to
#               start it lazily and the prices have to appear
#   crypto      [widgets.crypto] entries change; the bar draws the new ones
#   workspaces  label_type none -> index; the widget is rebuilt with numbers
#   accent      theme.accent changes; the stylesheet is swapped under the bar
#   size        bar.size changes; every bar is rebuilt at the new height
#   broken      the file is made unparseable; the panel keeps running and says
#               so in a banner, and the configuration on screen does not move
#
# The crypto widget is the built-in one rather than the user's custom-* script,
# because entries are what this scenario edits. It is pointed at
# scripts/crypto-stub.py, so the run needs no network and CoinGecko never hears
# from a test.
#
# Screenshots land in target/visual-smoke/reload/.
set -eu

trap 'pkill -f "target/debug/topbar-fake-" 2>/dev/null || true' EXIT INT TERM

artifact_root="${1:-target/visual-smoke/reload}"
port="${TOPBAR_SMOKE_STUB_PORT:-18086}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in python3 magick curl niri grim cargo timeout dbus-run-session; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)
live_config="$repo/crates/topbar-core/tests/fixtures/live-config.toml"

# --- the configuration the panel starts from -------------------------------
#
# The live file with the custom-* crypto script swapped for the built-in
# widget: this scenario edits `[widgets.crypto] entries`, which the script does
# not have. The weather comes off the bar because the live file has no
# coordinates in it, and "Configure…" in every frame is a distraction.
#
# The bar starts *without* the crypto widget: its section is configured and
# nothing places it, which is exactly the case lazy service start exists for.
# The first thing the driver does is add it.
config="$artifact_root/reload-config.toml"
sed -e 's/^left = \["workspaces", "custom-crypto"\]$/left = ["workspaces"]/' \
  -e 's/^center = \["weather", "clock"\]$/center = ["clock"]/' \
  "$live_config" >"$config"
cat >>"$config" <<'TOML'

[widgets.crypto]
entries = ["btc", "eth"]
interval = 1800
TOML
grep -q '^left = \["workspaces"\]$' "$config" || {
  echo "could not take the custom crypto script off the bar" >&2
  exit 1
}

# --- the price endpoint ----------------------------------------------------
stub_pid=""
stop_stub() {
  if [ -n "$stub_pid" ]; then
    kill "$stub_pid" 2>/dev/null || true
    wait "$stub_pid" 2>/dev/null || true
    stub_pid=""
  fi
}
trap 'stop_stub; pkill -f "target/debug/topbar-fake-" 2>/dev/null || true' EXIT INT TERM

python3 "$repo/scripts/crypto-stub.py" --port "$port" --status 200 \
  >"$artifact_root/stub.log" 2>&1 &
stub_pid=$!
waited=0
got=000
while [ "$waited" -lt 10 ]; do
  got=$(curl -s -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:$port/api/v3/simple/price?probe=1" 2>/dev/null || echo 000)
  [ "$got" = "200" ] && break
  sleep 1
  waited=$((waited + 1))
done
if [ "$got" != "200" ]; then
  echo "smoke-reload: nothing on port $port answers 200 (got '$got')" >&2
  echo "smoke-reload: a stray stub is probably holding it — check" >&2
  echo "  pgrep -af crypto-stub.py" >&2
  exit 1
fi
echo "smoke-reload: price stub answering on port $port"
export TOPBAR_CRYPTO_API="http://127.0.0.1:$port"

RUST_LOG="info,topbar::reload=debug,topbar::bar=debug,topbar::widgets=debug" \
TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-180}" \
TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-reload-shot.sh" \
TOPBAR_VISUAL_CONFIG="$config" \
  "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/session" \
  >"$artifact_root/session.log" 2>&1 ||
  echo "smoke-reload: the run exited non-zero; see $artifact_root/session.log" >&2

stop_stub

echo "--- screenshots ---"
find "$artifact_root" -name '*.png' | sort
echo "--- what the price endpoint was asked for, and when ---"
# The lazy-start assertion: the stub's own log. Nothing may reach it before
# the reload that places the widget, and something must reach it after.
grep -c "GET" "$artifact_root/stub.log" 2>/dev/null || echo 0
cat "$artifact_root/session/requests-before.txt" 2>/dev/null || true
cat "$artifact_root/session/requests-after.txt" 2>/dev/null || true
echo "--- what the panel reloaded ---"
grep -aE "reload|config error|watching" "$artifact_root/session/panel.log" 2>/dev/null ||
  echo "(nothing)"
echo "--- anything GTK complained about ---"
grep -aE "CRITICAL|WARNING \*\*|panicked" "$artifact_root/session/panel.log" 2>/dev/null ||
  echo "(clean)"
