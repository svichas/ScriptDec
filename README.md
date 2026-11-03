<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="112" alt="">

# ScriptDec

**Run your `package.json` scripts from the menu bar.**

Dev servers, watchers and one-off tasks keep running in the background, each in its own real terminal.

[![CI](https://github.com/svichas/ScriptDec/actions/workflows/ci.yml/badge.svg)](https://github.com/svichas/ScriptDec/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-c2410c.svg)](LICENSE)

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/popover-dark.png">
  <img src="docs/popover-light.png" width="380" alt="The ScriptDec menu bar window: two dev servers running with links to their ports, a failed build, and each project's pinned scripts">
</picture>

</div>

## Features

- **Every script, one click away.** Root scripts, every workspace package's scripts (`pnpm -F @acme/app run start`, and the yarn, npm and bun equivalents), and your own commands.
- **Leave them running.** Each script has its own terminal. Open it from the menu bar, type into it, restart or stop it. Quitting ScriptDec stops every script it started.
- **Search across all projects.** Click the icon or press your shortcut, type, press Enter.
- **Pin** the scripts you use so they stay at the top. The icon shows how many are running.
- **Dev server links.** The port a script prints becomes a one-click link.
- **Node per project.** Follows `.nvmrc`, `.node-version` or a Volta pin (including up the tree), or pick any version installed with nvm, fnm or Volta. Missing versions get an install button.
- **Edit `package.json` scripts.** Add, change, rename or delete them, for the root or any workspace package. Key order, indentation and line endings stay as they were.
- **Crash notifications.** A system notification when a script fails on its own. Stopping one yourself doesn't count, and neither does a failure you're watching in the terminal window.
- **Toolkit.** Install, outdated, update, audit and clean reinstall, for pnpm, yarn, npm and bun.

## Settings

Open Settings from the gear in the menu bar window, or right-click the icon.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/settings-dark.png">
  <img src="docs/settings-light.png" width="860" alt="The Settings window: a project's Node version, toolkit, custom scripts and package.json scripts">
</picture>

- **Projects.** Add a folder with the + button, or drop folders on the window. Each project has its Node version, toolkit, custom commands and `package.json` scripts.
- **General.** Launch at login, a global shortcut that opens ScriptDec from any app, crash notifications on or off, a light, dark or system theme, and the terminal font size.

## Install

Download the `.dmg` from the [latest release](https://github.com/svichas/ScriptDec/releases/latest), open it and drag ScriptDec to Applications.

The app is ad-hoc signed, not notarized, so macOS blocks the first launch. Open it once, then click Open Anyway in System Settings > Privacy & Security.

## Build from source

You need [Rust](https://rustup.rs), [Node.js](https://nodejs.org) with [pnpm](https://pnpm.io), and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```bash
pnpm install
```

```bash
pnpm dev
```

```bash
pnpm test
```

```bash
pnpm build
```

`pnpm build` writes `src-tauri/target/release/bundle/macos/ScriptDec.app` (an NSIS installer on Windows).

```bash
pnpm dmg
```

`pnpm dmg` writes the app and a disk image to `src-tauri/target/release/bundle/` (macOS only). It skips the Finder step that arranges the icons, because that step needs Automation permission and is the usual reason DMG builds fail. The image still has the app and an Applications link.

## How it works

ScriptDec is a [Tauri 2](https://tauri.app) app: Rust for the processes, plain HTML and JS for the windows.

- `src-tauri/src/sessions.rs` owns the PTYs: scrollback, stop escalation (Ctrl-C, then SIGTERM, then SIGKILL on the whole tree) and URL detection.
- `src-tauri/src/project.rs` reads and edits `package.json` and finds workspaces. `node.rs` resolves Node versions across nvm, fnm and Volta.
- `ui/` has no build step. `ui/vendor/` holds xterm.js, copied from `node_modules`. The colours are the tokens at the top of `ui/app.css`.
- `src-tauri/icons/icon.svg` and `tray.svg` are the icon sources. `pnpm tauri icon` turns them into the PNG, ICNS and ICO files beside them.
- Scripts run through your login shell, so they get the same `PATH` as your terminal. The project's Node version is set after your rc files, so it wins over your version manager's default.
- Projects and settings are stored in `config.json` in the app config folder (`~/Library/Application Support/com.scriptdec.desktop/` on macOS).

## Contributing

Bug reports and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) and the [code of conduct](CODE_OF_CONDUCT.md).

## License

[MIT](LICENSE) © 2026 Stefanos Vichas. ScriptDec bundles [xterm.js](https://xtermjs.org) (MIT); its license is in `ui/vendor/`.
