<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="112" alt="">

# ScriptDeck

**Run your `package.json` scripts from the menu bar.**

Dev servers, watchers and one-off tasks keep running in the background, each in its own real terminal.

[![CI](https://github.com/svichas/ScriptDeck/actions/workflows/ci.yml/badge.svg)](https://github.com/svichas/ScriptDeck/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-c2410c.svg)](LICENSE)

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/popover-dark.png">
  <img src="docs/popover-light.png" width="380" alt="The ScriptDeck menu bar window: the same dev server running in two worktrees, an API server and a failed build, then a project's branch, group and pinned scripts">
</picture>

</div>

## Features

- **Every script, one click away.** Root scripts, every workspace package's scripts (`pnpm -F @acme/app run start`, and the yarn, npm and bun equivalents), and your own commands.
- **Leave them running.** Each script has its own terminal. Open it from the menu bar, type into it, restart or stop it. Quitting ScriptDeck stops every script it started.
- **Reopen after a restart.** Turn on Reopen running scripts, and the scripts still running when you quit, log out or update start again with ScriptDeck. Ones you stopped stay stopped.
- **Find in the output.** Cmd+F (Ctrl+F on Windows) searches a terminal's whole scrollback. Cmd+click (Ctrl+click) opens any web link a script prints.
- **CPU and memory.** Hover a script's terminal tab to see what all its processes use together, like `1.2 GB · 42%`. CPU is a share of the whole Mac, so 100% means every core is busy. Past 500 MB or 10% CPU, its row in the menu bar window shows it too. macOS only.
- **AI agents (MCP).** Turn it on in Settings, and Claude Code or another agent can see your scripts, read and search their output, and start, restart or stop them. Agents never type into a terminal. Not on Windows yet.
- **Branches and worktrees.** Each project in a git repository shows the branch it's on, a dot for uncommitted changes and what's left to pull. Click it to check out another branch, make one, pull, or switch the project to another worktree of the repository. Its scripts then run in that folder, and what's already running keeps running, so the same `dev` can run on two branches at once, each in its own terminal. A new worktree for a new or existing branch is one name away, in a folder beside the repository. Settings lists the worktrees with what's running in each, and clones repositories.
- **Search across all projects.** Click the icon or press your shortcut, type, press Enter.
- **Run with arguments.** Option-click a script's Run button (Alt-click on Windows), or press Shift+Enter on a search result, then type `--watch` or `--port 3001`. ScriptDeck adds the `--` npm needs, runs it in its own terminal and remembers the arguments for next time. Save as custom script keeps the whole command.
- **Pin** the scripts you use so they stay at the top. The icon shows how many are running.
- **Groups.** Start the scripts you always run together, say web, api and a worker, with one click, and stop them the same way. Each still gets its own terminal.
- **Dev server links.** The port a script prints becomes a one-click link.
- **Port already in use.** When a script can't start because its port is taken, its row names what holds it, another script or a leftover process, and Free port stops that and restarts yours. macOS only.
- **Node per project.** Follows `.nvmrc`, `.node-version` or a Volta pin (including up the tree), or pick any version installed with nvm, fnm or Volta. Missing versions get an install button.
- **Edit `package.json` scripts.** Add, change, rename or delete them, for the root or any workspace package. Key order, indentation and line endings stay as they were.
- **Crash notifications.** A system notification when a script fails on its own. Stopping one yourself doesn't count, and neither does a failure you're watching in the terminal window.
- **Test results.** A notification when tests pass or fail, however quick: any script with test, e2e, vitest or jest in its name or command. Watch modes never end, so they stay quiet.
- **Restart on crash.** Switch it on per script in Settings, and a script that dies on its own comes back after 1 s, then 2, 4, 8 and 16 s. If all five restarts crash within a minute of starting, it stays down. You get one notification as it starts restarting and one if it gives up. Stopping it yourself never restarts it.
- **Durations.** Each script shows how long it has been running, or how long it took. Installs, builds and anything else that take 30 seconds or more post a notification when they finish, unless their terminal is in front.
- **Toolkit.** Install, outdated, update, audit and clean reinstall, for pnpm, yarn, npm and bun.

## Settings

Open Settings from the gear in the menu bar window, or right-click the icon.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/settings-dark.png">
  <img src="docs/settings-light.png" width="860" alt="The Settings window: a project's branch, custom scripts, group and package.json scripts">
</picture>

- **Projects.** Add a folder with the + button, clone a repository with the button beside it, or drop folders on the window. Each project has its Node version, toolkit, git branch and worktrees, custom commands, groups and `package.json` scripts.
- **Preferences.** Launch at login, reopening running scripts after a restart, a global shortcut that opens ScriptDeck from any app, crash, test and long-task notifications on or off (with an Allow button while macOS blocks them), a light, dark or system theme, the terminal font size, AI agent access (MCP), and Check for Updates.

## Install

Download the `.dmg` from the [latest release](https://github.com/svichas/ScriptDeck/releases/latest), open it and drag ScriptDeck to Applications.

The app is ad-hoc signed, not notarized, so macOS blocks the first launch. Open it once, then click Open Anyway in System Settings > Privacy & Security.

From 0.2.0 on, ScriptDeck updates itself. When a new release is out, a banner at the bottom of the menu bar window offers it: click Update, then Restart Now once it has downloaded. It waits for you because restarting stops your running scripts.

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

`pnpm build` writes `src-tauri/target/release/bundle/macos/ScriptDeck.app` (an NSIS installer on Windows).

```bash
pnpm dmg
```

`pnpm dmg` writes the app and a disk image to `src-tauri/target/release/bundle/` (macOS only). It skips the Finder step that arranges the icons, because that step needs Automation permission and is the usual reason DMG builds fail. The image still has the app and an Applications link.

## How it works

ScriptDeck is a [Tauri 2](https://tauri.app) app: Rust for the processes, plain HTML and JS for the windows.

- `src-tauri/src/sessions.rs` owns the PTYs: scrollback, stop escalation (Ctrl-C, then SIGTERM, then SIGKILL on the whole tree) and URL detection.
- `src-tauri/src/project.rs` reads and edits `package.json` and finds workspaces. `node.rs` resolves Node versions across nvm, fnm and Volta.
- `ui/` has no build step. `ui/vendor/` holds xterm.js, copied from `node_modules`. The colours are the tokens at the top of `ui/app.css`.
- `src-tauri/icons/icon.svg` and `tray.svg` are the icon sources. `pnpm tauri icon` turns them into the PNG, ICNS and ICO files beside them.
- Scripts run through your login shell, so they get the same `PATH` as your terminal. The project's Node version is set after your rc files, so it wins over your version manager's default.
- `src-tauri/src/update.rs` updates the app from the latest GitHub release's `latest.json`, through the [Tauri updater](https://tauri.app/plugin/updater/). It only installs packages signed with the key whose public half is in `tauri.conf.json`.
- `src-tauri/src/git/` runs the `git` command line: `mod.rs` for branches, worktrees, pull and clone, `parse.rs` for reading what git prints, and `commands.rs` for what the windows ask. A project's `worktree` is the folder its scripts run in, and a script runs once per folder. Git gets your login shell's `PATH` and the project's Node, so hooks such as husky's work as in a terminal. Pulls are fast-forward only, git never waits on a password prompt (it uses your credential helper or SSH agent, or says why it can't), and a worktree is never removed while it has uncommitted or untracked files, another worktree inside it, or scripts running in it.
- `src-tauri/src/mcp.rs` is the MCP server for AI agents. `ScriptDeck --mcp`, which an agent launches, pipes its stdio to a Unix socket the app opens in its config folder while the switch is on. Only your user can open the socket, and no network port is opened.
- `src-tauri/src/permission.rs` asks macOS whether ScriptDeck may post notifications, since the notification plugin answers "granted" on every desktop. Only a bundled build can ask, and `pnpm dev` posts as Terminal, so there Settings always shows them as allowed.
- Projects and settings are stored in `config.json` in the app config folder (`~/Library/Application Support/com.scriptdec.desktop/` on macOS). The bundle identifier keeps the app's old name, ScriptDec: changing it would move this folder, and installed apps would lose their projects and settings.

## Contributing

Bug reports and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) and the [code of conduct](CODE_OF_CONDUCT.md).

## License

[MIT](LICENSE) © 2026 Stefanos Vichas. ScriptDeck bundles [xterm.js](https://xtermjs.org) (MIT); its license is in `ui/vendor/`.
