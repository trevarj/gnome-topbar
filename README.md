# gnome-topbar

A GNOME Shell-style top bar for [niri](https://github.com/YaLTeR/niri), built
with GTK4 and `gtk4-layer-shell`.

![gnome-topbar](assets/screenshots/gnome-topbar.png)

One solid, full-width panel pinned to the top of every monitor: workspaces and
script indicators on the left, clock and weather in the center, tray, alerts,
and the quick-settings aggregate menu on the right. Clicking the clock opens a
GNOME date menu with notifications, calendar, world clocks, media controls, and
the weather forecast.

> **v2 is a ground-up rewrite and is under construction.** The `v2` branch
> currently builds the scaffold — configuration, CLI, and packaging are real;
> the panel itself lands milestone by milestone. Use the `master` branch for the
> shipping v1.

## Configuration

Configuration lives at `~/.config/gnome-topbar/config.toml`. Every key is
optional; anything you leave out falls back to the built-in default.

```sh
gnome-topbar --print-example-config > ~/.config/gnome-topbar/config.toml
gnome-topbar --check-config
```

The lookup order is `$XDG_CONFIG_HOME/gnome-topbar/config.toml`, then
`~/.config/gnome-topbar/config.toml`, then `./config.toml`. Pass `--config PATH`
to use a specific file (it must exist), and `--strict` to turn configuration
warnings into errors.

v1 config files load unchanged. Keys whose feature the rewrite removed are
accepted with a specific explanation of what happened to them.

## Install

The flake exposes a package and an overlay for `x86_64-linux`.

```nix
# flake.nix
{
  inputs.gnome-topbar.url = "github:trevarj/gnome-topbar/v2";

  outputs = { nixpkgs, gnome-topbar, ... }: {
    nixosConfigurations.yourhost = nixpkgs.lib.nixosSystem {
      modules = [
        { nixpkgs.overlays = [ gnome-topbar.overlays.default ]; }
        { environment.systemPackages = [ pkgs.gnome-topbar ]; }
      ];
    };
  };
}
```

Or run it without installing:

```sh
nix run github:trevarj/gnome-topbar/v2
```

## Running under niri

Add to `~/.config/niri/config.kdl`:

```kdl
spawn-at-startup "gnome-topbar"

// Media keys route through the panel so they show an OSD.
binds {
    XF86AudioRaiseVolume  allow-when-locked=true { spawn "gnome-topbar" "volume" "inc" "5"; }
    XF86AudioLowerVolume  allow-when-locked=true { spawn "gnome-topbar" "volume" "dec" "5"; }
    XF86AudioMute         allow-when-locked=true { spawn "gnome-topbar" "volume" "toggle-mute"; }
    XF86MonBrightnessUp                          { spawn "gnome-topbar" "brightness" "inc" "5"; }
    XF86MonBrightnessDown                        { spawn "gnome-topbar" "brightness" "dec" "5"; }
}
```

The panel reserves an exclusive zone, so niri lays windows out beneath it
automatically.

## Development

```sh
direnv allow                                        # or: nix develop
nix develop -c cargo test --workspace --all-targets # inner loop
nix flake check                                     # build + fmt + clippy + tests
nix build                                           # packaged binary in ./result
nix develop -c ./scripts/visual-smoke-niri.sh       # nested niri screenshot
```

See [AGENTS.md](AGENTS.md) for the full working agreement.

## License

MIT. See [LICENSE](LICENSE).
