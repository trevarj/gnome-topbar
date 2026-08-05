#!/usr/bin/env sh
# The updates and resources driver, run inside the nested niri session by
# scripts/smoke-updates.sh. One scenario per session, named by
# $SMOKE_UPD_SCENARIO.
#
# Nothing here runs a real package manager: the ones on this session's PATH are
# scripts the parent wrote, and the /etc/os-release the panel reads is a copy in
# the sandbox. /proc, on the other hand, is the machine's own — reading it is
# what the resources card *is*.
set -eu

. "$(dirname "$0")/smoke-shot.sh"

scenario="${SMOKE_UPD_SCENARIO:-arch}"
art="$SMOKE_ARTIFACTS"

# What the panel decided about updates, out of its own log.
show_log() {
  grep -i "updates:\|resources:" "$art/panel.log" >"$art/$1.txt" 2>/dev/null || true
  echo "--- what the panel decided ($1) ---"
  cat "$art/$1.txt" 2>/dev/null || true
}

case "$scenario" in
  arch)
    # (a) seven pending updates, counted by this run's own `checkupdates`.
    # The card is visible and names the first three packages.
    shot updates-arch topbar-popover
    show_log arch-decision
    ;;

  debian)
    # (b) the same card from an entirely different contract: apt's simulation,
    # counted by its `Inst ` lines rather than by its whole output.
    shot updates-debian topbar-popover
    show_log debian-decision
    ;;

  current)
    # (c) a machine with nothing pending. The card is ABSENT — not greyed out,
    # not reading "Up to date". The screenshot is of the panel without it.
    shot updates-none topbar-popover
    show_log current-decision
    ;;

  nixos)
    # (d) NixOS with no override. The card is absent for a different reason,
    # and the log line is what tells the user how to get one.
    shot updates-nixos topbar-popover
    show_log nixos-decision
    ;;

  resources)
    # (e) the overview against this machine's real /proc. The first frame has
    # no CPU number — there is nothing to subtract from — so the shot is taken
    # after a second sample has landed.
    sleep 8
    shot resources topbar-popover
    {
      echo "--- what the machine says ---"
      grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo
      echo "--- and its filesystems ---"
      df -h --output=target,size,used,pcent -x tmpfs -x devtmpfs 2>/dev/null | head -10
    } >"$art/machine.txt" 2>&1 || true
    echo "--- corroboration ---"
    cat "$art/machine.txt"
    show_log resources-decision
    ;;

  *)
    echo "unknown scenario: $scenario" >&2
    exit 1
    ;;
esac
