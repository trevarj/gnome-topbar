# GNOME Topbar Agent Rules

## System

- This project targets GNU Guix System development.
- Do not use `guix install`; use `guix shell -m manifest.scm`.
- The default shell is zsh.

## Design

- Follow GNOME Shell top-bar discipline: quiet, continuous, system-owned, and low-distraction.
- The default bar is a solid full-width top panel, not floating islands.
- Widgets and custom scripts should look like GNOME panel buttons: transparent at rest, rounded hover fill, bold text, Adwaita symbolic icons.
- Treat mass code reduction and simplification as a primary project goal; prefer deleting optional variants over preserving broad bar-framework flexibility.
- See `docs/project-goals.md` for the product boundary and non-goals.
- Design changes need automated screenshot coverage through the project smoke harness once available.

## Rust

- Use idiomatic Rust with typed boundaries.
- Prefer serde structs/enums for config and JSON parsing instead of hand-parsed strings.
- Keep GTK work on the main thread and service state behind small, explicit APIs.
- Add tests with every new feature or behavior change.

## Commits

- Use Conventional Commits: `feat`, `fix`, `refactor`, `chore`, `docs`, `test`, `build`, `ci`, `perf`, `style`.
- Keep commits atomic. Separate renames, behavior changes, docs, tests, and packaging when practical.

## Verification

With direnv loaded, run these before committing meaningful code changes:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

Run `guix build -f guix/gnome-topbar.scm` for packaging changes.

For runtime checks:

```sh
cargo run -p gnome-topbar -- --config config.toml -v
```
