#!/usr/bin/env sh
# The three M10 widgets that spend most of their life invisible, driven inside
# the nested niri session:
#
#   nix develop -c ./scripts/smoke-widgets.sh
#
# Each scenario is its own nested session, because each needs a different
# configuration and the panel reads that once at start-up. Every configuration
# is the live one with a single section rewritten, so the panel around the
# widget under test is the panel the user actually runs.
#
#   monitor    the system monitor under a spinner: hidden while healthy, an
#              icon and a reading once its threshold has been crossed twice,
#              and gone again after the hysteresis lets go
#   headset    45% and discharging, then charging, then the tool taken away
#   plain      a custom widget printing a line
#   json       one printing Waybar JSON with `class: "warning"`
#   empty      one printing nothing, which takes the widget off the bar
#   template   one whose output goes through `{output}`
#   failing    one that works once and then exits 1: the value stays
#   offline    one with `requires_network`, started on a disconnected
#              NetworkManager and then reconnected
#
# ## Two things this script is careful about
#
# The monitor scenario needs the machine to look busy. It runs **one** shell
# spinner for six seconds against a `cpu_threshold` of 5 rather than saturating
# the developer's cores against the real threshold of 90; the spinner stops by
# itself on a deadline and is killed in the trap as well.
#
# Every fake is reaped on exit for the same reason: they do not die with the
# private bus, and an interrupted run once left a pile of them alive for hours.
#
# Screenshots land in target/visual-smoke/widgets/<scenario>/.
set -eu

trap 'pkill -f "target/debug/topbar-fake-" 2>/dev/null || true
      pkill -f topbar-smoke-spinner 2>/dev/null || true' EXIT INT TERM

artifact_root="${1:-target/visual-smoke/widgets}"
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd)

for tool in magick niri grim cargo timeout dbus-run-session gdbus; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing required tool: $tool" >&2
    exit 1
  fi
done

repo=$(pwd)
live_config="$repo/crates/topbar-core/tests/fixtures/live-config.toml"

bin="$artifact_root/bin"
mkdir -p "$bin"

# --- the fixtures on PATH --------------------------------------------------

# The headset tool, reading a state file so the driver can change what it says
# underneath a running panel.
headset_state="$artifact_root/headset.json"
cat >"$bin/headsetcontrol" <<SH
#!/usr/bin/env sh
cat "$headset_state"
SH
chmod +x "$bin/headsetcontrol"

# One core, for a fixed number of seconds, with no forking inside the loop —
# it checks the clock once every twenty thousand iterations rather than once
# per iteration, which would spawn \`date\` thousands of times a second. The
# deadline is its own belt: even a driver that died without running its trap
# cannot leave this behind for more than a few seconds.
cat >"$bin/topbar-smoke-spinner" <<'SH'
#!/usr/bin/env sh
end=$(($(date +%s) + ${1:-6}))
while :; do
  i=0
  while [ "$i" -lt 20000 ]; do i=$((i + 1)); done
  [ "$(date +%s)" -ge "$end" ] && break
done
SH
chmod +x "$bin/topbar-smoke-spinner"

# --- the scripts a custom widget runs --------------------------------------

write_script() {
  path="$bin/$1"
  shift
  printf '#!/usr/bin/env sh\n%s\n' "$*" >"$path"
  chmod +x "$path"
  echo "$path"
}

plain=$(write_script plain.sh "echo 'BTC 103412'")
json=$(write_script json.sh \
  "printf '{\"text\":\"7 due\",\"tooltip\":\"7 updates pending\",\"class\":\"warning\"}\n'")
