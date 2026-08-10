#!/usr/bin/env sh
# The unread-mail driver, run inside the nested niri session by
# scripts/smoke-notmuch.sh. One scenario per session, named by
# $SMOKE_NOTMUCH_SCENARIO.
#
# The notmuch on this session PATH is irrelevant: the panel runs the program
# TOPBAR_SMOKE_NOTMUCH names, which is a script the parent wrote and which
# refuses to run unless it sees the sandbox configuration. No database is
# opened and no maildir is read.
set -eu

. "$(dirname "$0")/smoke-shot.sh"

scenario="${SMOKE_NOTMUCH_SCENARIO:-unread}"
art="$SMOKE_ARTIFACTS"

# What the panel decided about mail, out of its own log.
show_log() {
  grep -i "notmuch" "$art/panel.log" >"$art/$1.txt" 2>/dev/null || true
  echo "--- what the panel decided ($1) ---"
  cat "$art/$1.txt" 2>/dev/null || true
}

case "$scenario" in
  unread)
    # (a) twelve unread messages. The envelope is on the bar; the count is in
    # the tooltip, which no screenshot can show, so the log is the evidence
    # that the number reached the panel.
    shot notmuch-unread
    show_log unread-decision
    ;;

  popover)
    # (b) the list. Three conversations against a count of twelve, on purpose:
    # the header carries the messages, the rows carry the conversations, and
    # the one holding nine of them says so.
    shot notmuch-popover topbar-popover
    show_log popover-decision
    ;;

  empty)
    # (c) nothing unread. The widget is ABSENT — not greyed out, not a zero.
    # The screenshot is of the bar without it.
    shot notmuch-empty
    show_log empty-decision
    ;;

  missing)
    # (d) no notmuch on this machine at all. Absent for a different reason,
    # and the log line is the one that says which.
    shot notmuch-missing
    show_log missing-decision
    ;;

  garbage)
    # (e) a notmuch that answers with something this code cannot read. This is
    # the one that must not become a zero: an inbox reading empty because the
    # panel could not parse the answer is the failure a mail indicator is not
    # allowed to have.
    shot notmuch-garbage
    show_log garbage-decision
    ;;

  *)
    echo "unknown scenario: $scenario" >&2
    exit 1
    ;;
esac
