# ps

`ps` is a Rust-built PowerShell utility CLI. Its first job is to give users a
small amount of durable control over their PowerShell profile: saved paths and
prefixless shortcut commands.

PowerShell already ships with a `ps` alias for `Get-Process`, so this tool
installs a managed profile block that removes that alias in new sessions and
replaces it with the `ps` CLI function.

## Install

After this repository is published, users can install with cURL:

```powershell
curl.exe -fsSL https://raw.githubusercontent.com/krispowers/ps/main/scripts/install.ps1 | powershell -NoProfile -ExecutionPolicy Bypass -
```

For local development from this repo:

```powershell
cargo build --release
.\target\release\ps.exe init --yes
. $PROFILE
```

The installer places `ps.exe` in `%LOCALAPPDATA%\ps\bin`, adds that directory to
the user `Path`, and runs `ps init`.

## Commands

```powershell
ps init
ps commands
ps config-path settings
ps run kill 3000
```

Once the profile bridge is loaded, configured shortcuts do not need the `ps`
prefix:

```powershell
kill 3000
homebase
```

`ports` is also available without the `ps` prefix. It opens an interactive menu
of TCP ports and connections, showing port, state, local address, remote address,
and owning process. Each row has a `Kill` action to the right. Up/Down
navigation stays on port rows, Enter opens a detail view, Right moves to `Kill`,
Left moves back to the row, and `R` refreshes. The Rust-backed menu shows 20
ports per page and updates only the changed rows while you move around.

```powershell
ports
ports -p 3000
ports -s established
ports -n chrome
ports --sort process
ports --refresh
```

Other built-in prefixless utilities:

```powershell
doctor
procs
processes
procs -t "project"
history
envs
reload
mkcd scratch
```

`doctor` checks the profile bridge, managed config files, saved paths, PATH, and
history/session storage. `procs` and `processes` open a process menu with a kill
action. Use `procs -n <name>` to filter by process name or path, and
`procs -t <title>` to filter by the title/name text shown in the menu. `history` opens saved terminal
history and runs the selected command in the current shell. `envs` opens an
environment variable browser. `reload` validates and dot-sources `$PROFILE`.
`mkcd` creates a directory and moves into it.

## Saved Paths

Type `@` by itself and press Enter to open the saved paths menu. The menu starts
with an `Actions` section containing `Save current path` and `Clear saved paths`.
Saved paths print below in a `Saved Paths` section, sorted by most recent access.
Clearing asks for confirmation before removing anything.

```powershell
@
```

Selecting a saved path changes the current PowerShell session to that path.
Each saved path also has a `Delete` button to its right. Up/Down navigation stays
on paths; press Right while a path is selected to move to its delete button.
Deleting a single path asks for confirmation first.

For now, `@` does not accept text after it:

```powershell
@ api
```

That prints an error. If there is text before `@`, `ps` ignores it so normal
PowerShell text is not accidentally treated as a path shortcut.

## Settings

Type `%` by itself and press Enter to open the settings menu:

```powershell
%
```

Settings are stored in:

```text
%APPDATA%\ps\settings.json
```

Settings are grouped into sections:

```text
Prompt
  Short PWD
  Display Timestamps

Terminal History
  Terminal History
  History Max Length

Session History
  Session History
```

`Short PWD` changes the prompt path to the last two path parts. `Display
Timestamps` adds a timestamp chip before the path using the local machine time
in `HH:MM:SS` format. When disabled, no label chip is shown. Press Enter to
toggle the highlighted setting. `History Max Length` uses Left/Right for quick
adjustments, or Enter to type an exact number. The settings menu stays open
after changes; press Esc to close it.

When `Terminal History` is enabled, commands are saved per current path under:

```text
%APPDATA%\ps\history
```

The per-path history file is capped by `History Max Length`.

When `Session History` is enabled, each prompt updates:

```text
%APPDATA%\ps\sessions.json
```

On the next PowerShell start, `ps` restores paths from sessions that were marked
closed or whose recorded PowerShell process is no longer running, even if other
PowerShell instances are still open. It restores the most recent path into the
current session and opens additional restorable paths as Windows Terminal tabs
when `wt.exe` is available. If Windows Terminal is not available, it falls back
to new PowerShell windows. Spawned restore shells skip restore to avoid loops.

`ps commands` opens:

```text
%APPDATA%\ps\commands.json
```

Reload the current PowerShell session after editing commands:

```powershell
reload
```

## Shortcut Types

`kill-port` creates commands like `kill 3000`, finds the owning process for a TCP
port, and stops it.

```json
{
  "type": "kill-port",
  "description": "Stop the process listening on a TCP port."
}
```

`workspace` changes the current shell path and can open extra PowerShell windows
in other paths.

```json
{
  "type": "workspace",
  "description": "Jump to my main workspace.",
  "path": "C:\\Users\\krisp\\OneDrive\\Documents\\GitHub",
  "open_windows": [
    "C:\\Users\\krisp\\OneDrive\\Documents\\GitHub\\api",
    "C:\\Users\\krisp\\OneDrive\\Documents\\GitHub\\web"
  ]
}
```

`shell` runs PowerShell in the current session. Arguments passed to the shortcut
are available as `$psArgs`.

```json
{
  "type": "shell",
  "description": "Go to the desktop and list files.",
  "script": "Set-Location -LiteralPath $HOME\\Desktop\nGet-ChildItem"
}
```

Shell commands can also point at a `.ps1` file with `script_path`. By default,
script files are dot-sourced so they can import functions or update the current
session. Set `dot_source` to `false` to run the script with `&` instead.

```json
{
  "type": "shell",
  "description": "Load my local workspace helpers.",
  "script_path": "C:\\Users\\krisp\\scripts\\workspace.ps1",
  "dot_source": true
}
```

You can combine `script_path` and `script` when a command needs to import a file
and then run a little inline PowerShell:

```json
{
  "type": "shell",
  "description": "Load helpers and jump to the API.",
  "script_path": "C:\\Users\\krisp\\scripts\\workspace.ps1",
  "script": "Open-Workspace api",
  "dot_source": true
}
```

Shortcut names must start with a letter or underscore and may contain letters,
numbers, underscores, and hyphens.
