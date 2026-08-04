#!/usr/bin/env sh
# One screenshot of whatever the weather run put on screen, plus the state
# file the panel wrote. Driven by scripts/smoke-weather.sh; not useful alone.
#
# $SMOKE_EXPECT names the layer surface this scenario is about, so the capture
# waits until that surface has actually been drawn rather than for a number of
# seconds — see scripts/smoke-shot.sh for why the distinction has teeth.
set -eu

art="$SMOKE_ARTIFACTS"
. "$(dirname "$0")/smoke-shot.sh"

shot weather "${SMOKE_EXPECT:-}"

# The location the dialog saves lives in the sandboxed state file, and a
# second panel start reading it is what makes "saved" mean something.
if [ -n "${XDG_STATE_HOME:-}" ] && [ -f "$XDG_STATE_HOME/topbar/state.json" ]; then
  cp "$XDG_STATE_HOME/topbar/state.json" "$art/state.json"
fi

niri msg layers >"$art/layers.txt" 2>&1 || true
