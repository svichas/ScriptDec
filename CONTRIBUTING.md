# Contributing

Thanks for helping. Issues and pull requests are both welcome.

## Reporting a bug

Open an issue with the bug report form; it asks for your OS and ScriptDeck versions. When a script misbehaves, its package manager and command help a lot.

Report security problems privately instead, as described in [SECURITY.md](SECURITY.md).

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

Bump the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, then push a tag such as `v0.2.0`. The Release workflow builds the macOS and Windows apps and attaches them to a draft release, with signed update packages and the `latest.json` that installed apps read.

Check that `latest.json` lists `darwin-aarch64`, `darwin-x86_64` and `windows-x86_64`, and that neither build log warns that the key doesn't match the public key, then publish the draft. Publishing is what ships the update: installed apps find it at `releases/latest/download/latest.json`.

To re-run the macOS job, delete `latest.json` from the draft first. tauri-action keeps the macOS entries it finds there, so they would carry the previous run's signatures and Macs would refuse the update.

The workflow signs with the `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets. Keep a copy of the key: without it, installed apps can't be updated, only reinstalled.
