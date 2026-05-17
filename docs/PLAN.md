# GNOME Topbar Plan

## Current State

- Branch: `trev/gnome-topbar`.
- The project has been renamed to GNOME Topbar in code, docs, package metadata, and the default config.
- The default config is a GNOME Shell-style continuous top panel with Adwaita icons, bold panel text, left-side workspaces, centered clock, a required tray, and one right-side quick settings aggregate.
- Guix is the only supported packaging path for now. The in-repo package definition lives in `guix/gnome-topbar.scm` and reads Cargo inputs from `Cargo.lock`.
- CI runs Guix-backed formatting, clippy, tests, font subset checks, and a Guix package dry run.

## Direction

- Build a Wayland-only GTK panel inspired by GNOME Shell's top bar.
- Treat GNOME Shell's own top bar as a viable styling and design resource for spacing, typography, hover states, indicator density, popover rhythm, and low-distraction behavior.
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
guix build -f guix/gnome-topbar.scm
```

Run locally:

```sh
cargo run -p gnome-topbar -- --config config.toml -v
```

## Implementation Subtasks

### Phase 1: Visual Smoke Baseline

- [x] Make `scripts/visual-smoke-niri.sh` the required screenshot gate for design changes.
- [x] Document the visual smoke artifact path and expected Guix invocation.
- [x] Confirm the smoke harness works through `guix shell -m manifest.scm`.

### Phase 2: GNOME Shell Top-Bar Visual System

- [x] Audit the default panel against GNOME Shell's top bar for bar continuity, widget density, hover fill, icon sizing, clock placement, workspace indicators, and popover spacing.
- [x] Keep the default bar continuous and system-owned, not a set of floating islands.
- [x] Standardize widget button sizing, hover states, padding, radius, icon alignment, and typography.
- [x] Keep visual changes covered by the smoke harness before making further large design changes.

Visual audit notes:

- Current smoke output shows a solid full-width black top panel, centered clock, compact right-side system indicators, and left-side workspace indicators.
- The default bar already follows GNOME Shell's continuous-panel direction; future visual work should focus on hover/button consistency, indicator sizing, and popover rhythm.
- Workspace indicators use compact dot/pill sizing and should be tuned only with screenshot comparison.

### Phase 3: Custom Scripts

- [x] Preserve Waybar-style custom script support for plain text and JSON output.
- [x] Add focused tests for empty output, JSON empty `text`, `label` fallback, `percentage` tooltip fallback, and zero-like script output where intended.
- [x] Keep `README.md` and `docs/waybar-migration.md` aligned with supported JSON fields and empty-text hide behavior.
- [x] Keep common migrated scripts working: crypto, weather, headset, VPN, and distro/icon launcher.

### Phase 4: Icons And Themes

- [x] Keep Adwaita as the default GTK icon theme.
- [x] Improve GTK/Adwaita icon candidates and Material Symbols fallbacks for battery, Wi-Fi, volume, resources, notifications, and media.
- [x] Update icon mapping tests with each mapping change.
- [x] Add or strengthen regression coverage for live icon theme reload through `IconsService::reconfigure()`.

### Phase 5: Niri And Workspaces

- [x] Keep Niri workspaces visually close to GNOME Shell workspace indicators.
- [x] Preserve mouse-wheel workspace switching.
- [x] Extend pure tests for workspace display filtering, scroll target selection, active/occupied transitions, and Niri per-output behavior.
- [x] Tune workspace indicator CSS only after the smoke baseline is available.

### Phase 6: Control Panel And Popovers

- [x] Inventory clock, calendar, notifications, weather, media, and quick settings entry points.
- [x] Continue converging those widgets into one GNOME-like control panel entry point one behavior slice at a time.
- [x] Keep the notification bell visible only when notifications exist.
- [x] Standardize blur, background, radius, padding, heading typography, and body typography across popovers, using GNOME Shell top-bar popovers as the primary visual reference where applicable.

Current control panel inventory:

- Clock: opens either calendar-only content or `build_clock_control_panel` when `[widgets.clock].control_panel = true`.
- Calendar: embedded in the clock control panel through `build_clock_calendar_popover`.
- Notifications: live in the clock control panel by default. The standalone bell remains opt-in and opens either notification-only content or the clock control panel when `[widgets.notifications].control_panel = true`; `hide_empty` controls bell visibility.
- Weather: pulled into the clock control panel from a configured custom widget exec via `control_panel_weather_widget`.
- Media: embedded in the clock control panel through `build_media_popover_with_controller`; standalone media widget still has its own popover and pop-out window.
- Quick settings: currently opens its own keep-alive window from the bar widget; convergence should happen one behavior slice at a time.

Completed control-panel slice:

- Clock control-panel options and weather-widget integration are covered by config tests.
- Notification `hide_empty` and control-panel routing are covered by config tests; the default layout keeps notification access in the clock control panel rather than a standalone bar icon.
- Popover surface CSS is covered by shared token tests for background, radius, padding, and typography.

### Phase 7: Packaging, CI, And Fork Hardening

- [x] Keep Guix as the only supported packaging path for now.
- [x] Maintain `guix/gnome-topbar.scm` using `cargo-inputs-from-lockfile`.
- [x] Keep GitHub Actions focused on fmt, clippy, tests, font subset checks, and Guix package dry runs.
- [x] Avoid reintroducing AUR, COPR, Nix, distro-specific install docs, or release packaging.
- [x] Remove stale legacy naming, distro-specific references, old release machinery, and outdated screenshots when encountered during related work.
- [x] Keep docs concise and current so future agents can pick up work without reconstructing context from commit history.

Packaging and cleanup audit:

- `guix/gnome-topbar.scm` uses `cargo-inputs-from-lockfile`.
- GitHub Actions run Guix-backed fmt, clippy, tests, font subset checks, and `guix build -f guix/gnome-topbar.scm --dry-run`.
- No AUR, COPR, Nix, distro-specific packaging files, or release machinery are present in the tracked project files.
- No stale legacy project names, distro-specific packaging files, or release machinery are present in the tracked project files.
