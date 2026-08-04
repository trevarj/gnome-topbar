{
  description = "gnome-topbar - GNOME Shell-style top bar for niri (GTK4 + layer-shell)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    crane.url = "github:ipetkov/crane";
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      git-hooks,
    }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      lib = pkgs.lib;
      craneLib = crane.mkLib pkgs;

      # crane's filter keeps Rust and TOML sources only. Two other things are
      # compiled in with include_str! and have to survive it: the example
      # config, which is the binary's --print-example-config output, and the
      # recorded protocol fixtures the parser tests are built on.
      src = lib.cleanSourceWith {
        src = ./.;
        filter =
          path: type:
          (craneLib.filterCargoSources path type)
          || (builtins.baseNameOf path == "config.toml")
          || (lib.hasInfix "/tests/fixtures/" path);
      };

      commonArgs = {
        inherit src;
        pname = "gnome-topbar";
        version = "2.0.0";
        strictDeps = true;
        nativeBuildInputs = with pkgs; [
          pkg-config
          wrapGAppsHook4
          # `dbus-daemon`, for the notification daemon's bus tests. They stand
          # up a private bus per test rather than touching the session's, so
          # the tool has to be on PATH at *test* time, which strictDeps makes
          # a nativeBuildInputs job rather than a buildInputs one.
          dbus
        ];
        buildInputs = with pkgs; [
          gtk4
          gtk4-layer-shell
          glib
          pango
          gdk-pixbuf
          librsvg
          cairo
          graphene
          wayland
          dbus
          udev
          libpulseaudio
          pipewire
          adwaita-icon-theme
          hicolor-icon-theme
          gsettings-desktop-schemas
        ];
      };

      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      gnome-topbar = craneLib.buildPackage (
        commonArgs
        // {
          inherit cargoArtifacts;
          cargoExtraArgs = "-p gnome-topbar";
          meta = {
            description = "GNOME Shell-inspired GTK4 top bar for niri";
            license = lib.licenses.mit;
            mainProgram = "gnome-topbar";
            platforms = [ "x86_64-linux" ];
          };
        }
      );

      pre-commit = git-hooks.lib.${system}.run {
        src = ./.;
        hooks = {
          rustfmt.enable = true;
          nixfmt-rfc-style.enable = true;
          convco.enable = true;
          end-of-file-fixer.enable = true;
          trim-trailing-whitespace.enable = true;
        };
      };
    in
    {
      packages.${system} = {
        default = gnome-topbar;
        inherit gnome-topbar;
      };

      overlays.default = final: prev: {
        gnome-topbar = self.packages.${final.stdenv.hostPlatform.system}.default;
      };

      apps.${system}.default = {
        type = "app";
        program = lib.getExe gnome-topbar;
      };

      checks.${system} = {
        build = gnome-topbar;
        fmt = craneLib.cargoFmt {
          inherit src;
          pname = "gnome-topbar";
          version = "2.0.0";
        };
        clippy = craneLib.cargoClippy (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--workspace --all-targets -- -D warnings";
          }
        );
        test = craneLib.cargoTest (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--workspace --all-targets";
          }
        );
        pre-commit = pre-commit;
      };

      devShells.${system}.default = craneLib.devShell {
        checks = self.checks.${system};
        packages = with pkgs; [
          rust-analyzer
          niri
          grim
          headsetcontrol
          brightnessctl
          # `notify-send`, for driving the notification daemon by hand. Always
          # inside `dbus-run-session`: the panel under test must never be
          # pointed at the session bus the developer is logged into.
          libnotify
        ];
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        shellHook = pre-commit.shellHook;
      };

      formatter.${system} = pkgs.nixfmt-tree;
    };
}
