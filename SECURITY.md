# Security policy

Please report security problems privately, not in a public issue: use [Report a vulnerability](https://github.com/svichas/ScriptDec/security/advisories/new) on the Security tab.

Only the latest release gets security fixes.

## What counts

ScriptDec runs the scripts in your `package.json` files with your user's permissions, so running a project's scripts is as safe as the project itself. A vulnerability is ScriptDec doing something you didn't ask for, for example:

- running a command other than the script you started,
- letting a script name, workspace name or `.nvmrc` from a repo inject shell commands,
- opening anything other than a `localhost` address found in script output,
- letting an AI agent in while its switch is off, or letting one start anything but a listed script or type into a terminal,
- installing an update that isn't signed with the release key.
