# AGENTS.md

Working agreement for agents and humans on topbar v2.

## System

The host is **NixOS 26.05** running **niri**. Everything goes through the flake —
there is no system-wide Rust toolchain and no `cargo` on `$PATH` outside the dev
shell.

- `direnv allow` once in the repo root, or prefix every command with
  `nix develop -c`.
- **Never run `cargo` outside the dev shell.** A bare `cargo build` either fails
  or picks up the wrong toolchain and links against the wrong GTK.
- The shell is **zsh**.
- Do not set `LD_LIBRARY_PATH` or `CC` anywhere. The flake's `buildInputs` and
  `wrapGAppsHook4` already provide every library, icon theme, and GSettings
  schema the panel needs. Overriding them is how the Guix-era build broke.
- Pre-commit hooks (rustfmt, nixfmt, whitespace, convco) install themselves via
  the dev shell's `shellHook`. Clippy and tests are CI's job, not the hook's.

## Design

v2 is a GNOME Shell top bar, not a status-bar toolkit. When a decision is
ambiguous, ask what GNOME Shell does. GNOME Shell is the design inspiration
only; the project is not affiliated with the GNOME Project.

- **The name is `topbar`, always lowercase.** Prose, headings, sentence
  starts, `--help`, log lines, and the notification server's name all use it
  verbatim — never capitalised, not even to open a sentence. `TOPBAR_*`
  environment variables are the sole exception, and Rust types keep Rust's own
  naming conventions.
- **Quiet.** Widgets that have nothing to say are invisible. `system_monitor`
  shows nothing while the machine is healthy; the mic indicator appears only
  while a source is in use.
- **Continuous.** One solid, full-width, opaque panel pinned to the top edge.
  No islands, no floating pills, no per-widget outlines, no bottom position.
- **System-owned.** The panel presents system state, not decoration. There is
  one dark palette, one accent color, and one generated stylesheet.
- **Mass code reduction is the point.** v1 was 76k lines; v2 targets ~30k. A
  change that adds a subsystem needs to justify itself against that budget.
- **Only the planned surface.** Do not add widgets, config keys, or options that
  are not in the plan. Dropped v1 features stay dropped; their config keys are
  accepted with a specific warning so old files keep loading.
- Motion: 120-150ms for micro-interactions, 150-250ms for containers, never
  over 300ms. `theme.animations = false` means *zero* motion, and
  `gtk-enable-animations` is honored too.
- Icons: Adwaita symbolic names, resolved through one semantic-name module. The
  `os_logo` Nerd Font glyph is the only text-glyph exception.

## Rust

- Stable toolchain, **edition 2024**. No nightly features.
- **Typed boundaries.** Parse protocol data into types once, at the edge. Use
  `niri-ipc`'s types for niri and `#[zbus::proxy]`/`#[zbus::interface]` for
  D-Bus. Hand-walking `serde_json::Value` or `zvariant::Value` in widget code is
  a bug, not a shortcut.
- **`topbar-services` must never depend on `gtk4`.** That dependency edge is the
  thread-safety mechanism: if services cannot name a widget type, they cannot
  touch one. State crosses the boundary as `Send + Clone` handles and
  `Arc<Snapshot>` values.
- **All GTK work happens on the main thread.** Services publish into
  `tokio::sync::watch`; the GTK side subscribes through `bridge::bind_state`,
  which runs the render closure on the main context and aborts when the widget
  drops.
- Blocking I/O never runs on the GTK main thread. No `call_sync`, no blocking
  socket reads, no `std::process::Command` in a click handler.
- Mutating service calls return `Result`; failures are rendered, not discarded.
- **Every behavior change ships with a test.** Protocol parsers get fixtures,
  state machines get table tests, and the config schema gets a case in
  `crates/topbar-core/src/config.rs` or the live-config contract test.

## Verification

- `nix flake check` — the gate. Run it before every push. It covers build, fmt,
  clippy with `-D warnings`, tests, and the pre-commit hooks.
- `nix build` — run for any change to packaging, dependencies, or the GTK link
  path.
- `nix develop -c cargo test --workspace --all-targets` — the inner loop.
- `nix develop -c ./scripts/visual-smoke-niri.sh` — nested niri + `grim`
  screenshot. Local only: niri has no headless backend, so CI cannot run it.
- UI milestones also need a run on the live niri session against
  `~/.config/topbar/config.toml`.

## Commits

- **Conventional Commits**, enforced by the `convco` pre-commit hook.
- Atomic: each commit builds, passes tests, and does one thing. Prefer a series
  of small commits over one large one.
- Subject in the imperative mood, no trailing period. Explain *why* in the body
  when the reason is not obvious from the diff.
- Breaking changes use `!` and a `BREAKING CHANGE:` footer.
