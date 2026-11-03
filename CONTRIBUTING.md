# Contributing

Thanks for helping. Issues and pull requests are both welcome.

## Reporting a bug

Include your OS version, the ScriptDec version (Settings > General > About), what you did, what you expected and what happened instead. When a script misbehaves, its package manager and command help a lot.

## Development

Set up as in [Build from source](README.md#build-from-source), then start the app with your changes:

```bash
pnpm dev
```

The UI in `ui/` has no build step: reload a window from its right-click menu to pick up changes. Rust changes rebuild and restart the app.

Before you open a pull request, run the tests and clippy. CI runs both on macOS.

```bash
pnpm test
```

```bash
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

## Guidelines

- One fix or feature per pull request, as small as it can be.
- No frontend framework or bundler. Build DOM with `h()` from `ui/shared.js`, never `innerHTML` with data: names come from other people's `package.json` files.
- Anything that reaches a shell and comes from a repo must be quoted with `sh_quote` (`src-tauri/src/project.rs`).
- Colours come from the tokens at the top of `ui/app.css`. Check light and dark.
- New logic in `src-tauri/src` gets a test beside it.

## Releasing

Bump the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, then push a tag such as `v0.2.0`. The Release workflow builds the macOS and Windows apps and attaches them to a draft release.
