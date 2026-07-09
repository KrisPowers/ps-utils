use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::{
    config::{self, ShortcutCommand},
    profile::{self, InstallResult, ShellTarget},
};

const MODULE_NAME: &str = "PsUtils";
const MODULE_GUID: &str = "8d4fa06f-bdbd-49c8-9b8b-3774f8c8f5a3";
const BUILTIN_COMMANDS: &[&str] = &[
    "ports",
    "doctor",
    "zip",
    "compress",
    "pack",
    "procs",
    "processes",
    "history",
    "envs",
    "workspaces",
    "mkcd",
];
const RESERVED_COMMANDS: &[&str] = &["ps", "init", "commands", "config-path", "run", "emit"];

pub fn install(shell: ShellTarget, exe_path: &Path) -> Result<Vec<InstallResult>> {
    config::ensure_all()?;

    let commands = command_exports()?;
    let module = render_module(exe_path, &commands);
    let manifest = render_manifest(&commands);
    let mut results = Vec::new();

    for dir in module_dirs(shell)? {
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create module directory {}", dir.display()))?;

        let module_path = dir.join(format!("{MODULE_NAME}.psm1"));
        let manifest_path = dir.join(format!("{MODULE_NAME}.psd1"));
        let module_changed = write_if_changed(&module_path, &module)?;
        let manifest_changed = write_if_changed(&manifest_path, &manifest)?;

        results.push(InstallResult {
            path: module_path,
            changed: module_changed || manifest_changed,
        });
    }

    Ok(results)
}

fn command_exports() -> Result<Vec<String>> {
    let config = config::load_commands()?;
    let mut commands = BUILTIN_COMMANDS
        .iter()
        .map(|command| command.to_string())
        .collect::<BTreeSet<_>>();

    for (name, command) in config.commands {
        if !crate::shortcut::valid_shortcut_name(&name) {
            continue;
        }

        if is_reserved_command(&name) {
            continue;
        }

        match command {
            ShortcutCommand::KillPort { .. }
            | ShortcutCommand::Workspace { .. }
            | ShortcutCommand::Shell { .. } => {
                commands.insert(name);
            }
        }
    }

    Ok(commands.into_iter().collect())
}

