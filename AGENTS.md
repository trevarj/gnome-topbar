# GNOME Panel Agent Rules

## System

- This project targets GNU Guix System development.
- Do not use `guix install`; use `guix shell -m manifest.scm`.
- The default shell is zsh.

## Design

- Follow GNOME Shell top-bar discipline: quiet, continuous, system-owned, and low-distraction.
- The default bar is a solid full-width top panel, not floating islands.
- Widgets and custom scripts should look like GNOME panel buttons: transparent at rest, rounded hover fill, bold text, Adwaita symbolic icons.
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

Run these before committing meaningful code changes:

```sh
guix shell -m manifest.scm -- cargo fmt --check
guix shell -m manifest.scm -- cargo clippy --workspace --all-targets -- -D warnings
guix shell -m manifest.scm -- cargo test --workspace --all-targets
```

For runtime checks:

```sh
guix shell -m manifest.scm -- sh -c 'export LD_LIBRARY_PATH=$LIBRARY_PATH; cargo run -p gnome-panel -- --config config.toml -v'
```
