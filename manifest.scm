(use-modules (guix packages)
             (gnu)
             (gnu packages)
             (gnu packages base)
             (gnu packages commencement)
             (gnu packages llvm)
             (gnu packages pkg-config)
             (rustup build toolchain))

(packages->manifest
 (append
  (list
   gcc-toolchain
   gnu-make
   clang-toolchain-21
   binutils
   pkg-config
   (rustup "nightly-2026-03-14"
           #:components
           '("rust-analyzer" "rustfmt" "rust-src" "rust-std" "clippy")))
  (map specification->package
       '("gtk"
         "adwaita-icon-theme"
         "papirus-icon-theme"
         "gtk4-layer-shell"
         "glib"
         "dbus"
         "eudev"
         "wayland"
         "pango"
         "gdk-pixbuf"
         "cairo"
         "graphene"
         "pulseaudio"
         "upower"
         "power-profiles-daemon"
         "network-manager"
         "bluez"
         "brightnessctl"
         "headsetcontrol"
         "curl"
         "jq"
         "nss-certs"
         "bash"
         "coreutils"))))
