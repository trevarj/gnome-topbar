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

[widgets.custom-crypto]
exec = "~/.config/gnome-topbar/scripts/crypto.sh -r"
interval = 1800
max_chars = 40
```

Use built-in widgets for weather and headset battery instead of carrying
Waybar wrapper scripts:

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

VPN status belongs in Quick Settings. GNOME Topbar detects NetworkManager VPNs
and active external tunnel interfaces such as `tun0`/`wg0`; keep one-off VPN
scripts out of the bar unless you need a deliberately separate indicator.

Standalone widget names such as `battery`, `media`, `cpu`, and `memory` are
intentionally outside the supported surface. Existing configs that reference
them warn and skip the widget. Use Quick Settings resource overview for CPU,
memory, and disk status.

```toml
[widgets.quick_settings]
resource_overview = true
```

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
[widgets.custom-link]
label = "Docs"
interval = 1800
on_click = "xdg-open https://example.invalid"
```
