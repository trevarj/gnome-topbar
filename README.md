# GNOME Topbar

A Wayland-only GTK top bar inspired by GNOME Shell for compositors such as
Niri. GNOME Topbar provides a continuous system panel, integrated
notifications, quick settings, media controls, workspaces, and scriptable
status modules.

This project is inspired by GNOME Shell's top bar. It is not affiliated with
or endorsed by the GNOME project.

## Goals

- Follow GNOME Shell top-bar design: quiet, continuous, system-owned, and
  low-distraction.
- Prefer Niri-first behavior while keeping the existing Wayland compositor
  backend architecture available.
- Keep custom script modules useful for migrations from Waybar.
- Grow common script use cases into tested native Rust modules over time.
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

The shipped default is a GNOME Shell-style top bar:

- left: Niri workspaces
- center: clock, calendar, notifications, media, and optional weather
- right: tray plus one Quick Settings aggregate for network, audio, battery,
  Bluetooth, and VPN

Standalone status widgets such as `battery`, `keyboard_layout`,
`notifications`, and custom scripts remain available when explicitly added to
a widget list.

## Native Script Widgets

Weather and headset are first-class widgets with the same script output
contract as custom widgets:

```toml
[widgets]
center = ["clock"] # weather auto-places next to clock
right = ["headset", "quick_settings"]

[widgets.clock]
control_panel = true
media_thumbnail = false # compact active-media companion
media_eq = true

[widgets.weather]
exec = "~/.config/gnome-topbar/scripts/weather.sh"
interval = 1800
position = "left" # "left" or "right" of the clock
tooltip = "Weather"

[widgets.headset]
exec = "~/.config/gnome-topbar/scripts/headsetcontrol.sh"
interval = 5
tooltip = "Headset battery"
```

Set `disabled = true` under either widget table to hide it. When weather is
enabled, the clock control panel also uses its `exec` output for the weather
line.

## Custom Scripts

Custom widgets are intended for migration and extensibility. They use the
`custom-` prefix and can poll shell commands:

```toml
[widgets]
left = ["workspaces", "custom-crypto"]

[widgets.custom-crypto]
exec = "~/.config/gnome-topbar/scripts/crypto.sh -r"
interval = 1800
tooltip = "Crypto prices"
```

Waybar-style custom script output is supported for migration. Scripts may emit
plain text or JSON:

```json
{"text":"󰋎 ","tooltip":"Headset: 72%","percentage":72}
```

The `text` field is displayed on the bar; `label` is accepted as a fallback
display field for simple scripts. `tooltip` is used as the widget tooltip.
When `tooltip` is absent, `percentage` is shown as a percentage tooltip.
Empty text hides the widget when no static config `label` fallback is set.

See [docs/waybar-migration.md](docs/waybar-migration.md) for migration
examples.

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

## License

MIT