empty=$(write_script empty.sh "exit 0")
template=$(write_script template.sh "echo 21")
# Counts its own runs, so a screenshot says how many there have been. That is
# what makes a *deferral* visible: a number that has not moved is a run that
# did not happen, which no amount of staring at an unchanged price would show.
counter="$artifact_root/runs"
online=$(write_script online.sh "n=\$(cat '$counter' 2>/dev/null || echo 0)
n=\$((n + 1))
echo \"\$n\" >'$counter'
echo \"run \$n\"")
rm -f "$counter"
# Works once and then fails, so the widget has a value to keep. The marker
# lives in the run's own artifact directory and nowhere shared.
marker="$artifact_root/failed-once"
failing=$(write_script failing.sh "if [ -f '$marker' ]; then exit 1; fi
touch '$marker'
echo 'BTC 103412'")
rm -f "$marker"

# --- the configurations ----------------------------------------------------
#
# `sed` ranges rather than whole-file substitutions: `interval` appears in four
# sections of the live file, and rewriting all of them would be rewriting a
# different configuration from the one under test.

# Two rewrites every scenario gets.
#
# The weather comes off the bar: it has no coordinates in the live file — the
# user picked their location in v1's dialog, which lives in state.json — so it
# would draw "Configure…" in every frame, which is a distraction rather than a
# fact about the widget under test.
#
# The crypto script is pointed at a fixture. The user's own reaches CoinGecko
# through curl and jq; a smoke run must not depend on the internet, and must
# not put a request on it either.
custom='/^\[widgets.custom-crypto\]$/,/^$/'
monitor='/^\[widgets.system_monitor\]$/,/^$/'

base_config() {
  name=$1
  script=$2
  out="$artifact_root/$name-config.toml"
  sed -e 's/^center = \["weather", "clock"\]$/center = ["clock"]/' \
    -e "$custom s#^exec = .*#exec = \"$script\"#" \
    "$live_config" >"$out"
  grep -q "^exec = \"$script\"$" "$out" || {
    echo "could not point custom-crypto at $script" >&2
    exit 1
  }
  echo "$out"
}

# A custom-widget scenario: the base, plus this widget's own interval and an
# optional template line inserted into its section. `sed` ranges rather than
# whole-file substitutions, because `interval` appears in four sections of the
# live file and rewriting all of them would be testing a different config.
custom_config() {
  out=$(base_config "$1" "$2")
  interval=${3:-1800}
  template_line=${4:-}
  sed -i -e "$custom s#^interval = .*#interval = $interval#" "$out"
  if [ -n "$template_line" ]; then
    sed -i -e "$custom s#^exec = .*#&\n$template_line#" "$out"
  fi
  echo "$out"
}

# The system monitor watching a threshold one spinner can cross, sampled every
# second so two consecutive samples take two seconds rather than ten.
#
# Fifteen percent, not five. An idle nested session is not idle — it renders,
# and `grim` reads the frame back — so it sits around eight percent of this
# machine, which a threshold of five never clears again. One busy shell adds
# about eleven points, which is the whole point: the widget has to appear
# because of the spinner and disappear when it stops.
#
# Memory and disk are pushed out of the way so the frame is about the CPU. A
# developer whose disk is 91% full would otherwise get a second icon that has
# nothing to do with what this scenario is checking.
monitor_config=$(base_config monitor "$plain")
sed -i \
  -e "$monitor s/^cpu_threshold = .*/cpu_threshold = 15/" \
  -e "$monitor s/^memory_threshold = .*/memory_threshold = 99/" \
  -e "$monitor s/^interval = .*/interval = 1/" \
  -e "$monitor s/^tooltip = .*/tooltip = \"System load\"\ndisk_threshold = 99/" \
  "$monitor_config"
grep -q '^cpu_threshold = 15$' "$monitor_config" || {
  echo "could not lower the CPU threshold" >&2
  exit 1
}
grep -q '^disk_threshold = 99$' "$monitor_config" || {
  echo "could not raise the disk threshold" >&2
  exit 1
}

headset_config=$(base_config headset "$plain")

# --- one nested session ----------------------------------------------------
#
# run <scenario> <config>. Anything else a scenario needs — a NetworkManager of
# its own, say — is exported by the caller around the call: a shell only treats
# `VAR=value` as an assignment when it is literal at parse time, so passing one
# through "$@" would try to run it as a program.
run() {
  scenario=$1
  config=$2

  echo "smoke-widgets: $scenario"
  SMOKE_WIDGET_SCENARIO="$scenario" \
  SMOKE_HEADSET_STATE="$headset_state" \
  SMOKE_HEADSET_TOOL="$bin/headsetcontrol" \
  SMOKE_PATH="$bin" \
  RUST_LOG="info,topbar::bar=debug,topbar::widgets=debug,topbar_services::custom=debug,topbar_services::headset=debug" \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-90}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-widgets-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$scenario" \
    >"$artifact_root/$scenario.log" 2>&1 ||
    echo "smoke-widgets: $scenario exited non-zero; see $artifact_root/$scenario.log" >&2
  mv "$artifact_root/$scenario/panel.log" "$artifact_root/$scenario-panel.log" 2>/dev/null || true
}

# The headset starts at 45% and discharging; the driver moves it from there.
reset_headset() {
  cat >"$headset_state" <<'JSON'
{"devices":[{"status":"success","device":"Arctis Nova 7",
  "battery":{"status":"BATTERY_AVAILABLE","level":45}}]}
JSON
  cat >"$bin/headsetcontrol" <<SH
#!/usr/bin/env sh
cat "$headset_state"
SH
  chmod +x "$bin/headsetcontrol"
}
reset_headset

run monitor "$monitor_config"
run headset "$headset_config"
reset_headset

run plain "$(custom_config plain "$plain")"
run json "$(custom_config json "$json")"
run empty "$(custom_config empty "$empty")"
run template "$(custom_config template "$template" 1800 'template = "{output}°C"')"
run failing "$(custom_config failing "$failing" 5)"

# `--state 20` is NM_STATE_DISCONNECTED, and the widget runs every three
# seconds. The very first run still happens: connectivity starts optimistic on
# purpose — the panel is up before it has talked to any bus, and starting
# "offline" would put the first fetch behind a round trip that may never come
# back. Everything after it is deferred, which is what a run counter that has
# stopped moving proves. The driver then reconnects over the fake's own control
# interface and the counter advances by exactly one.
offline_config=$(custom_config offline "$online" 3)
export TOPBAR_SMOKE_NM="--state 20"
run offline "$offline_config"
unset TOPBAR_SMOKE_NM

echo "--- screenshots ---"
find "$artifact_root" -name '*.png' | sort
echo "--- panel logs ---"
for log in "$artifact_root"/*-panel.log; do
  [ -f "$log" ] || continue
  echo "=== $log ==="
  cat "$log"
done
