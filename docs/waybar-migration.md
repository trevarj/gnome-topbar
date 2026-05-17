# Waybar Custom Script Migration

GNOME Topbar supports Waybar-style custom script output for common status
modules. Weather and headset have native widget names; other one-off scripts
can keep using the `custom-` prefix. The default bar keeps common system status
inside Quick Settings.

```toml
[widgets]
left = ["workspaces", "custom-crypto"]
center = ["clock"]
right = ["headset", "custom-vpn", "tray", "quick_settings"]

[widgets.custom-crypto]
exec = "~/.config/gnome-topbar/scripts/crypto.sh -r"
interval = 1800
max_chars = 40

[widgets.weather]
exec = "~/.config/gnome-topbar/scripts/weather.sh"
interval = 1800
position = "left"
tooltip = "Weather"

[widgets.headset]
exec = "~/.config/gnome-topbar/scripts/headsetcontrol.sh"
interval = 5
tooltip = "Headset battery"

[widgets.custom-vpn]
exec = "sh -c 'test -r /sys/class/net/tun0/carrier && grep -qx 1 /sys/class/net/tun0/carrier && echo vpn'"
interval = 5
```

Battery, network, audio, Bluetooth, and VPN status are shown by
`quick_settings` by default. Add standalone widgets such as `battery` only when
you intentionally want a Waybar-like module split.

## Output Formats

Plain text is displayed directly:

```text
BTC 103421 ETH 3850
```

JSON output may use Waybar-style fields:

```json
{"text":"󰋎 ","tooltip":"Headset: 72%","percentage":72}
```

- `text`: text shown in the panel.
- `label`: fallback text shown in the panel when `text` is empty.
- `tooltip`: tooltip shown on hover.
- `percentage`: numeric value used as fallback tooltip text when no tooltip is set.

Empty display text hides the widget when no static config `label` fallback is set.

## Clicks

Custom widgets support shell click handlers:

```toml
[widgets.weather]
on_click = "xdg-open https://wttr.in"
on_click_right = "sh -c 'echo F > /tmp/gnome-topbar-weather-unit'"
```
