# topbar

A GNOME Shell-style top bar for [niri](https://github.com/YaLTeR/niri), built
with GTK4 and `gtk4-layer-shell`. GNOME Shell is the design inspiration only —
topbar is not affiliated with or endorsed by the GNOME Project.

![topbar](assets/screenshots/topbar.png)

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

v1 config files load unchanged. Keys whose feature the rewrite removed are
accepted with a specific explanation of what happened to them.

## Install

The flake exposes a package and an overlay for `x86_64-linux`.

```nix
# flake.nix
{
  inputs.topbar.url = "github:trevarj/topbar/v2";

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
nix run github:trevarj/topbar/v2
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
```

The panel reserves an exclusive zone, so niri lays windows out beneath it
automatically.

## Upgrading from gnome-topbar

v2 renamed the project to topbar. Everything the panel owns moved with it:

- **Binary, package, and flake outputs** are `topbar`. `pkgs.gnome-topbar`
  becomes `pkgs.topbar`, and every `spawn "gnome-topbar" …` keybind in
  `config.kdl` becomes `spawn "topbar" …`.
- **Layer-shell namespaces** dropped the prefix: `gnome-topbar` is now
  `topbar`, and the popover, toast, and tooltip surfaces are `topbar-popover`,
  `topbar-toast`, and `topbar-tooltip`. **Any niri `layer-rule` that matches on
  the old namespace stops matching and has to be updated**, for example
  `layer-rule { match namespace="^gnome-topbar$" … }` →
  `match namespace="^topbar$"`.
- **Configuration** should move to `~/.config/topbar/config.toml`.
  `~/.config/gnome-topbar/config.toml` still loads, with one warning per start
  telling you to move it. An explicit `--config PATH` never warns.
- **Runtime state** moves itself: `$XDG_STATE_HOME/gnome-topbar/` is renamed to
  `$XDG_STATE_HOME/topbar/` on the first start, unless the new directory
  already exists.
- **The socket and lock file** are `$XDG_RUNTIME_DIR/topbar.sock` and
  `topbar.lock`.
- **Environment variables** are `TOPBAR_*`, not `GNOME_TOPBAR_*`.

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
