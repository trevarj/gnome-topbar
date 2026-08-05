# topbar

A GNOME Shell-style top bar for [niri](https://github.com/YaLTeR/niri), built
with GTK4 and `gtk4-layer-shell`. GNOME Shell is the design inspiration only —
topbar is not affiliated with or endorsed by the GNOME Project.

![topbar](assets/screenshots/topbar.png)

One solid, full-width panel pinned to the top of every monitor: workspaces and
indicators on the left, clock and weather in the center, tray, alerts and the
quick-settings aggregate menu on the right. Clicking the clock opens a GNOME
date menu with notifications, calendar, world clocks, media controls and the
weather forecast.

![the control panel](assets/screenshots/topbar-panel.png)

## What is on the bar

- **workspaces** — GNOME Activities-style dots with an animated active pill,
  filtered to the monitor they are on. Urgent workspaces pulse.
- **clock** — any `strftime` format, aligned to the boundary, with the date
  menu behind it: notification history with grouping and Do Not Disturb, a
  calendar, world clocks, MPRIS media controls and a five-day forecast.
- **weather** — Open-Meteo current conditions and forecast, with a location
  search dialog. One cache for the whole panel.
- **crypto** — bitcoin, ethereum and monero prices from CoinGecko, singly or as
  pair ratios, with a popover that edits the list without touching your config.
- **tray** — StatusNotifierItem host with dbusmenu support and an overflow
  chevron.
- **system_monitor** — invisible while the machine is healthy; fades in with
  warning-tinted icons once CPU, memory or disk crosses a threshold.
- **headset** — battery for a 2.4GHz wireless headset, via `headsetcontrol`.
- **keyboard_layout**, **os_logo**, and **custom-\*** widgets that run a script
  and speak Waybar's JSON contract.
- **quick_settings** — the GNOME 45 aggregate menu: Wi-Fi (with a secret agent
  for passwords), Bluetooth (with a pairing agent), VPN, volume and microphone,
  brightness, Power Mode, Caffeine, battery health with charge thresholds,
  pending updates, a resource overview, privacy dots, and a power section that
  asks you to hold the row down.

Plus the surfaces that are not widgets: the notification daemon and its
banners, the volume and brightness OSD capsule, and compositor blur behind
everything the panel draws.

**Configuration is applied live.** Save `config.toml` and the panel changes:
colours are a stylesheet swap, a widget's own section rebuilds that widget, and
only the bar's geometry rebuilds the bars. A file that does not parse changes
nothing and says so in one banner.

## Install

The flake exposes a package and an overlay for `x86_64-linux`.

```nix
# flake.nix
{
  inputs.topbar.url = "github:trevarj/topbar";

  outputs = { nixpkgs, topbar, ... }: {
    nixosConfigurations.yourhost = nixpkgs.lib.nixosSystem {
      modules = [
        { nixpkgs.overlays = [ topbar.overlays.default ]; }
        { environment.systemPackages = [ pkgs.topbar ]; }
      ];
    };
  };
}
```

Or run it without installing:

```sh
nix run github:trevarj/topbar
```

## Running under niri

Add to `~/.config/niri/config.kdl`:

```kdl
spawn-at-startup "topbar"

// Media keys route through the panel so they show an OSD.
binds {
    XF86AudioRaiseVolume  allow-when-locked=true { spawn "topbar" "volume" "inc" "5"; }
    XF86AudioLowerVolume  allow-when-locked=true { spawn "topbar" "volume" "dec" "5"; }
    XF86AudioMute         allow-when-locked=true { spawn "topbar" "volume" "toggle-mute"; }
    XF86MonBrightnessUp                          { spawn "topbar" "brightness" "inc" "5"; }
    XF86MonBrightnessDown                        { spawn "topbar" "brightness" "dec" "5"; }
}

// Blur behind the panel and its popovers, if you want it. `theme.blur = true`
// in topbar's own configuration asks for the region; this is what the
// compositor does with it.
layer-rule {
    match namespace="^topbar$"
    match namespace="^topbar-popover$"

    background-effect {
        xray false
    }
}
```

The panel reserves an exclusive zone, so niri lays windows out beneath it
automatically.

The volume and brightness commands act on PulseAudio and logind **directly**
and only then try to raise an OSD, so a media key still works when the panel is
not running and when the configuration is broken.

## Configuration

Configuration lives at `~/.config/topbar/config.toml`. Every key is optional;
anything you leave out falls back to the built-in default.

```sh
topbar --print-example-config > ~/.config/topbar/config.toml
topbar --check-config
```

The lookup order is `$XDG_CONFIG_HOME/topbar/config.toml`, then
`~/.config/topbar/config.toml`, then the same two under the project's former
`gnome-topbar` name, then `./config.toml`. Pass `--config PATH` to use a
specific file (it must exist), and `--strict` to turn configuration warnings
into errors.

Every key, its default and what it does:
[docs/configuration.md](docs/configuration.md). How the thing is built:
[docs/architecture.md](docs/architecture.md).

## Switching from gnome-topbar (v1)

v2 is a ground-up rewrite and a rename. Your configuration file keeps working
byte-for-byte, but a handful of things around it have to move.

1. **Stop v1 first.** Both versions take `org.freedesktop.Notifications` with
   `ReplaceExisting`, so whichever starts last owns the notifications and the
   other one silently stops receiving them. Remove the old
   `spawn-at-startup "gnome-topbar"` line from `config.kdl` and kill the
   running panel before starting topbar.
2. **Update every niri `layer-rule`.** The layer-shell namespaces lost the
   prefix: `gnome-topbar` is now `topbar`, and the popover, toast and tooltip
   surfaces are `topbar-popover`, `topbar-toast` and `topbar-tooltip`. A rule
   matching `^gnome-topbar$` stops matching anything — this is the one that
   makes people think blur broke.
3. **Update your key binds and your package.** Every
   `spawn "gnome-topbar" …` becomes `spawn "topbar" …`, and
   `pkgs.gnome-topbar` becomes `pkgs.topbar`.
4. **Move your config, when you feel like it.**
   `~/.config/gnome-topbar/config.toml` still loads, with one line on start
   telling you to move it to `~/.config/topbar/config.toml`. An explicit
   `--config PATH` never warns.
5. **Check `[updates] update_count_command`.** v2 detects the distribution and
   deduces a read-only counting command for Guix, Debian, Arch, Fedora,
   Fedora's image-based editions and NixOS. A command left over from another
   machine still wins outright and hides the card when it fails, so delete it
   unless you meant it. On NixOS, point `[updates] flake` at your system flake
   if it is not at `/etc/nixos`; the count comes from re-locking a scratch copy
   and your real `flake.lock` is never written.
6. **Consider swapping `custom-crypto` for the built-in `crypto` widget.** It
   draws the same numbers without a shell script, a `curl` or a `jq`, and its
   popover edits the list of coins without touching your configuration file.
   The `custom-*` engine is not going anywhere if you would rather keep the
   script.
7. **Features v1 had that v2 does not.** Material You and the light and GTK
   theme modes, wallpaper colour extraction, the Material Symbols icon font,
   the outline system, widget groups, the `outputs` allowlist, the bottom bar
   position, cellular, MPD and the cava visualiser are gone. Their keys still
   load and each one tells you what happened to it; `bar.position = "bottom"`
   is the one that is an error rather than a warning, because there is nothing
   sensible to do instead.
8. **Runtime state moves itself.** `$XDG_STATE_HOME/gnome-topbar/` is renamed
   to `$XDG_STATE_HOME/topbar/` on the first start, unless the new directory
   already exists. The socket and lock file are `$XDG_RUNTIME_DIR/topbar.sock`
   and `topbar.lock`, and the environment variables are `TOPBAR_*`.

## Development

```sh
direnv allow                                        # or: nix develop
nix develop -c cargo test --workspace --all-targets # inner loop
nix flake check                                     # build + fmt + clippy + tests
nix build                                           # packaged binary in ./result
nix develop -c ./scripts/smoke-full-bar.sh          # nested niri, one screenshot
```

The `scripts/smoke-*.sh` runs each drive one part of the panel inside a nested
niri session with stand-ins on a private bus, and archive their screenshots
under `target/visual-smoke/`. They are local-only: niri has no headless
backend, so CI cannot run them.

See [AGENTS.md](AGENTS.md) for the full working agreement.

## License

MIT. See [LICENSE](LICENSE).
