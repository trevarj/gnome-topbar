#!/bin/bash
cargo build --release -p gnome-panel && target/release/gnome-panel 2>&1 | tee /tmp/gnome-panel-debug.log
