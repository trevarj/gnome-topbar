# GNOME Panel Plan

## Current State

- Branch: `trev/gnome-panel`.
- The Vibepanel fork has been renamed to GNOME Panel in code, docs, package metadata, and the default config.
- The default config is a GNOME Shell-style continuous top panel with Adwaita icons, bold panel text, centered clock, quick settings, notifications, media, workspaces, and custom script support.
- Guix is the only supported packaging path for now. The in-repo package definition lives in `guix/gnome-panel.scm` and reads Cargo inputs from `Cargo.lock`.
- CI runs Guix-backed formatting, clippy, tests, font subset checks, and a Guix package dry run.

## Direction

- Build a Wayland-only GTK panel inspired by GNOME Shell's top bar.
- Prefer Niri-first behavior while keeping the existing Wayland compositor backend architecture available.
- Keep the default experience quiet, continuous, system-owned, and low-distraction.
- Preserve Waybar-style custom script migration while growing common scripts into tested native Rust modules over time.
- Use idiomatic Rust: typed config boundaries, serde for structured parsing, small service APIs, GTK work on the main thread, and tests with every feature or behavior change.

## Working Rules

- Use Conventional Commits: `feat`, `fix`, `refactor`, `chore`, `docs`, `test`, `build`, `ci`, `perf`, `style`.
- Keep commits atomic. Separate docs, packaging, behavior, refactors, and tests when practical.
- Update this file when task status or project direction changes.
- Keep original license attribution intact.
- Do not add distro-specific install docs or packaging files beyond Guix.

## Verification

Run these before committing meaningful code changes:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

Run this for packaging changes:

```sh
guix build -f guix/gnome-panel.scm
```

Run locally:

```sh
cargo run -p gnome-panel -- --config config.toml -v
```

## Task List

### Design And Visual System

- Audit default CSS against GNOME Shell top-bar behavior.
- Keep the default bar continuous, not floating islands.
- Standardize widget button sizing, hover states, padding, radius, icon alignment, and typography.
- Add automated visual or screenshot smoke coverage before further large design changes.

### Control Panel

- Continue converging clock, calendar, notifications, weather, media, and quick settings into one GNOME-like control panel entry point.
- Keep the notification bell visible only when notifications exist.
- Ensure popovers share consistent blur, radius, padding, and heading/body typography.

### Custom Scripts

- Preserve Waybar-style custom script support for text and JSON output.
- Document supported JSON fields and hide behavior for empty text.
- Add focused tests for custom widgets that hide when scripts return empty or zero-like output.
- Keep common migrated scripts working: crypto, weather, headset, VPN, and distro/icon launcher.

### Icons And Themes

- Keep Adwaita as the default GTK icon theme.
- Improve logical icon mappings for GNOME-like battery, Wi-Fi, volume, resources, notifications, and media icons.
- Maintain Material Symbols fallback support without making it the default.
- Ensure live icon theme reload keeps working.

### Niri And Workspaces

- Keep Niri workspaces visually close to GNOME Shell workspace indicators.
- Preserve mouse-wheel workspace switching.
- Add tests around workspace display, scrolling behavior, and active/occupied state transitions where possible.

### Packaging And CI

- Keep Guix as the only supported packaging path for now.
- Maintain `guix/gnome-panel.scm` using `cargo-inputs-from-lockfile`.
- Keep GitHub Actions focused on fmt, clippy, tests, font subset checks, and Guix package dry runs.
- Avoid reintroducing AUR, COPR, Nix, distro-specific install docs, or release packaging.

### Cleanup And Fork Hardening

- Continue removing stale Vibepanel naming, distro-specific references, and old release machinery when found.
- Review screenshots and assets; replace old floating-island examples with GNOME Panel defaults once visual smoke tooling exists.
- Keep docs concise and current so future agents can pick up work without reconstructing context from commit history.
