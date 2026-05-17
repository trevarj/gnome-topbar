#!/usr/bin/env bash
set -euo pipefail

cargo run --release -p gnome-topbar -- --config config.toml -v 2>&1 | tee /tmp/gnome-topbar-debug.log
