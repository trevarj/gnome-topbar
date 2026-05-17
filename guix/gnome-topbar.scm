(define-module (gnome-topbar)
  #:use-module (guix build-system cargo)
  #:use-module (guix gexp)
  #:use-module ((guix licenses) #:prefix license:)
  #:use-module (guix packages)
  #:use-module (gnu packages)
  #:use-module (gnu packages pkg-config)
  #:use-module (ice-9 match))

;; Seed/update dependency inputs with:
;;   guix import crate --lockfile=Cargo.lock gnome-topbar
;;
;; This local definition intentionally stays in-repo while the package is not
;; upstreamed to Guix.  The generated crate dependency list is large; keep it
;; close to Cargo.lock and refresh it before attempting an upstream submission.
(define-public gnome-topbar
  (package
    (name "gnome-topbar")
    (version "0.14.1")
    (source
     (local-file ".." "gnome-topbar-checkout"
                 #:recursive? #t
                 #:select?
                 (lambda (file stat)
                   (and (not (string-contains file "/.git"))
                        (not (string-contains file "/target"))))))
    (build-system cargo-build-system)
    (arguments
     (list
      #:install-source? #f
      #:cargo-inputs
      ;; TODO: populate from `guix import crate --lockfile=Cargo.lock gnome-topbar`.
      '()))
    (native-inputs
     (list pkg-config))
    (inputs
     (map specification->package
          '("gtk"
            "gtk4-layer-shell"
            "glib"
            "dbus"
            "eudev"
            "pango"
            "gdk-pixbuf"
            "cairo"
            "graphene"
            "pulseaudio"
            "upower"
            "power-profiles-daemon"
            "network-manager"
            "bluez")))
    (home-page "https://github.com/trevarj/gnome-topbar")
    (synopsis "GNOME Shell-inspired GTK top bar for Wayland")
    (description
     "GNOME Topbar is a Wayland-only GTK top bar inspired by GNOME Shell.  It
provides a continuous system panel with notifications, quick settings, media
controls, workspaces, and custom script modules.")
    (license license:expat)))

gnome-topbar
