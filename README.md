# ScriptDec

A menu bar app for running your `package.json` scripts in the background. Built with Rust (Tauri 2) and real PTYs.

- **Run** root scripts, every workspace package's scripts (`pnpm -F @acme/app run start`), or your own commands.
- **Leave them running.** Each script has its own terminal. Open it from the menu bar, type into it, restart or stop it.
- **Search** across all projects: click the icon, type, press Enter.
- **Pin** the scripts you use so they stay at the top. The icon shows how many are running.
- **Node per project:** follows `.nvmrc` / `.node-version` (including up the tree), or pick any installed nvm version. Missing versions get an install button.
- **Toolkit:** install, outdated, update, audit, clean reinstall, for pnpm, yarn, npm and bun.
- **Dev server links:** the port a script prints becomes a one-click link.

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

## How it works

- `src-tauri/src/sessions.rs` owns the PTYs: scrollback, stop escalation (Ctrl-C, then SIGTERM, then SIGKILL on the whole tree) and URL detection.
- `src-tauri/src/project.rs` reads `package.json` and workspaces. `node.rs` resolves nvm versions.
- `ui/` is plain HTML and JS with no build step. `ui/vendor/` holds xterm.js, copied from `node_modules`.
- Scripts run through your login shell, so they get the same `PATH` as your terminal. The project's Node version is set after your rc files, so it wins over an nvm default.
- Projects are stored in `config.json` in the app config folder (`~/Library/Application Support/com.scriptdec.desktop/` on macOS).

Quitting ScriptDec stops every script it started.
