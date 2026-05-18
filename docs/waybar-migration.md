# Waybar Custom Script Migration

GNOME Topbar is not a Waybar replacement. It supports a narrow `custom-*`
escape hatch for small status scripts, while core system controls stay in the
clock panel or Quick Settings.

Use `custom-*` for one-off scripts:

```toml
[widgets]
left = ["workspaces", "custom-crypto"]
center = ["clock"]
right = ["tray", "quick_settings"]

[widgets.clock]
control_panel = true
control_panel_weather_widget = "custom-weather"

[widgets.custom-crypto]
exec = "~/.config/gnome-topbar/scripts/crypto.sh -r"
interval = 1800
max_chars = 40

[widgets.custom-weather]
exec = "~/.config/gnome-topbar/scripts/weather.sh"
interval = 1800
```

VPN status belongs in Quick Settings. GNOME Topbar detects NetworkManager VPNs
and active external tunnel interfaces such as `tun0`/`wg0`; keep one-off VPN
scripts out of the bar unless you need a deliberately separate indicator.

Standalone widget names such as `battery`, `weather`, `headset`, `media`,
`notifications`, and resource monitors are intentionally outside the supported
surface. Existing configs that reference them warn and skip the widget.

## Output Formats

Plain text is displayed directly:

```text
BTC 103421 ETH 3850
```

JSON output may use Waybar-style fields:

```json
{"text":"VPN","tooltip":"Connected"}
```

- `text`: text shown in the panel.
- `label`: fallback text shown in the panel when `text` is empty.
- `tooltip`: tooltip shown on hover.
- `percentage`: numeric value used as fallback tooltip text when no tooltip is set.

Empty display text hides the widget when no static config `label` fallback is set.

## Clicks

Custom widgets support shell click handlers:

```toml
[widgets.custom-weather]
exec = "~/.config/gnome-topbar/scripts/weather.sh"
interval = 1800
on_click = "xdg-open https://wttr.in"
```
