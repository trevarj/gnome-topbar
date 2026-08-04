#!/usr/bin/env sh
# One screenshot of whatever the weather run put on screen, plus the state
# file the panel wrote. Driven by scripts/smoke-weather.sh; not useful alone.
set -eu

art="$SMOKE_ARTIFACTS"

# niri runs inside a winit window here, and a window nobody is looking at is
# throttled: grim hands back the last frame that was *presented*, which can be
# seconds behind the last frame that was drawn. Waiting is the whole fix. The
# weather also has to arrive, which is one HTTP round trip to loopback.
sleep 4
grim "$art/weather.png"

# The location the dialog saves lives in the sandboxed state file, and a
# second panel start reading it is what makes "saved" mean something.
if [ -n "${XDG_STATE_HOME:-}" ] && [ -f "$XDG_STATE_HOME/topbar/state.json" ]; then
  cp "$XDG_STATE_HOME/topbar/state.json" "$art/state.json"
fi

niri msg layers >"$art/layers.txt" 2>&1 || true
