# ScriptDec

A menu bar app for running your `package.json` scripts in the background. Built with Rust (Tauri 2) and real PTYs.

- **Run** root scripts, every workspace package's scripts (`pnpm -F @acme/app run start`), or your own commands.
- **Leave them running.** Each script has its own terminal. Open it from the menu bar, type into it, restart or stop it.
- **Search** across all projects: click the icon, type, press Enter.
- **Pin** the scripts you use so they stay at the top. The icon shows how many are running.
- **Node per project:** follows `.nvmrc`, `.node-version` or a Volta pin (including up the tree), or pick any version installed with nvm, fnm or Volta. Missing versions get an install button.
- **Edit `package.json` scripts:** add, change, rename or delete them, for the root or any workspace package. Key order, indentation and line endings stay as they were.
- **Crash notifications:** a system notification when a script fails on its own. Stopping one yourself doesn't count, and neither does a failure you're watching in the terminal window.
- **Toolkit:** install, outdated, update, audit, clean reinstall, for pnpm, yarn, npm and bun.
- **Dev server links:** the port a script prints becomes a one-click link.
- **Launch at login:** a checkbox in Settings.

## Develop

Needs Rust and pnpm.

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

The app is ad-hoc signed, not notarized. On another Mac, the first launch needs System Settings > Privacy & Security > Open Anyway.

## How it works

- `src-tauri/src/sessions.rs` owns the PTYs: scrollback, stop escalation (Ctrl-C, then SIGTERM, then SIGKILL on the whole tree) and URL detection.
- `src-tauri/src/project.rs` reads and edits `package.json` and finds workspaces. `node.rs` resolves Node versions across nvm, fnm and Volta.
- `ui/` is plain HTML and JS with no build step. `ui/vendor/` holds xterm.js, copied from `node_modules`. The colours are the tokens at the top of `ui/app.css`.
- `src-tauri/icons/icon.svg` and `tray.svg` are the icon sources. `tauri icon` turns them into the PNG, ICNS and ICO files beside them.
- Scripts run through your login shell, so they get the same `PATH` as your terminal. The project's Node version is set after your rc files, so it wins over your version manager's default.
- Projects are stored in `config.json` in the app config folder (`~/Library/Application Support/com.scriptdec.desktop/` on macOS).

Quitting ScriptDec stops every script it started.
