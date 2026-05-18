# Project Goals

GNOME Topbar is a Niri-first, GNOME Shell-inspired top panel for GNU Guix System.

- Keep the default and supported experience close to GNOME Shell's top bar: continuous, quiet, system-owned, and low-distraction.
- Offer a few useful configuration points, not a framework for building a fully different bar.
- Treat mass code reduction and simplification as a primary goal.
- Prefer integrating status and controls into the clock control panel or Quick Settings over adding standalone bar modules.
- Support Niri and NetworkManager as the default system integration path.
- Keep `custom-*` as a narrow escape hatch for one-off indicators.
- Remove features whose main value is theme breadth, Waybar parity, or optional variants that increase maintenance cost.

## Non-Goals

- Do not compete with Waybar, polybar-style setups, docks, taskbars, or general-purpose bar builders.
- Do not preserve compatibility layers when deleting them makes the project simpler and the supported behavior clearer.
- Do not add compositor-specific feature growth outside the Niri target.
- Do not add alternate Wi-Fi service backends unless NetworkManager cannot support a required GNOME-like flow.
