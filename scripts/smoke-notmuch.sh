#!/usr/bin/env sh
# The unread-mail matrix, driven inside the nested niri session.
#
#   nix develop -c ./scripts/smoke-notmuch.sh
#
# NOTHING HERE READS ANYBODY REAL MAIL. Every scenario points the panel at a
# fake `notmuch` written by this script, through TOPBAR_SMOKE_NOTMUCH, which
# names the program outright rather than relying on PATH order: SMOKE_PATH
# prepends, so a real notmuch would still be one directory behind a fake of the
# same name. NOTMUCH_CONFIG is pointed at a fixture as a second belt, and the
# fake refuses to run at all unless it sees that fixture, which is the third.
#
#   1  unread    twelve unread messages in three conversations: the envelope
#   2  popover   the list: sender, subject, and notmuch own relative time
#   3  empty     nothing unread: the widget is ABSENT, not a zero
#   4  missing   no notmuch on this machine: absent, and the log says why
#   5  garbage   notmuch printing something unreadable: absent, not zero
#
# Screenshots and captured output land in target/visual-smoke/notmuch/.
set -eu

trap 'pkill -f "target/debug/topbar-fake-" 2>/dev/null || true' EXIT INT TERM

artifact_root="${1:-target/visual-smoke/notmuch}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in magick niri grim cargo timeout dbus-run-session; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)
live_config="crates/topbar-core/tests/fixtures/live-config.toml"
fixtures="$artifact_root/fixtures"
mkdir -p "$fixtures"

# The live fixture is a byte-for-byte copy of the v1 configuration and must not
# be edited, so the widget is added to a copy of it.
config="$artifact_root/notmuch-config.toml"
sed 's/^right = \[$/right = [\n  "notmuch",/' "$live_config" >"$config"
grep -q '"notmuch",' "$config" || {
  echo "smoke-notmuch: the widget was not placed in $config" >&2
  exit 1
}

# A notmuch configuration that names a maildir which does not exist. The fake
# never opens it; it is here so that a real notmuch reached by accident would
# fail loudly rather than quietly reading the developer own database.
notmuch_config="$fixtures/notmuch-config"
cat >"$notmuch_config" <<EOF
[database]
path=$fixtures/no-such-maildir
EOF

# --- the fake -----------------------------------------------------------
# Written per scenario, because what it prints is the scenario.
make_notmuch() {
  name=$1
  count=$2
  search=$3

  bin="$fixtures/$name-bin"
  rm -rf "$bin"
  mkdir -p "$bin"

  cat >"$bin/notmuch" <<EOF
#!/usr/bin/env sh
# A stand-in for notmuch that never opens a database. It refuses to run unless
# it can see the sandbox configuration, so a copy of this script left somewhere
# else cannot stand in for the real thing.
if [ "\${NOTMUCH_CONFIG:-}" != "$notmuch_config" ]; then
  echo "fake notmuch: refusing to run outside the smoke sandbox" >&2
  exit 64
fi
case "\$1" in
  count) printf '%s\n' '$count' ;;
  search) printf '%s\n' '$search' ;;
  *) echo "fake notmuch: unexpected subcommand \$1" >&2; exit 1 ;;
esac
EOF
  chmod +x "$bin/notmuch"
  echo "$bin/notmuch"
}

# Three conversations, twelve messages: the list is shorter than the count on
# purpose, which is the case the popover has to explain rather than hide.
threads='[{"thread":"0000000000000691","timestamp":1786328986,"date_relative":"Today 02:29","matched":9,"total":103,"authors":"Eli Zaretskii, Stefan Monnier| tomas@tuxteam.de","subject":"Re: bug#79231: seq-uniq is quadratic","tags":["inbox","unread"]},{"thread":"00000000000004d2","timestamp":1786300000,"date_relative":"Yest. 18:26","matched":2,"total":4,"authors":"Ludovic Courtes","subject":"[PATCH] gnu: guile-fibers: Update to 1.3.1","tags":["inbox","unread"]},{"thread":"00000000000001a4","timestamp":1786200000,"date_relative":"Sat. 11:02","matched":1,"total":1,"authors":"noreply@forge.example","subject":"","tags":["inbox","unread"]}]'

unread_bin=$(make_notmuch unread "12	smoke-uuid	42" "$threads")
empty_bin=$(make_notmuch empty "0	smoke-uuid	42" '[]')
garbage_bin=$(make_notmuch garbage "notmuch: unknown option" 'not json at all')
missing_bin="$fixtures/no-such-notmuch"

# Every fake has to answer before a single screenshot is taken, or the run
# photographs a panel with nothing behind it and calls that a result.
for fake in "$unread_bin" "$empty_bin" "$garbage_bin"; do
  if ! NOTMUCH_CONFIG="$notmuch_config" "$fake" count --lastmod 'tag:unread' >/dev/null; then
    echo "smoke-notmuch: the fake at $fake did not answer" >&2
    exit 1
  fi
done
# And the belt itself has to hold.
if NOTMUCH_CONFIG=/etc/notmuch-config "$unread_bin" count --lastmod x >/dev/null 2>&1; then
  echo "smoke-notmuch: the fake ran with a configuration outside the sandbox" >&2
  exit 1
fi
if [ -e "$missing_bin" ]; then
  echo "smoke-notmuch: $missing_bin exists; the missing scenario proves nothing" >&2
  exit 1
fi

# One nested session: run <scenario> <notmuch> [open]
run() {
  scenario=$1
  program=$2
  open=${3:-}

  echo "smoke-notmuch: $scenario"
  if [ -n "$open" ]; then
    export TOPBAR_SMOKE_OPEN="$open"
  else
    unset TOPBAR_SMOKE_OPEN
  fi

  RUST_LOG="topbar_services::notmuch=debug,topbar_services::proc=debug" \
  SMOKE_NOTMUCH_SCENARIO="$scenario" \
  NOTMUCH_CONFIG="$notmuch_config" \
  TOPBAR_SMOKE_NOTMUCH="$program" \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-90}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-notmuch-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$scenario" \
    >"$artifact_root/$scenario.log" 2>&1 ||
    echo "smoke-notmuch: $scenario exited non-zero; see $artifact_root/$scenario.log" >&2
  mv "$artifact_root/$scenario/panel.log" "$artifact_root/$scenario-panel.log" 2>/dev/null || true
}

run unread "$unread_bin"
run popover "$unread_bin" notmuch
run empty "$empty_bin"
run missing "$missing_bin"
run garbage "$garbage_bin"

echo "--- screenshots ---"
find "$artifact_root" -name '*.png' | sort