fn is_reserved_command(name: &str) -> bool {
    RESERVED_COMMANDS
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
        || BUILTIN_COMMANDS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

pub(crate) fn module_dirs(shell: ShellTarget) -> Result<Vec<PathBuf>> {
    let documents = profile::documents_dir()?;
    let mut paths = Vec::new();

    match shell {
        ShellTarget::All => {
            paths.push(
                documents
                    .join("PowerShell")
                    .join("Modules")
                    .join(MODULE_NAME),
            );
            paths.push(
                documents
                    .join("WindowsPowerShell")
                    .join("Modules")
                    .join(MODULE_NAME),
            );
        }
        ShellTarget::Pwsh => {
            paths.push(
                documents
                    .join("PowerShell")
                    .join("Modules")
                    .join(MODULE_NAME),
            );
        }
        ShellTarget::WindowsPowerShell => {
            paths.push(
                documents
                    .join("WindowsPowerShell")
                    .join("Modules")
                    .join(MODULE_NAME),
            );
        }
    }

    Ok(paths)
}

fn render_module(exe_path: &Path, commands: &[String]) -> String {
    let exe = ps_quote(&exe_path.display().to_string());
    let custom_commands = commands
        .iter()
        .filter(|command| {
            !BUILTIN_COMMANDS
                .iter()
                .any(|builtin| builtin.eq_ignore_ascii_case(command))
        })
        .cloned()
        .collect::<Vec<_>>();

    let mut module = format!(
        r#"# Managed by ps. Re-run `ps install-commands` after moving the binary or changing shortcuts.
$script:PsCliPath = {exe}

function __psInvokeCli {{
    param(
        [string]$Command,
        [Parameter(ValueFromRemainingArguments = $true)]
        [object[]]$Arguments
    )

    & $script:PsCliPath $Command @Arguments
    $__psExitCode = $LASTEXITCODE
    $global:LASTEXITCODE = $__psExitCode
}}

function __psInvokeShortcut {{
    param(
        [string]$Name,
        [Parameter(ValueFromRemainingArguments = $true)]
        [object[]]$Arguments
    )

    $__psEmitted = & $script:PsCliPath emit $Name @Arguments
    $__psExitCode = $LASTEXITCODE
    $global:LASTEXITCODE = $__psExitCode
    if ($__psExitCode -ne 0) {{ return }}

    $__psScript = $__psEmitted -join [Environment]::NewLine
    if (-not [string]::IsNullOrWhiteSpace($__psScript)) {{
        Invoke-Expression $__psScript
    }}
}}

function ports {{
    param(
        [Alias('p')]
        [int]$Port = 0,
        [Parameter(ValueFromRemainingArguments = $true)]
        [object[]]$Arguments
    )

    $__psPortsArgs = @('ports')

    if ($PSBoundParameters.ContainsKey('Port')) {{
        $__psPortsArgs += @('-p', [string]$Port)
    }}

    if ($null -ne $Arguments -and $Arguments.Count -gt 0) {{
        $__psPortsArgs += $Arguments
    }}

    & $script:PsCliPath @__psPortsArgs
    $__psExitCode = $LASTEXITCODE
    $global:LASTEXITCODE = $__psExitCode
}}

function doctor {{
    __psInvokeCli doctor @args
}}

function zip {{
    __psInvokeCli zip @args
}}

function compress {{
    __psInvokeCli compress @args
}}

function pack {{
    __psInvokeCli pack @args
}}

function procs {{
    __psInvokeCli procs @args
}}

function processes {{
    __psInvokeCli processes @args
}}

function envs {{
    __psInvokeCli envs @args
}}

function workspaces {{
    $resultPath = Join-Path ([IO.Path]::GetTempPath()) "ps-workspaces-$PID-$([Guid]::NewGuid().ToString('N')).ps1"

    try {{
        & $script:PsCliPath workspaces --result $resultPath @args
        $__psWorkspacesExitCode = $LASTEXITCODE
        $global:LASTEXITCODE = $__psWorkspacesExitCode

        if ($__psWorkspacesExitCode -ne 0) {{ return }}
        if (-not (Test-Path -LiteralPath $resultPath)) {{ return }}

        $scriptText = Get-Content -LiteralPath $resultPath -Raw
        if (-not [string]::IsNullOrWhiteSpace($scriptText)) {{
            Invoke-Expression $scriptText
        }}
    }} finally {{
        if (Test-Path -LiteralPath $resultPath) {{
            Remove-Item -LiteralPath $resultPath -Force -ErrorAction SilentlyContinue
        }}
    }}
}}

function history {{
    $resultPath = Join-Path ([IO.Path]::GetTempPath()) "ps-history-$PID-$([Guid]::NewGuid().ToString('N')).txt"

    try {{
        & $script:PsCliPath history --result $resultPath @args
        $__psHistoryExitCode = $LASTEXITCODE
        $global:LASTEXITCODE = $__psHistoryExitCode

        if ($__psHistoryExitCode -ne 0) {{ return }}
        if (-not (Test-Path -LiteralPath $resultPath)) {{ return }}

        $command = Get-Content -LiteralPath $resultPath -Raw
        if (-not [string]::IsNullOrWhiteSpace($command)) {{
            Invoke-Expression $command
        }}
    }} finally {{
        if (Test-Path -LiteralPath $resultPath) {{
            Remove-Item -LiteralPath $resultPath -Force -ErrorAction SilentlyContinue
        }}
    }}
}}

function mkcd {{
    param(
        [Parameter(Mandatory = $true, Position = 0)]
        [string]$Path
    )

    New-Item -ItemType Directory -Force -Path $Path | Out-Null
    Set-Location -LiteralPath $Path
}}

"#
    );

    for command in custom_commands {
        module.push_str(&format!(
            "function {command} {{\n    __psInvokeShortcut {} @args\n}}\n\n",
            ps_quote(&command)
        ));
    }

    module.push_str(&format!(
        "$__psExportedCommands = @({})\nforeach ($__psExportedCommand in $__psExportedCommands) {{\n    if (Test-Path -LiteralPath \"Alias:$__psExportedCommand\") {{\n        Remove-Item -LiteralPath \"Alias:$__psExportedCommand\" -Force -ErrorAction SilentlyContinue\n    }}\n}}\n\n",
        ps_array(commands)
    ));

    module.push_str(&format!(
        "Export-ModuleMember -Function @({})\n",
        ps_array(commands)
    ));

    module
}

fn render_manifest(commands: &[String]) -> String {
    format!(
        r#"@{{
    RootModule = 'PsUtils.psm1'
    ModuleVersion = '{}'
    GUID = '{MODULE_GUID}'
    Author = 'ps'
    Description = 'Profile-independent PowerShell commands for ps.'
    FunctionsToExport = @({})
    CmdletsToExport = @()
    VariablesToExport = @()
    AliasesToExport = @()
}}
"#,
        env!("CARGO_PKG_VERSION"),
        ps_array(commands)
    )
}

fn write_if_changed(path: &Path, contents: &str) -> Result<bool> {
    if fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(false);
    }

    fs::write(path, contents)
        .with_context(|| format!("failed to write module file {}", path.display()))?;
    Ok(true)
}

fn ps_array(values: &[String]) -> String {
    values
        .iter()
        .map(|value| ps_quote(value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_exports_builtin_commands() {
        let commands = BUILTIN_COMMANDS
            .iter()
            .map(|command| command.to_string())
            .collect::<Vec<_>>();
        let module = render_module(Path::new("C:\\tools\\ps.exe"), &commands);

        assert!(module.contains("function ports"));
        assert!(module.contains("function workspaces"));
        assert!(module.contains("function mkcd"));
        assert!(module.contains("$__psExportedCommands = @("));
        assert!(module.contains("Remove-Item -LiteralPath \"Alias:$__psExportedCommand\""));
        assert!(module.contains("Export-ModuleMember -Function"));
        assert!(module.contains("'ports'"));
    }

    #[test]
    fn module_exports_custom_shortcuts() {
        let commands = vec!["ports".to_string(), "serve-api".to_string()];
        let module = render_module(Path::new("C:\\tools\\ps.exe"), &commands);
        let manifest = render_manifest(&commands);

        assert!(module.contains("function serve-api"));
        assert!(module.contains("__psInvokeShortcut 'serve-api' @args"));
        assert!(manifest.contains("'serve-api'"));
    }

    #[test]
    fn reserved_commands_do_not_become_custom_exports() {
        assert!(is_reserved_command("ports"));
        assert!(is_reserved_command("ps"));
        assert!(!is_reserved_command("serve"));
    }
}
