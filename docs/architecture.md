# GNOME Topbar Architecture

This document provides a high-level overview of GNOME Topbar's architecture for developers who want to understand or extend the codebase.

## Crate Structure

GNOME Topbar is organized as a Cargo workspace with two crates:

```
crates/
  gnome-topbar-core/    # Reusable library: config, theming, error types
  gnome-topbar/         # Main application: GTK widgets, services, UI
```

### gnome-topbar-core

A library crate containing:

- **Config parsing** (`config.rs`) - TOML configuration with validation, merging defaults with user overrides
- **Theming** (`theme.rs`) - `ThemePalette` computes all theme values and generates CSS variables
- **Error types** (`error.rs`) - Typed errors using `thiserror`
- **Logging** (`logging.rs`) - Tracing subscriber setup

This crate has no GTK dependencies, making it easier to test and potentially reuse.

### gnome-topbar

The main application crate containing:

- **Widgets** - Self-contained GTK4 components
- **Services** - Long-lived singleton services for system integration
- **Bar management** - Multi-monitor support, hot-reload

## Service Architecture

Services are process-wide singletons that provide shared state and system integration. They follow a consistent pattern:

```rust
pub struct FooService {
    // Internal state
    callbacks: Callbacks<FooSnapshot>,
}

impl FooService {
    /// Get the global singleton instance
    pub fn global() -> Rc<Self> {
        thread_local! {
            static INSTANCE: Rc<FooService> = FooService::new();
        }
        INSTANCE.with(|s| s.clone())
    }

    /// Register a callback for state changes
    pub fn connect<F>(&self, callback: F)
    where
        F: Fn(&FooSnapshot) + 'static
    {
        self.callbacks.register(callback);
        // Immediately notify with current state
        self.callbacks.notify(&self.current_snapshot());
    }
}
```

Key services:

| Service | Purpose |
|---------|---------|
| `BatteryService` | UPower D-Bus integration for Quick Settings battery state |
| `BrightnessService` | Backlight control via sysfs/logind |
| `AudioService` | PulseAudio volume and device management |
| `NetworkService` | NetworkManager D-Bus for WiFi/VPN |
| `BluetoothService` | BlueZ D-Bus for Bluetooth |
| `NotificationService` | freedesktop notifications via D-Bus |
| `CompositorManager` | Workspace and keyboard-layout state (see below) |
| `ConfigManager` | Hot-reload with file watching |
| `BarManager` | Multi-monitor bar lifecycle |

## Compositor Backend Abstraction

GNOME Topbar targets Niri. The compositor service keeps a small trait boundary
for workspace, focus, keyboard layout, and window-list consumers, but the only
supported implementation is Niri IPC.

```
services/compositor/
  mod.rs          # Public exports
  types.rs        # CompositorBackend trait, WorkspaceSnapshot, WindowInfo
  manager.rs      # CompositorManager singleton
  niri.rs         # Niri IPC implementation
```

The `CompositorBackend` trait defines the interface:

```rust
pub trait CompositorBackend: Send + Sync {
    fn start(&self, on_workspace: WorkspaceCallback, on_window: WindowCallback);
    fn stop(&self);
    fn list_workspaces(&self) -> Vec<WorkspaceMeta>;
    fn get_workspace_snapshot(&self) -> WorkspaceSnapshot;
    fn get_focused_window(&self) -> Option<WindowInfo>;
    fn switch_workspace(&self, workspace_id: i32);
    fn name(&self) -> &'static str;
}
```

`advanced.compositor` accepts `auto` and `niri`; both use the Niri backend.
Other compositor compatibility layers were removed to keep the project aligned
with its Niri-first goals.

## Widget System

Widgets are self-contained GTK4 components. Each widget:

1. Implements a `FooWidget` struct holding GTK widgets and state
2. Has a `FooConfig` struct implementing `WidgetConfig` trait
3. Connects to relevant services for data
4. Returns a `&gtk4::Box` via `.widget()` for embedding

Example widget structure:

```rust
pub struct ClockWidget {
    base: BaseWidget,      // Common widget container
    label: Label,          // GTK label
    timer_source: Rc<RefCell<Option<SourceId>>>,
}

impl ClockWidget {
    pub fn new(config: ClockConfig) -> Self { /* ... */ }
    pub fn widget(&self) -> &gtk4::Box { self.base.widget() }
}
```

`WidgetFactory` constructs widgets from config entries:

```rust
WidgetFactory::build(&entry, qs_handle, output_id) -> Option<BuiltWidget>
```

## Configuration Flow

1. **Load**: `Config::find_and_load()` searches XDG paths for config.toml
2. **Merge**: User config merges with embedded `DEFAULT_CONFIG_TOML`
3. **Validate**: `config.validate()` checks enum values, constraints
4. **Theme**: `ThemePalette::from_config()` computes all derived values
5. **CSS**: `palette.css_vars_block()` generates CSS custom properties
6. **Hot-reload**: `ConfigManager` watches file, re-applies on change

## Multi-Monitor Support

`BarManager` handles multi-monitor setups:

1. On startup, enumerate monitors via `Display::monitors()`
2. Create one bar per monitor (respecting `bar.outputs` filter)
3. Listen for monitor connect/disconnect signals
4. `sync_monitors()` adds/removes bars as needed

Each bar receives its monitor's connector name (e.g., "eDP-1") which is passed to widgets for per-monitor filtering (workspace indicators, window titles).

## Hot-Reload

Two types of hot-reload:

1. **CSS reload**: `ConfigManager` watches for changes, reloads CSS provider
2. **Config reload**: Currently reloads CSS variables; full widget rebuild planned

File watching uses `notify` crate with debouncing to avoid rapid updates during saves.

## Threading Model

- **Main thread**: All GTK operations, widget updates
- **Service threads**: Some services spawn background threads for IPC
- **Callbacks**: Services marshal updates to GLib main loop via `glib::idle_add_local`

Services use `RwLock` for thread-safe state sharing between IPC threads and the main GTK thread.

## Directory Layout

```
crates/gnome-topbar/src/
  main.rs           # Entry point, CLI, GTK app setup
  bar.rs            # Bar window creation, CSS loading
  sectioned_bar.rs  # Layout widget for left/center/right sections
  layout_math.rs    # Layout calculations for panel sections and groups
  styles.rs         # CSS class name constants
  services.rs       # Service module exports
  services/
    compositor/     # Niri compositor service and shared types
    audio.rs        # PulseAudio integration
    battery.rs      # UPower integration for Quick Settings
    ...
  widgets/
    mod.rs          # WidgetFactory, WidgetConfig trait
    base.rs         # BaseWidget helper
    clock.rs        # Clock widget
    workspaces.rs   # Workspace indicators
    quick_settings/ # Quick settings panel components
    ...
```

## Widget Surface

The supported panel surface is intentionally small: `workspaces`, `clock`,
`tray`, `quick_settings`, `keyboard_layout`, and `custom-*`. New standalone
widgets should be avoided unless they preserve the GNOME Shell top-bar shape
and replace more code than they add.

## Compositor Scope

New compositor backends are out of scope unless the project goals change. Keep
the service boundary small and Niri-focused.
