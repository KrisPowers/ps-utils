# PowerShell Utilities

PowerShell Utilities is a fast, Rust-built toolkit for everyday PowerShell
work. The command stays short—`ps`—and adds interactive tools for ports,
processes, saved paths, workspaces, history, environment variables, and custom
shortcuts.

> PowerShell normally reserves `ps` as an alias for `Get-Process`. Setup replaces
> that alias with a small profile bridge; `Get-Process` itself remains available.

## Install

The prebuilt release supports 64-bit Windows. From PowerShell:

```powershell
curl.exe -fsSL https://raw.githubusercontent.com/KrisPowers/ps-utils/main/scripts/install.ps1 | powershell -NoProfile -ExecutionPolicy Bypass -
```

Restart PowerShell, or load the updated profile immediately:

```powershell
. $PROFILE
ps doctor
```

The installer places `ps.exe` in `%LOCALAPPDATA%\ps\bin`, adds that directory to
your user `PATH`, and configures both PowerShell 7 and Windows PowerShell
profiles. It installs the latest release by default. To pin a release:

```powershell
$env:PS_INSTALL_VERSION = "v0.1.0"
curl.exe -fsSL https://raw.githubusercontent.com/KrisPowers/ps-utils/main/scripts/install.ps1 | powershell -NoProfile -ExecutionPolicy Bypass -
```

## Quick start

Most utilities can be called directly after setup:

```powershell
ports                  # Browse TCP ports and stop an owning process
procs                  # Browse and stop processes
history                # Search saved command history
envs                   # Browse environment variables
workspaces             # Save or open a multi-tab workspace
@                      # Save, open, or remove favorite paths
%                      # Change ps settings
mkcd scratch           # Create a directory and enter it
reload                 # Validate and reload your PowerShell profile
```

The `ps` prefix is always available too:

```powershell
ps ports -p 3000
ps procs -n node
ps history -q cargo
ps envs -q path
ps --help
```

## Utilities

### Ports and processes

`ports` shows TCP port, state, local and remote addresses, and the owning
process. Select a row for details or move right to its `Kill` action.

```powershell
ports -p 3000
ports -s established
ports -n chrome
ports --sort process
ports --refresh
```

`procs` (also `processes`) opens the process menu:

```powershell
procs -n node
procs -t "project"
```

### Workspaces

Run `workspaces` to save the current PowerShell tabs as a named workspace or
open an existing one. Saved workspaces are ordered by recent use.

```powershell
workspaces
workspaces open api
```

For scripted setup, add a workspace from the current directory:

```powershell
ps workspaces add api
```

Or define its main path and extra tabs explicitly:

```powershell
ps workspaces add suite `
  --path C:\Projects\suite `
  --open C:\Projects\suite\api `
  --open C:\Projects\suite\web

ps workspaces list
ps workspaces remove suite
```

### Saved paths and settings

Type `@` by itself to open the saved-path menu. Save the current directory,
jump to a favorite, or remove paths without editing configuration.

Type `%` by itself to manage:

- shortened prompt paths and prompt timestamps;
- per-directory terminal history and its maximum length;
- restoration of paths from closed PowerShell sessions.

Settings, paths, workspaces, history, and session data live under
`%APPDATA%\ps`.

### Custom shortcuts

Open the shortcut configuration:

```powershell
ps commands
```

Reload the profile after editing it:

```powershell
reload
```

Three shortcut types are supported:

```json
{
  "version": 1,
  "commands": {
    "kill": {
      "type": "kill-port",
      "description": "Stop the process listening on a TCP port."
    },
    "api": {
      "type": "workspace",
      "description": "Open the API workspace.",
      "path": "C:\\Projects\\api",
      "open_windows": ["C:\\Projects\\api\\docs"]
    },
    "serve": {
      "type": "shell",
      "description": "Start the development server.",
      "script": "npm run dev"
    }
  }
}
```

Arguments supplied to a `shell` shortcut are available to its PowerShell script
as `$psArgs`. A shell shortcut can also use `script_path` and optionally set
`dot_source` to `false`.

Shortcut names must start with a letter or underscore and may contain letters,
numbers, underscores, and hyphens.

## Command reference

| Command | Purpose |
| --- | --- |
| `ps init` | Install or refresh the managed profile bridge |
| `ps doctor` | Check profiles, config, `PATH`, history, and session storage |
| `ps ports` | Browse and filter TCP connections |
| `ps procs` | Browse and filter processes |
| `ps history` | Search saved terminal history |
| `ps envs` | Browse environment variables |
| `ps workspaces` | Save, list, open, or remove workspaces |
| `ps commands` | Open custom shortcut configuration |
| `ps config-path <file>` | Print a managed config path |
| `ps run <shortcut> [args]` | Run a shortcut in a child process |

Use `ps <command> --help` for every option.

## Build from source

Requires a current stable Rust toolchain:

```powershell
git clone https://github.com/KrisPowers/ps-utils.git
Set-Location ps-utils
cargo test
cargo build --release
.\target\release\ps.exe init --yes
. $PROFILE
```

## License

[MIT](LICENSE)
