#!/usr/bin/env sh
# The network driver, run inside the nested niri session by
# scripts/smoke-network.sh. One scenario per session, named by
# $SMOKE_NET_SCENARIO.
#
# The panel is talking to a NetworkManager of this run's own on the private
# session bus, except in the `real` scenario — which is deliberately pointed at
# the machine's actual one and must be seen to do nothing to it.
#
# Two kinds of capture are used. `shot` waits for a *still* frame, which is what
# most screenshots want; a spinner never stands still, so the frames that are
# supposed to show one are taken with plain `grim` after the panel itself has
# already been photographed still.
set -eu

. "$(dirname "$0")/smoke-shot.sh"

scenario="${SMOKE_NET_SCENARIO:-panel}"
art="$SMOKE_ARTIFACTS"

# The fake's own control interface, driven the way `nmcli` would drive the real
# NetworkManager from outside the panel.
nm() {
  method=$1
  shift
  gdbus call --session \
    --dest org.freedesktop.NetworkManager \
    --object-path /io/github/trevarj/topbar/FakeNm1 \
    --method "io.github.trevarj.topbar.FakeNm1.$method" "$@" \
    >"$art/nm-$method.txt" 2>&1 || echo "[exit $?]" >>"$art/nm-$method.txt"
  echo "--- $method $* ---"
  cat "$art/nm-$method.txt"
}

# Read one of the fake's recorders into a named file.
record() {
  method=$1
  label=$2
  gdbus call --session \
    --dest org.freedesktop.NetworkManager \
    --object-path /io/github/trevarj/topbar/FakeNm1 \
    --method "io.github.trevarj.topbar.FakeNm1.$method" \
    >"$art/$label-$method.txt" 2>&1 || echo "[exit $?]" >>"$art/$label-$method.txt"
  echo "--- $method ($label) ---"
  cat "$art/$label-$method.txt"
}

# A frame with something moving in it. `shot` would never settle on one.
moving() {
  sleep "${2:-3}"
  grim "$art/$1.png"
  echo "smoke-shot: $1 captured while something was moving"
}

# Run a registered debug smoke action inside the panel.
action() {
  "$SMOKE_TOPBAR" popover show "$1" >/dev/null 2>&1 || true
}

# Prove the password is in no process's command line. This is the leak v1 had:
# `nmcli … password <it>` left the key readable in /proc for the life of the
# process, and the whole secret-agent design exists to make it impossible.
no_psk_in_argv() {
  {
    echo "searching every command line under /proc for the smoke password"
    found=0
    for entry in /proc/[0-9]*; do
      [ -r "$entry/cmdline" ] || continue
      if tr '\0' ' ' <"$entry/cmdline" 2>/dev/null | grep -q "topbar-smoke-psk"; then
        echo "LEAK: $entry $(tr '\0' ' ' <"$entry/cmdline")"
        found=1
      fi
    done
    [ "$found" -eq 0 ] && echo "no process has it in argv (correct)"
    echo "searching the panel's own log"
    if grep -q "topbar-smoke-psk" "$art/panel.log" 2>/dev/null; then
      echo "LEAK: panel.log carries the password"
    else
      echo "panel.log does not carry it (correct)"
    fi
    # The fake NetworkManager's own recorder is *supposed* to have it: that
    # file is the assertion that the key arrived through the agent's reply.
    # Anything else carrying it would be the leak.
    echo "searching every file this run wrote, except the fake's own recorder"
    if grep -rl "topbar-smoke-psk" "$art" 2>/dev/null |
      grep -vE "psk-audit|-Secrets[.]txt|driver[.]log"; then
      echo "LEAK: the files above carry it"
    else
      echo "nothing but the fake's recorder carries it (correct)"
    fi
  } >"$art/psk-audit.txt" 2>&1
  echo "--- password audit ---"
  cat "$art/psk-audit.txt"
}

