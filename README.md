# GNOME Topbar

A Wayland-only GTK top bar inspired by GNOME Shell for Niri. GNOME Topbar
provides a continuous system panel with workspaces, an integrated clock control
panel, tray, Quick Settings, and a narrow `custom-*` escape hatch.

This project is inspired by GNOME Shell's top bar. It is not affiliated with
or endorsed by the GNOME project.

![GNOME Topbar running on Niri](assets/screenshots/gnome-topbar.png)

## Goals

- Follow GNOME Shell top-bar design: quiet, continuous, system-owned, and
  low-distraction.
- Prefer Niri-first behavior.
- Treat mass code reduction and simplification as a primary goal.
- Keep `custom-*` useful for one-off indicators without becoming a
  general-purpose Waybar replacement.
- Stay idiomatic Rust: typed config boundaries, explicit parsing, small
  services, and tests with new behavior.

## Configuration

GNOME Topbar looks for configuration in:

```sh
~/.config/gnome-topbar/config.toml
```

Print the default configuration:

```sh
gnome-topbar --print-example-config
```

Validate a configuration:

```sh
gnome-topbar --check-config --config ~/.config/gnome-topbar/config.toml
```

Dump the built-in defaults:

```sh
gnome-topbar dump default-config ~/.config/gnome-topbar/config.toml
```

The shipped default is a GNOME Shell-style top bar:

- left: Niri workspaces
- center: clock, calendar, notifications, and media
- right: tray plus one Quick Settings aggregate for network, audio, battery,
  Bluetooth, VPN, updates, and idle inhibitor

Standalone Waybar-style status widgets have been removed from the supported
surface. Unknown or removed widget names produce configuration warnings and are
skipped. CPU, memory, and disk status lives in Quick Settings.

## Updates

Guix update counting is disabled until configured:

```toml
[updates]
update_count_command = "" # print a number, or one update per line
check_interval = 3600
```

## Battery Health

The Quick Settings battery health controls need kernel charge threshold files
and UPower threshold support. On systems where UPower does not auto-detect the
vendor preset, add hwdb data for the battery so `upower -i` reports
`charge-threshold-supported: yes` and the intended start/end limits.

The topbar prefers direct sysfs writes when the threshold files are writable by
the user running `gnome-topbar`, then refreshes its state from sysfs. UPower may
lag after threshold changes, so sysfs remains the source of truth when both are
available.

On Guix System, configure this declaratively:

- Add UPower/hwdb data that exposes the battery charge limit, for example
  `CHARGE_LIMIT=75,80` for BAT0.
- Add a udev rule that grants the topbar user write access to
  `/sys/class/power_supply/BAT0/charge_control_start_threshold` and
  `/sys/class/power_supply/BAT0/charge_control_end_threshold`.
- Reconfigure the system, reload udev, and trigger the BAT0 power-supply
  device or reboot.

Useful checks:

```sh
upower -i /org/freedesktop/UPower/devices/battery_BAT0
ls -l /sys/class/power_supply/BAT0/charge_control_*_threshold
```

## Custom Scripts

Custom widgets are intended for small one-off indicators. They use the
`custom-` prefix and can poll shell commands:

```toml
[widgets]
left = ["workspaces", "custom-crypto"]

[widgets.custom-crypto]
exec = "~/.config/gnome-topbar/scripts/crypto.sh -r"
interval = 1800
tooltip = "Crypto prices"
```

Weather and headset battery indicators are built in, so common panel status
does not need wrapper scripts:

```toml
[widgets]
center = ["weather", "clock"]
right = ["tray", "headset", "quick_settings"]

[widgets.weather]
# latitude = 0.0
# longitude = 0.0
unit = "celsius"
interval = 1800

[widgets.headset]
interval = 5
```

Resource overview is built into Quick Settings:

```toml
[widgets.quick_settings]
resource_overview = true
```

Waybar-style custom script output is supported. Scripts may emit
plain text or JSON:

```json
{"text":"󰋎 ","tooltip":"Headset: 72%","percentage":72}
```

The `text` field is displayed on the bar; `label` is accepted as a fallback
display field for simple scripts. `tooltip` is used as the widget tooltip.
When `tooltip` is absent, `percentage` is shown as a percentage tooltip.
Empty text hides the widget when no static config `label` fallback is set.

See [docs/project-goals.md](docs/project-goals.md) for the product boundary.

## Development

Use direnv for the Guix development environment:

```sh
direnv allow
direnv reload
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

Run locally:

```sh
cargo run -p gnome-topbar -- --config config.toml -v
```

Run the Niri visual smoke check for design changes:

```sh
guix shell -m manifest.scm -- ./scripts/visual-smoke-niri.sh
```

The screenshot is written to `target/visual-smoke/gnome-topbar.png`.

Build the Guix package:

```sh
guix build -f guix/gnome-topbar.scm
```

The in-repo Guix package uses the local checkout as its source. For Guix Home
or System configs that consume the fork, pin `https://github.com/trevarj/gnome-topbar`
to a commit and hash in your own package module.

## License

MIT
