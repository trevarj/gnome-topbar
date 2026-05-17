#!/bin/bash
cargo build --release -p gnome-topbar && target/release/gnome-topbar 2>&1 | tee /tmp/gnome-topbar-debug.log
