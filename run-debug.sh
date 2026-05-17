#!/usr/bin/env bash
set -euo pipefail

cargo run --release -p gnome-panel -- --config config.toml -v 2>&1 | tee /tmp/gnome-panel-debug.log
