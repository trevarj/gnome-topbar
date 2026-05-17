# GNOME Panel

A Wayland-only GTK top bar inspired by GNOME Shell for compositors such as
Niri. GNOME Panel provides a continuous system panel, integrated
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

GNOME Panel looks for configuration in:

```sh
~/.config/gnome-panel/config.toml
```

Print the default configuration:

```sh
gnome-panel --print-example-config
```

Validate a configuration:

```sh
gnome-panel --check-config --config ~/.config/gnome-panel/config.toml
```

## Custom Scripts

Custom widgets use the `custom-` prefix and can poll shell commands:

```toml
[widgets]
left = ["workspaces", "custom-weather"]

[widgets.custom-weather]
exec = "~/.config/gnome-panel/scripts/weather.sh"
interval = 1800
tooltip = "Weather"
```

Waybar-style custom script output is supported for migration. Scripts may emit
plain text or JSON:

```json
{"text":"󰋎 ","tooltip":"Headset: 72%","percentage":72}
```

The `text` field is displayed on the bar. `tooltip` is used as the widget
tooltip. Empty text hides the widget when no static `label` fallback is set.

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
cargo run -p gnome-panel -- --config config.toml -v
```

Build the Guix package:

```sh
guix build -f guix/gnome-panel.scm
```

## License

MIT
