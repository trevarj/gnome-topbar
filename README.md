# GNOME Topbar

A Wayland-only GTK top bar inspired by GNOME Shell for Niri. GNOME Topbar
provides a continuous system panel with workspaces, an integrated clock control
panel, tray, Quick Settings, and a narrow `custom-*` escape hatch.

This project is inspired by GNOME Shell's top bar. It is not affiliated with
or endorsed by the GNOME project.

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
gnome-topbar dump default-css ~/.config/gnome-topbar/style.css
```

The shipped default is a GNOME Shell-style top bar:

- left: Niri workspaces
- center: clock, calendar, notifications, and media
- right: tray plus one Quick Settings aggregate for network, audio, battery,
  Bluetooth, VPN, updates, and idle inhibitor

Standalone Waybar-style status widgets have been removed from the supported
surface. Unknown or removed widget names produce configuration warnings and are
skipped.

## Updates

Guix update counting is disabled until configured:

```toml
[updates]
update_count_command = "" # print a number, or one update per line
check_interval = 3600
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

The clock control panel can read a custom weather script without placing that
script on the bar:

```toml
[widgets.clock]
control_panel = true
control_panel_weather_widget = "custom-weather"

[widgets.custom-weather]
exec = "~/.config/gnome-topbar/scripts/weather.sh"
interval = 1800
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
