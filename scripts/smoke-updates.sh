#!/usr/bin/env sh
# The updates and resources matrix, driven inside the nested niri session.
#
#   nix develop -c ./scripts/smoke-updates.sh
#
# The updates scenarios put a *fake package manager* on the session's PATH and
# a *fake /etc/os-release* in its sandbox, so the panel can be on Arch or
# Debian without the machine being either — and so nothing here ever runs the
# real `apt-get`, `dnf` or `checkupdates`. The panel reads the copy through
# TOPBAR_SMOKE_ROOT; /etc itself is only ever read.
#
# The resources scenario is the opposite and deliberately so: it reads the
# machine's real /proc, because that is read-only, and because a fake /proc
# would prove the parser works on a fixture the parser tests already cover.
# What this proves is that the numbers on screen are this machine's.
#
#   1  arch      an Arch machine with seven pending updates
#   2  debian    a Debian machine, counted from apt's simulation
#   3  current   an Arch machine with nothing pending: the card is ABSENT
#   4  nixos     NixOS with no override: the card is absent, and the log says
#                what to configure
#   5  resources the overview against the real /proc: CPU, memory, swap, disks
#
# Screenshots and captured output land in target/visual-smoke/upd/<scenario>/.
set -eu

# These runs start no D-Bus sidecars — the fake package managers are shell
# scripts that exit on their own — but the harness they call still brings up a
# nested compositor, so the same guard is kept for the day a scenario here does
# need a fake.
trap 'pkill -f "target/debug/topbar-fake-" 2>/dev/null || true' EXIT INT TERM

artifact_root="${1:-target/visual-smoke/upd}"
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
# The live configuration sets `update_count_command = "guix ..."`, which is
# exactly the dead-command path — right for the user, useless for a screenshot
# of a populated card. These runs use a config with the key removed so the
# auto-detection is what is on trial.
config="$artifact_root/auto-config.toml"
grep -v '^update_count_command' "$live_config" >"$config"

fixtures="$artifact_root/fixtures"
mkdir -p "$fixtures"

# The os-release files, byte for byte what those distributions ship.
cat >"$fixtures/arch.os-release" <<'EOF'
NAME="Arch Linux"
PRETTY_NAME="Arch Linux"
ID=arch
BUILD_ID=rolling
EOF
cat >"$fixtures/debian.os-release" <<'EOF'
PRETTY_NAME="Debian GNU/Linux 12 (bookworm)"
NAME="Debian GNU/Linux"
VERSION_ID="12"
ID=debian
EOF
cat >"$fixtures/nixos.os-release" <<'EOF'
ID=nixos
NAME=NixOS
PRETTY_NAME="NixOS 26.05 (Warbler)"
VERSION_ID="26.05"
EOF

# A PATH holding nothing but the fake package managers, so `checkupdates`
# inside the session is this script's and never pacman-contrib's.
make_bin() {
  bin="$fixtures/$1-bin"
  rm -rf "$bin"
  mkdir -p "$bin"
  echo "$bin"
}

arch_bin=$(make_bin arch)
cat >"$arch_bin/checkupdates" <<'EOF'
#!/usr/bin/env sh
cat <<'PACKAGES'
linux 6.12.4.arch1-1 -> 6.12.5.arch1-1
mesa 1:24.3.1-1 -> 1:24.3.2-1
firefox 133.0-1 -> 133.0.3-1
sqlite 3.47.1-1 -> 3.47.2-1
systemd 257.1-1 -> 257.2-1
vim 9.1.0866-1 -> 9.1.0910-1
git 2.47.1-1 -> 2.47.2-1
PACKAGES
EOF

current_bin=$(make_bin current)
# `checkupdates` exits 2 with nothing on stdout when there is nothing to do,
# which is its documented contract and not the same thing as a failure.
cat >"$current_bin/checkupdates" <<'EOF'
#!/usr/bin/env sh
exit 2
EOF

debian_bin=$(make_bin debian)
cat >"$debian_bin/apt-get" <<'EOF'
#!/usr/bin/env sh
cat <<'SIMULATION'
NOTE: This is only a simulation!
Reading package lists...
Building dependency tree...
Calculating upgrade...
The following packages will be upgraded:
  base-files libc6 libssl3
3 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.
Inst base-files [12.4+deb12u5] (12.4+deb12u7 Debian:12.8/stable [amd64])
Conf base-files (12.4+deb12u7 Debian:12.8/stable [amd64])
Inst libc6 [2.36-9+deb12u7] (2.36-9+deb12u9 Debian:12.8/stable [amd64])
Conf libc6 (2.36-9+deb12u9 Debian:12.8/stable [amd64])
Inst libssl3 [3.0.14-1~deb12u2] (3.0.15-1~deb12u1 Debian:12.8/stable [amd64])
SIMULATION
EOF

chmod +x "$arch_bin"/* "$current_bin"/* "$debian_bin"/*

# One nested session: run <scenario> <os-release> <bin-dir>
run() {
  scenario=$1
  release=${2:-}
  bin=${3:-}

  echo "smoke-upd: $scenario"
  if [ -n "$release" ]; then
    export TOPBAR_SMOKE_OSRELEASE="$fixtures/$release"
  else
    unset TOPBAR_SMOKE_OSRELEASE
  fi
  if [ -n "$bin" ]; then
    export SMOKE_PATH="$bin"
  else
    unset SMOKE_PATH
  fi

  RUST_LOG="topbar_services::updates=debug,topbar_services::resources=debug,topbar_services::proc=debug" \
  SMOKE_UPD_SCENARIO="$scenario" \
  TOPBAR_SMOKE_OPEN=quick_settings \
  TOPBAR_SMOKE_TIMEOUT="${TOPBAR_SMOKE_TIMEOUT:-90}" \
  TOPBAR_SMOKE_DRIVER="$repo/scripts/smoke-updates-shot.sh" \
  TOPBAR_VISUAL_CONFIG="$config" \
    "$repo/scripts/visual-smoke-niri.sh" "$artifact_root/$scenario" \
    >"$artifact_root/$scenario.log" 2>&1 ||
    echo "smoke-upd: $scenario exited non-zero; see $artifact_root/$scenario.log" >&2
  mv "$artifact_root/$scenario/panel.log" "$artifact_root/$scenario-panel.log" 2>/dev/null || true
}

run arch arch.os-release "$arch_bin"
run debian debian.os-release "$debian_bin"
run current arch.os-release "$current_bin"
run nixos nixos.os-release ""
run resources arch.os-release "$arch_bin"

echo "--- screenshots ---"
find "$artifact_root" -name '*.png' | sort