case "$scenario" in
  bar)
    # (a) the button on the bar. First with the radio doing the work: the
    # strength icon for the network the card is on, and the VPN badge beside
    # it. Then a cable goes in and takes precedence, which is the rule.
    nm SetVpnActive "uuid-work" true
    sleep 3
    shot bar-wifi
    nm SetCarrier true 1000
    sleep 3
    shot bar-wired
    ;;

  panel)
    # (b) the panel with both pills collapsed: Wi-Fi named by the network it is
    # on, VPN named by the profile, and the wired row under the grid.
    shot panel topbar-popover
    record Calls panel
    ;;

  list)
    # (c) seven networks in range, sorted, with a padlock on the secured ones
    # and a checkmark on the one the card is on.
    shot list topbar-popover
    record Calls list
    # Then one row is left mid-connect: the queued outcome takes half a minute
    # to answer, which is exactly long enough to photograph a spinner.
    nm QueueActivationOutcome "slow"
    action quick-settings-wifi-connect
    moving list-connecting 6
    ;;

  password)
    # (d) a secured stranger the card has never joined. The panel asks
    # NetworkManager to join it, NetworkManager asks the panel's agent for a
    # password, and the row appears — nothing here guessed that a password was
    # needed.
    # `moving`, not `shot`: the row the panel is joining keeps its spinner
    # turning for as long as the prompt is up, so there is no still frame to
    # settle on — which is the point of the pessimistic policy.
    moving password 12
    # An unsolicited request is refused: the panel prompts for what the user
    # asked for, not for whatever wants a secret.
    nm TriggerGetSecrets "Elsewhere" "Elsewhere" 1
    action quick-settings-wifi-password
    sleep 4
    shot password-sent topbar-popover
    record Secrets password
    record Calls password
    no_psk_in_argv
    ;;

  authfail)
    # (e) the same, refused. The caption under the entry says so, and the
    # profile NetworkManager added for the attempt is gone rather than left in
    # the list as a dead duplicate.
    record ProfileCount before
    sleep 12
    action quick-settings-wifi-password
    # The prompt comes back with the error on it and the row keeps spinning,
    # so this frame has something moving in it too.
    moving authfail 6
    record ProfileCount after
    record Calls authfail
    record Secrets authfail
    ;;

  vpn)
    # (f) two profiles. One is brought up from outside and leads the list with
    # its accent mark; then the panel is asked for the other, which is queued
    # to answer nothing, so its row spins and the caption follows.
    nm SetVpnActive "uuid-work" true
    sleep 3
    shot vpn-active topbar-popover
    nm QueueActivationOutcome "timeout"
    action quick-settings-vpn-connect
    moving vpn-pending 6
    record Calls vpn
    ;;

  radiooff)
    # (g) the radio switched off from outside, as `nmcli radio wifi off` would:
    # the pill loses its fill, the list goes, the bar icon flips.
    shot radio-on topbar-popover
    nm SetWirelessEnabled false
    sleep 4
    shot radio-off topbar-popover
    ;;

  real)
    # (h) THE READ-ONLY RUN. No fake: the panel is on the machine's real system
    # bus. It must list what is there and change nothing — no scan, no
    # activation, and above all no secret agent, because a second agent would
    # take the session's own panel out of the queue for its prompts.
    sleep 5
    shot real-list topbar-popover
    {
      echo "--- what the service decided ---"
      sed 's/\x1b\[[0-9;]*m//g' "$art/panel.log" | grep -iE "read-only" || echo "(nothing)"
      echo "--- anything that would have changed the machine (must be none) ---"
      sed 's/\x1b\[[0-9;]*m//g' "$art/panel.log" |
        grep -iE "joining|secret agent registered|RequestScan" || echo "none"
    } >"$art/real-audit.txt" 2>&1
    echo "--- read-only audit ---"
    cat "$art/real-audit.txt"
    ;;

  *)
    echo "unknown scenario: $scenario" >&2
    exit 1
    ;;
esac
