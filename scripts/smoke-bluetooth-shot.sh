#!/usr/bin/env sh
# The Bluetooth driver, run inside the nested niri session by
# scripts/smoke-bluetooth.sh. One scenario per session, named by
# $SMOKE_BT_SCENARIO.
#
# The panel is talking to a BlueZ of this run's own, on the private session bus.
# The real one is on the system bus and is the developer's headphones; the
# `real` scenario is the only one that goes near it, and what it is checking is
# that the panel does nothing.
set -eu

. "$(dirname "$0")/smoke-shot.sh"

scenario="${SMOKE_BT_SCENARIO:-devices}"
art="$SMOKE_ARTIFACTS"

# The fake's own control interface, which is how a driver with no pointer moves
# the world the panel is looking at.
fake() {
  method=$1
  shift
  gdbus call --session \
    --dest org.bluez \
    --object-path /io/github/trevarj/topbar/FakeBluez1 \
    --method "io.github.trevarj.topbar.FakeBluez1.$method" "$@" \
    >"$art/bluez-$method.txt" 2>&1 || echo "[exit $?]" >>"$art/bluez-$method.txt"
  echo "--- $method $* ---"
  cat "$art/bluez-$method.txt"
}

case "$scenario" in
  bar)
    # (0) the final composition: network, VPN badge, output volume, Bluetooth,
    # battery, and the microphone dot — every indicator the button can carry
    # except the screen-share one, which needs a PipeWire this sandbox has
    # none of. The order is [`model::ORDER`] and nothing in it moves.
    parecord --device=topbar_smoke_mic --raw /dev/null >"$art/parecord.log" 2>&1 &
    recorder=$!
    sleep 3
    shot bar-all
    kill "$recorder" 2>/dev/null || true
    wait "$recorder" 2>/dev/null || true
    sleep 3
    shot bar-quiet
    ;;

  grid)
    # (a) the complete GNOME 45 grid: five toggles at once. Wi-Fi and VPN come
    # from the fake NetworkManager, Power Mode from the fake power daemon,
    # Bluetooth from the fake BlueZ, and Caffeine from the real logind — which
    # is the one daemon a smoke run is allowed to hold an inhibitor against.
    shot grid topbar-popover
    fake Calls
    ;;

  devices)
    # (b) three paired devices, connected-first: the headset with its battery,
    # then the two idle ones by name.
    shot devices topbar-popover
    # A battery arriving *after* the fact, as an interface rather than a
    # property — which is what a headset does a second after connecting.
    fake SetBattery "mouse" 41
    sleep 3
    shot devices-battery topbar-popover
    ;;

  spinner)
    # (c) a connect that takes its time. The queued `slow` outcome holds
    # BlueZ's reply for thirty seconds, which is what a device in a drawer
    # does — so the row's spinner is on screen to be photographed.
    shot devices-idle topbar-popover
    # `popover show <name>` runs a registered smoke action, which is how a
    # driver sequences several steps inside one session: TOPBAR_SMOKE_OPEN
    # fires exactly once, at start-up. The device it acts on came from the
    # environment the *panel* was started with — see smoke-bluetooth.sh.
    "$SMOKE_TOPBAR" popover show quick-settings-bluetooth-connect >/dev/null 2>&1 || true
    sleep 5
    # A spinning row cannot ever give `shot` two identical frames, so it is
    # allowed to give up: what is wanted is a frame from the middle of the
    # attempt, and "still changing" is the evidence that it *was* the middle.
    SHOT_TIMEOUT=8 shot devices-connecting topbar-popover || true
    ;;

  fail)
    # (d) a connect the panel asked for, refused. The switch was never moved —
    # the policy is pessimistic — and the caption lands under the row.
    shot connect-failed topbar-popover
    grep -i "bluetooth\|could not" "$art/../panel.log" >"$art/failure-lines.txt" 2>/dev/null || true
    echo "--- what the panel logged ---"
    cat "$art/failure-lines.txt" 2>/dev/null || true
    ;;

  pairing)
    # (e) a pairing this panel did not start. The fake calls the panel's own
    # Agent1 and waits; the row goes up with the code; the smoke action
    # confirms it; the fake records what came back.
    shot pairing-before topbar-popover
    fake TriggerConfirmation "kb" 123456
    sleep 4
    shot pairing-prompt topbar-popover
    # Through the service handle, the way the row's own Confirm button sends
    # it — there is no pointer to press it with.
    "$SMOKE_TOPBAR" popover show quick-settings-bluetooth-confirm >/dev/null 2>&1 || true
    sleep 5
    shot pairing-after topbar-popover
    # The fake recorded what came back out of its own Agent1 call, which is
    # how this proves the answer travelled on the bus and not merely on screen.
    fake Replies
    fake Agents
    ;;

  off)
    # (f) the radio switched off: the rows go, the caption arrives, and the
    # bar indicator goes with them.
    shot bt-off topbar-popover
    fake Calls
    ;;

  real)
    # (g) the machine's REAL BlueZ, with no override in the environment. The
    # panel lists whatever is paired and does nothing else: no agent
    # registered, no radio switched, no device connected. The log line is the
    # evidence, and `bluetoothctl show` afterwards is the corroboration.
    shot bt-real topbar-popover
    grep -i "bluetooth" "$art/../panel.log" >"$art/real-lines.txt" 2>/dev/null || true
    echo "--- what the panel logged against the real adapter ---"
    cat "$art/real-lines.txt" 2>/dev/null || true
    ;;

  *)
    echo "unknown scenario: $scenario" >&2
    exit 1
    ;;
esac
