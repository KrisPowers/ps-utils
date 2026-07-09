use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::config::{self, ConfigFile};

const START_MARKER: &str = "# >>> ps cli >>>";
const END_MARKER: &str = "# <<< ps cli <<<";

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ShellTarget {
    All,
    Pwsh,
    WindowsPowerShell,
}

#[derive(Debug, Clone)]
pub struct InstallResult {
    pub path: PathBuf,
    pub changed: bool,
}

pub fn install(shell: ShellTarget, exe_path: &Path) -> Result<Vec<InstallResult>> {
    config::ensure_all()?;

    let block = render_block(exe_path);
    let mut results = Vec::new();

    for path in profile_paths(shell)? {
        let changed = upsert_profile_block(&path, &block)?;
        results.push(InstallResult { path, changed });
    }

    Ok(results)
}

pub fn render_block(exe_path: &Path) -> String {
    let exe = ps_quote(&exe_path.display().to_string());
    let commands = ps_quote(
        &config::config_path(ConfigFile::Commands)
            .display()
            .to_string(),
    );
    let paths = ps_quote(&config::config_path(ConfigFile::Paths).display().to_string());
    let settings = ps_quote(
        &config::config_path(ConfigFile::Settings)
            .display()
            .to_string(),
    );

    format!(
        r#"{START_MARKER}
# Managed by ps. Re-run `ps init` after moving the binary.
$script:PsCliPath = {exe}
$script:PsCommandsPath = {commands}
$script:PsPathsPath = {paths}
$script:PsSettingsPath = {settings}
$script:PsConfigDir = Split-Path -Parent $script:PsSettingsPath
$script:PsHistoryDir = Join-Path $script:PsConfigDir 'history'
$script:PsSessionsPath = Join-Path $script:PsConfigDir 'sessions.json'
$script:PsSessionId = [Guid]::NewGuid().ToString()

if (Test-Path Alias:ps) {{
    Remove-Item Alias:ps -Force -ErrorAction SilentlyContinue
}}

function global:ps {{
    & $script:PsCliPath @args
    $__psExitCode = $LASTEXITCODE
    $global:LASTEXITCODE = $__psExitCode
}}

function global:__psNormalizePathText {{
    param([object]$Path)

    if ($null -eq $Path) {{ return '' }}

    $value = [string]$Path
    if ($value.StartsWith('\\?\')) {{
        return $value.Substring(4)
    }}

    return $value
}}

function global:__psGetCurrentPath {{
    try {{
        $providerPath = (Get-Location).ProviderPath
        if (-not [string]::IsNullOrWhiteSpace($providerPath)) {{
            return (__psNormalizePathText $providerPath)
        }}
    }} catch {{ }}

    return (__psNormalizePathText (Get-Location).Path)
}}

function global:__psNowUnixSeconds {{
    return [int64](([DateTime]::UtcNow - [DateTime]'1970-01-01').TotalSeconds)
}}

function global:__psEmptyPathsConfig {{
    return [pscustomobject]@{{
        version = 1
        saved = @()
    }}
}}

function global:__psEnsurePathsFile {{
    if (Test-Path -LiteralPath $script:PsPathsPath) {{ return }}

    $dir = Split-Path -Parent $script:PsPathsPath
    if (-not [string]::IsNullOrWhiteSpace($dir)) {{
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }}

    __psEmptyPathsConfig | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $script:PsPathsPath -Encoding UTF8
}}

function global:__psLoadPathsConfig {{
    __psEnsurePathsFile

    try {{
        $raw = Get-Content -LiteralPath $script:PsPathsPath -Raw
        if ([string]::IsNullOrWhiteSpace($raw)) {{
            return (__psEmptyPathsConfig)
        }}

        $config = $raw | ConvertFrom-Json
        if ($null -eq $config) {{
            return (__psEmptyPathsConfig)
        }}

        if ($null -eq $config.saved) {{
            $config | Add-Member -MemberType NoteProperty -Name saved -Value @() -Force
        }}

        return $config
    }} catch {{
        return (__psEmptyPathsConfig)
    }}
}}

function global:__psSavePathsConfig {{
    param([object[]]$Saved)

    $dir = Split-Path -Parent $script:PsPathsPath
    if (-not [string]::IsNullOrWhiteSpace($dir)) {{
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }}

    $payload = [ordered]@{{
        version = 1
        saved = @($Saved)
    }}

    $payload | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $script:PsPathsPath -Encoding UTF8
}}

function global:__psEmptySettingsConfig {{
    return [pscustomobject]@{{
        version = 1
        short_pwd = $false
        display_timestamps = $false
        terminal_history = $false
        terminal_history_max_length = 200
        session_history = $false
    }}
}}

function global:__psEnsureSettingsFile {{
    if (Test-Path -LiteralPath $script:PsSettingsPath) {{ return }}

    $dir = Split-Path -Parent $script:PsSettingsPath
    if (-not [string]::IsNullOrWhiteSpace($dir)) {{
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }}

    __psEmptySettingsConfig | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $script:PsSettingsPath -Encoding UTF8
}}

function global:__psLoadSettingsConfig {{
    __psEnsureSettingsFile

    try {{
        $raw = Get-Content -LiteralPath $script:PsSettingsPath -Raw
        if ([string]::IsNullOrWhiteSpace($raw)) {{
            return (__psEmptySettingsConfig)
        }}

        $settings = $raw | ConvertFrom-Json
        if ($null -eq $settings) {{
            return (__psEmptySettingsConfig)
        }}

        if ($null -eq $settings.PSObject.Properties['short_pwd']) {{
            $settings | Add-Member -MemberType NoteProperty -Name short_pwd -Value $false -Force
        }}

        if ($null -eq $settings.PSObject.Properties['display_timestamps']) {{
            $settings | Add-Member -MemberType NoteProperty -Name display_timestamps -Value $false -Force
        }}

        if ($null -eq $settings.PSObject.Properties['terminal_history']) {{
            $settings | Add-Member -MemberType NoteProperty -Name terminal_history -Value $false -Force
        }}

        if ($null -eq $settings.PSObject.Properties['terminal_history_max_length']) {{
            $settings | Add-Member -MemberType NoteProperty -Name terminal_history_max_length -Value 200 -Force
        }}

        if ($null -eq $settings.PSObject.Properties['session_history']) {{
            $settings | Add-Member -MemberType NoteProperty -Name session_history -Value $false -Force
        }}

        $settings.short_pwd = [bool]$settings.short_pwd
        $settings.display_timestamps = [bool]$settings.display_timestamps
        $settings.terminal_history = [bool]$settings.terminal_history
        $settings.session_history = [bool]$settings.session_history

        try {{
            $settings.terminal_history_max_length = [Math]::Max(10, [Math]::Min(5000, [int]$settings.terminal_history_max_length))
        }} catch {{
            $settings.terminal_history_max_length = 200
        }}

        return $settings
    }} catch {{
        return (__psEmptySettingsConfig)
    }}
}}

function global:__psSaveSettingsConfig {{
    param([object]$Settings)

    $dir = Split-Path -Parent $script:PsSettingsPath
    if (-not [string]::IsNullOrWhiteSpace($dir)) {{
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }}

    $payload = [ordered]@{{
        version = 1
        short_pwd = [bool]$Settings.short_pwd
        display_timestamps = [bool]$Settings.display_timestamps
        terminal_history = [bool]$Settings.terminal_history
        terminal_history_max_length = [Math]::Max(10, [Math]::Min(5000, [int]$Settings.terminal_history_max_length))
        session_history = [bool]$Settings.session_history
    }}

    $payload | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $script:PsSettingsPath -Encoding UTF8
}}

function global:__psToggleSetting {{
    param([string]$Key)

    $settings = __psLoadSettingsConfig

    switch ($Key) {{
        'short_pwd' {{
            $settings.short_pwd = -not [bool]$settings.short_pwd
            break
        }}
        'display_timestamps' {{
            $settings.display_timestamps = -not [bool]$settings.display_timestamps
            break
        }}
        'terminal_history' {{
            $settings.terminal_history = -not [bool]$settings.terminal_history
            break
        }}
        'session_history' {{
            $settings.session_history = -not [bool]$settings.session_history
            break
        }}
    }}

    __psSaveSettingsConfig $settings
    return (__psLoadSettingsConfig)
}}

function global:__psAdjustSetting {{
    param(
        [string]$Key,
        [int]$Delta
    )

    $settings = __psLoadSettingsConfig

    switch ($Key) {{
        'terminal_history_max_length' {{
            return (__psSetNumericSetting $Key ([int]$settings.terminal_history_max_length + $Delta))
        }}
    }}

    return $settings
}}

function global:__psSetNumericSetting {{
    param(
        [string]$Key,
        [int]$Value
    )

    $settings = __psLoadSettingsConfig

    switch ($Key) {{
        'terminal_history_max_length' {{
            $settings.terminal_history_max_length = [Math]::Max(10, [Math]::Min(5000, $Value))
            break
        }}
    }}

    __psSaveSettingsConfig $settings
    return (__psLoadSettingsConfig)
}}

function global:__psShortenPromptPath {{
    param([string]$Path)

    $clean = __psNormalizePathText $Path
    $parts = @($clean -split '[\\/]' | Where-Object {{ -not [string]::IsNullOrWhiteSpace($_) }})

    if ($parts.Count -le 2) {{
        return $clean
    }}

    return ($parts[($parts.Count - 2)..($parts.Count - 1)] -join '\')
}}

function global:__psPromptPathText {{
    $path = __psGetCurrentPath
    $settings = __psLoadSettingsConfig

    if ([bool]$settings.short_pwd) {{
        return (__psShortenPromptPath $path)
    }}

    return $path
}}

function global:__psPromptLabelText {{
    $settings = __psLoadSettingsConfig

    if ([bool]$settings.display_timestamps) {{
        return (Get-Date -Format 'HH:mm:ss')
    }}

    return ''
}}

function global:prompt {{
    try {{
        __psUpdateSessionSnapshot

        $label = __psPromptLabelText
        $path = __psPromptPathText

        if (-not [string]::IsNullOrWhiteSpace($label)) {{
            Write-Host " $label " -NoNewline -ForegroundColor Black -BackgroundColor White
            Write-Host " " -NoNewline
        }}

        Write-Host $path -NoNewline -ForegroundColor Gray
        Write-Host " > " -NoNewline -ForegroundColor Gray

        return " "
    }} catch {{
        return "PS $($executionContext.SessionState.Path.CurrentLocation)> "
    }}
}}

function global:__psPathHash {{
    param([string]$Path)

    $normalized = (__psNormalizePathText $Path).ToLowerInvariant()
    $sha = [System.Security.Cryptography.SHA256]::Create()

    try {{
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($normalized)
        $hash = [BitConverter]::ToString($sha.ComputeHash($bytes)).Replace('-', '').ToLowerInvariant()
        return $hash.Substring(0, 16)
    }} finally {{
        $sha.Dispose()
    }}
}}

function global:__psHistoryPathForCurrentDirectory {{
    $currentPath = __psGetCurrentPath
    $hash = __psPathHash $currentPath
    return (Join-Path $script:PsHistoryDir "$hash.json")
}}

function global:__psRecordTerminalHistory {{
    param([string]$CommandLine)

    $settings = __psLoadSettingsConfig
    if (-not [bool]$settings.terminal_history) {{ return }}
    if ([string]::IsNullOrWhiteSpace($CommandLine)) {{ return }}

    $trimmed = $CommandLine.Trim()
    if ($trimmed -eq '@' -or $trimmed -eq '%') {{ return }}

    New-Item -ItemType Directory -Force -Path $script:PsHistoryDir | Out-Null

    $currentPath = __psGetCurrentPath
    $historyPath = __psHistoryPathForCurrentDirectory
    $entries = @()

    if (Test-Path -LiteralPath $historyPath) {{
        try {{
            $raw = Get-Content -LiteralPath $historyPath -Raw
            if (-not [string]::IsNullOrWhiteSpace($raw)) {{
                $existing = $raw | ConvertFrom-Json
                $entries = @(@($existing.commands) | Where-Object {{ $null -ne $_ }})
            }}
        }} catch {{
            $entries = @()
        }}
    }}

    $entries += [pscustomobject]@{{
        command = $trimmed
        path = $currentPath
        timestamp = (__psNowUnixSeconds)
    }}

    $maxLength = [Math]::Max(10, [Math]::Min(5000, [int]$settings.terminal_history_max_length))
    if ($entries.Count -gt $maxLength) {{
        $entries = @($entries | Select-Object -Last $maxLength)
    }}

    $payload = [ordered]@{{
        version = 1
        path = $currentPath
        commands = @($entries)
    }}

    $payload | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $historyPath -Encoding UTF8
}}

function global:__psEmptySessionsConfig {{
    return [pscustomobject]@{{
        version = 1
        last_updated = 0
        sessions = @()
    }}
}}

function global:__psLoadSessionsConfig {{
    if (-not (Test-Path -LiteralPath $script:PsSessionsPath)) {{
        return (__psEmptySessionsConfig)
    }}

    try {{
        $raw = Get-Content -LiteralPath $script:PsSessionsPath -Raw -ErrorAction Stop
        if ([string]::IsNullOrWhiteSpace($raw)) {{
            return (__psEmptySessionsConfig)
        }}

        $sessions = $raw | ConvertFrom-Json
        if ($null -eq $sessions -or $null -eq $sessions.sessions) {{
            return (__psEmptySessionsConfig)
        }}

        return $sessions
    }} catch {{
        return (__psEmptySessionsConfig)
    }}
}}

function global:__psSaveSessionsConfig {{
    param([object[]]$Sessions)

    New-Item -ItemType Directory -Force -Path $script:PsConfigDir | Out-Null

    $payload = [ordered]@{{
        version = 1
        last_updated = (__psNowUnixSeconds)
        sessions = @($Sessions)
    }}

    try {{
        $payload | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $script:PsSessionsPath -Encoding UTF8 -ErrorAction Stop
    }} catch {{ }}
}}

function global:__psProcessStartUnixSeconds {{
    param([int]$ProcessId)

    try {{
        $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
        if ($null -eq $process -or $null -eq $process.StartTime) {{ return 0 }}

        return [int64]([DateTimeOffset]$process.StartTime).ToUnixTimeSeconds()
    }} catch {{
        return 0
    }}
}}

function global:__psUpdateSessionSnapshot {{
    $settings = __psLoadSettingsConfig
    $restoreEnabled = [bool]$settings.session_history

    $currentPath = __psGetCurrentPath
    if ([string]::IsNullOrWhiteSpace($currentPath)) {{ return }}

    $config = __psLoadSessionsConfig
    $now = (__psNowUnixSeconds)
    $sessions = @(@($config.sessions) | Where-Object {{
        $null -ne $_ -and
        -not [string]::IsNullOrWhiteSpace([string]$_.path) -and
        [string]$_.id -ne $script:PsSessionId
    }})

    $sessions += [pscustomobject]@{{
        id = $script:PsSessionId
        pid = $PID
        process_start = (__psProcessStartUnixSeconds $PID)
        path = $currentPath
        updated_at = $now
        status = 'open'
        closed_at = 0
        restored_at = 0
        restore_enabled = $restoreEnabled
        host = [string]$Host.Name
    }}

    $sessions = @($sessions |
        Sort-Object @{{ Expression = {{ if ($null -eq $_.updated_at) {{ 0 }} else {{ [int64]$_.updated_at }} }}; Descending = $true }} |
        Select-Object -First 12)

    __psSaveSessionsConfig $sessions
}}

function global:__psMarkSessionClosed {{
    $settings = __psLoadSettingsConfig
    $restoreEnabled = [bool]$settings.session_history

    $currentPath = __psGetCurrentPath
    if ([string]::IsNullOrWhiteSpace($currentPath)) {{ return }}

    $config = __psLoadSessionsConfig
    $now = (__psNowUnixSeconds)
    $sessions = @(@($config.sessions) | Where-Object {{
        $null -ne $_ -and
        -not [string]::IsNullOrWhiteSpace([string]$_.path) -and
        [string]$_.id -ne $script:PsSessionId
    }})

    $sessions += [pscustomobject]@{{
        id = $script:PsSessionId
        pid = $PID
        process_start = (__psProcessStartUnixSeconds $PID)
        path = $currentPath
        updated_at = $now
        status = 'closed'
        closed_at = $now
        restored_at = 0
        restore_enabled = $restoreEnabled
        host = [string]$Host.Name
    }}

    __psSaveSessionsConfig $sessions
}}

function global:__psSessionCanRestore {{
    param([object]$Session)

    if ($null -eq $Session) {{ return $false }}
    if ([string]::IsNullOrWhiteSpace([string]$Session.path)) {{ return $false }}

    if ($null -ne $Session.PSObject.Properties['restore_enabled'] -and -not [bool]$Session.restore_enabled) {{
        return $false
    }}

    $status = [string]$Session.status
    if ($status -eq 'closed' -or [string]::IsNullOrWhiteSpace($status)) {{
        return $true
    }}

    if ($status -eq 'open') {{
        try {{
            $sessionPid = [int]$Session.pid
            if ($sessionPid -gt 0) {{
                $process = Get-Process -Id $sessionPid -ErrorAction SilentlyContinue
                if ($null -eq $process) {{
                    return $true
                }}

                $sessionUpdatedAt = 0
                $processStartedAt = 0

                try {{ $sessionUpdatedAt = [int64]$Session.updated_at }} catch {{ }}
                try {{ $processStartedAt = [int64]([DateTimeOffset]$process.StartTime).ToUnixTimeSeconds() }} catch {{ }}

                if ($sessionUpdatedAt -gt 0 -and $processStartedAt -gt 0 -and $processStartedAt -gt $sessionUpdatedAt) {{
                    return $true
                }}

                return $false
            }}
        }} catch {{ }}

        return $true
    }}

    return $false
}}

function global:__psRestoreCandidateSessions {{
    param([object]$Config)

    $sessions = @(@($Config.sessions) | Where-Object {{
        __psSessionCanRestore $_
    }})

    return @($sessions |
        Sort-Object `
            @{{ Expression = {{ if ($null -eq $_.closed_at) {{ 0 }} else {{ [int64]$_.closed_at }} }}; Descending = $true }}, `
            @{{ Expression = {{ if ($null -eq $_.updated_at) {{ 0 }} else {{ [int64]$_.updated_at }} }}; Descending = $true }})
}}

function global:__psMarkSessionsRestored {{
    param([string[]]$Paths)

    if ($null -eq $Paths -or $Paths.Count -eq 0) {{ return }}

    $config = __psLoadSessionsConfig
    $now = (__psNowUnixSeconds)
    $sessions = @()

    foreach ($session in @($config.sessions)) {{
        if ($null -eq $session) {{ continue }}

        $isRestoredPath = $false
        foreach ($path in $Paths) {{
            if (__psSamePath ([string]$session.path) ([string]$path)) {{
                $isRestoredPath = $true
                break
            }}
        }}

        if ($isRestoredPath -and (__psSessionCanRestore $session)) {{
            $session | Add-Member -MemberType NoteProperty -Name status -Value 'restored' -Force
            $session | Add-Member -MemberType NoteProperty -Name restored_at -Value $now -Force
        }}

        $sessions += $session
    }}

    __psSaveSessionsConfig $sessions
}}

function global:__psLaunchSessionPath {{
    param([string]$Path)

    $clean = __psNormalizePathText $Path
    if ([string]::IsNullOrWhiteSpace($clean)) {{ return }}

    $escaped = $clean.Replace("'", "''")
    $escapedProfile = ([string]$PROFILE).Replace("'", "''")
    $command = "`$env:PS_SKIP_SESSION_RESTORE = '1'; . '$escapedProfile'; Set-Location -LiteralPath '$escaped'"
    $shell = ''

    try {{
        $shell = [string](Get-Process -Id $PID -ErrorAction SilentlyContinue).Path
    }} catch {{ }}

    if ([string]::IsNullOrWhiteSpace($shell)) {{
        if ($PSVersionTable.PSEdition -eq 'Core') {{
            $shell = (Get-Command pwsh.exe -ErrorAction SilentlyContinue).Source
        }} else {{
            $shell = (Get-Command powershell.exe -ErrorAction SilentlyContinue).Source
        }}
    }}

    if ([string]::IsNullOrWhiteSpace($shell)) {{
        $shell = 'powershell.exe'
    }}

    $arguments = @('-NoExit', '-NoProfile', '-Command', $command)

    if (Get-Command wt.exe -ErrorAction SilentlyContinue) {{
        Start-Process wt.exe -ArgumentList (@('new-tab', $shell) + $arguments)
    }} else {{
        Start-Process -FilePath $shell -ArgumentList $arguments
    }}
}}

function global:__psMaybeRestoreSession {{
    $settings = __psLoadSettingsConfig
    if (-not [bool]$settings.session_history) {{ return }}
    if ($env:PS_SKIP_SESSION_RESTORE -eq '1') {{ return }}

    $config = __psLoadSessionsConfig
    $paths = @(__psRestoreCandidateSessions $config |
        Select-Object -ExpandProperty path -Unique)

    if ($paths.Count -eq 0) {{ return }}

    __psMarkSessionsRestored $paths

    $first = [string]$paths[0]
    if (-not (__psSamePath (__psGetCurrentPath) $first)) {{
        try {{ Set-Location -LiteralPath $first }} catch {{ }}
    }}

    foreach ($path in @($paths | Select-Object -Skip 1)) {{
        __psLaunchSessionPath ([string]$path)
    }}
}}

function global:__psSamePath {{
    param(
        [string]$Left,
        [string]$Right
    )

    return [string]::Equals((__psNormalizePathText $Left), (__psNormalizePathText $Right), [StringComparison]::OrdinalIgnoreCase)
}}

function global:__psPathName {{
    param([string]$Path)

    $clean = __psNormalizePathText $Path
    $leaf = ''

    try {{
        $leaf = Split-Path -LiteralPath $clean -Leaf
    }} catch {{ }}

    if ([string]::IsNullOrWhiteSpace($leaf)) {{ $leaf = $clean }}
    if ([string]::IsNullOrWhiteSpace($leaf)) {{ $leaf = 'path' }}

    return $leaf
}}

function global:__psUniquePathName {{
    param(
        [object[]]$Saved,
        [string]$BaseName
    )

    $candidate = $BaseName
    $counter = 2

    while (@($Saved | Where-Object {{ [string]$_.name -eq $candidate }}).Count -gt 0) {{
        $candidate = "$BaseName-$counter"
        $counter += 1
    }}

    return $candidate
}}

function global:__psSortedSavedPaths {{
    param([object]$Config)

    $saved = @(@($Config.saved) | Where-Object {{
        $null -ne $_ -and -not [string]::IsNullOrWhiteSpace([string]$_.path)
    }})

    return @($saved | Sort-Object `
        @{{ Expression = {{ if ($null -eq $_.last_accessed) {{ 0 }} else {{ [int64]$_.last_accessed }} }}; Descending = $true }}, `
        @{{ Expression = {{ [string]$_.name }}; Ascending = $true }})
}}

function global:__psPathMenuEntries {{
    param(
        [object]$Config,
        [string]$CurrentPath
    )

    $entries = @()

    $entries += [pscustomobject]@{{
        index = $entries.Count
        kind = 'save'
        label = 'Save current path'
        path = $CurrentPath
        section = 'actions'
    }}

    $entries += [pscustomobject]@{{
        index = $entries.Count
        kind = 'clear'
        label = 'Clear saved paths'
        path = ''
        section = 'actions'
    }}

    foreach ($saved in (__psSortedSavedPaths $Config)) {{
        $path = __psNormalizePathText $saved.path

        $entries += [pscustomobject]@{{
            index = $entries.Count
            kind = 'open'
            label = $path
            path = $path
            section = 'paths'
        }}

        $entries += [pscustomobject]@{{
            index = $entries.Count
            kind = 'delete'
            label = 'Delete'
            path = $path
            section = 'paths'
        }}
    }}

    return $entries
}}

function global:__psClearMenuRegion {{
    param(
        [int]$Top,
        [int]$Height
    )

    try {{
        $width = [Math]::Max(1, [Console]::BufferWidth - 1)
        $blank = ' ' * $width
        $maxLine = [Math]::Min([Console]::BufferHeight - 1, $Top + $Height)

        for ($line = $Top; $line -le $maxLine; $line += 1) {{
            [Console]::SetCursorPosition(0, $line)
            [Console]::Write($blank)
        }}

        [Console]::SetCursorPosition(0, $Top)
    }} catch {{ }}
}}

function global:__psClearMenuLine {{
    param([int]$Line)

    try {{
        $width = [Math]::Max(1, [Console]::BufferWidth - 1)
        $blank = ' ' * $width
        [Console]::SetCursorPosition(0, $Line)
        [Console]::Write($blank)
        [Console]::SetCursorPosition(0, $Line)
        return $true
    }} catch {{
        return $false
    }}
}}

function global:__psClampMenuText {{
    param(
        [string]$Text,
        [int]$MaxLength
    )

    if ($MaxLength -lt 4) {{ return $Text }}
    if ($Text.Length -le $MaxLength) {{ return $Text }}

    return $Text.Substring(0, $MaxLength - 3) + '...'
}}

function global:__psWritePathMenuPath {{
    param(
        [object]$Entry,
        [object]$DeleteEntry,
        [bool]$PathSelected,
        [bool]$DeleteSelected
    )

    $deleteButtonWidth = 12
    $width = [Math]::Max(20, [Console]::BufferWidth - $deleteButtonWidth - 7)
    $path = __psClampMenuText (__psNormalizePathText $Entry.path) $width

    if ($PathSelected) {{
        Write-Host $path -NoNewline -ForegroundColor Black -BackgroundColor Yellow
    }} else {{
        Write-Host $path -NoNewline
    }}

    Write-Host '  ' -NoNewline

    if ($null -ne $DeleteEntry) {{
        __psWritePathMenuButton $DeleteEntry $DeleteSelected
    }}

    Write-Host ''
}}

function global:__psIsDestructiveMenuItem {{
    param([object]$Entry)

    if ($null -eq $Entry) {{ return $false }}

    $kind = [string]$Entry.kind
    $label = [string]$Entry.label

    return (
        $kind -eq 'delete' -or
        $kind -eq 'clear' -or
        $label.StartsWith('Delete') -or
        $label.StartsWith('Clear')
    )
}}

function global:__psWritePathMenuButton {{
    param(
        [object]$Entry,
        [bool]$Selected
    )

    $button = "[ $($Entry.label) ]"
    $isDestructive = __psIsDestructiveMenuItem $Entry

    if ($Selected) {{
        if ($isDestructive) {{
            Write-Host $button -NoNewline -ForegroundColor White -BackgroundColor DarkRed
        }} else {{
            Write-Host $button -NoNewline -ForegroundColor Black -BackgroundColor Yellow
        }}
    }} else {{
        if ($isDestructive) {{
            Write-Host $button -NoNewline -ForegroundColor Red
        }} else {{
            Write-Host $button -NoNewline
        }}
    }}
}}

function global:__psSettingsMenuEntries {{
    return @(
        [pscustomobject]@{{
            index = 0
            group = 'Prompt'
            kind = 'toggle'
            key = 'short_pwd'
            label = 'Short PWD'
        }},
        [pscustomobject]@{{
            index = 1
            group = 'Prompt'
            kind = 'toggle'
            key = 'display_timestamps'
            label = 'Display Timestamps'
        }},
        [pscustomobject]@{{
            index = 2
            group = 'Terminal History'
            kind = 'toggle'
            key = 'terminal_history'
            label = 'Terminal History'
        }},
        [pscustomobject]@{{
            index = 3
            group = 'Terminal History'
            kind = 'number'
            key = 'terminal_history_max_length'
            label = 'History Max Length'
        }},
        [pscustomobject]@{{
            index = 4
            group = 'Session History'
            kind = 'toggle'
            key = 'session_history'
            label = 'Session History'
        }}
    )
}}

function global:__psSettingEnabled {{
    param(
        [object]$Settings,
        [object]$Entry
    )

    switch ([string]$Entry.key) {{
        'short_pwd' {{ return [bool]$Settings.short_pwd }}
        'display_timestamps' {{ return [bool]$Settings.display_timestamps }}
        'terminal_history' {{ return [bool]$Settings.terminal_history }}
        'session_history' {{ return [bool]$Settings.session_history }}
        default {{ return $false }}
    }}
}}

function global:__psSettingValueText {{
    param(
        [object]$Settings,
        [object]$Entry
    )

    switch ([string]$Entry.key) {{
        'terminal_history_max_length' {{ return [string]$Settings.terminal_history_max_length }}
        default {{ return '' }}
    }}
}}

function global:__psWriteSettingsToggleButton {{
    param(
        [bool]$Enabled,
        [bool]$Selected
    )

    $label = if ($Enabled) {{ 'Enabled' }} else {{ 'Disabled' }}
    $button = "[ $label ]"

    if ($Selected) {{
        Write-Host $button -NoNewline -ForegroundColor Black -BackgroundColor Yellow
    }} elseif ($Enabled) {{
        Write-Host $button -NoNewline -ForegroundColor Green
    }} else {{
        Write-Host $button -NoNewline -ForegroundColor DarkGray
    }}
}}

function global:__psWriteSettingsValueButton {{
    param(
        [string]$Value,
        [bool]$Selected
    )

    $button = "[ $Value ]"

    if ($Selected) {{
        Write-Host $button -NoNewline -ForegroundColor Black -BackgroundColor Yellow
    }} else {{
        Write-Host $button -NoNewline -ForegroundColor Cyan
    }}
}}

function global:__psRenderSettingsMenu {{
    param(
        [object[]]$Entries,
        [object]$Settings,
        [int]$Selected,
        [int]$Top,
        [int]$Height
    )

    __psClearMenuRegion $Top $Height

    Write-Host 'Settings' -ForegroundColor DarkGray
    Write-Host ''

    $lastGroup = ''

    for ($index = 0; $index -lt $Entries.Count; $index += 1) {{
        $entry = $Entries[$index]
        $selectedRow = ([int]$entry.index -eq $Selected)
        $group = [string]$entry.group

        if ($group -ne $lastGroup) {{
            if (-not [string]::IsNullOrWhiteSpace($lastGroup)) {{
                Write-Host ''
            }}

            Write-Host $group -ForegroundColor DarkGray
            $lastGroup = $group
        }}

        Write-Host '  ' -NoNewline
        Write-Host ([string]$entry.label).PadRight(22) -NoNewline

        if ($entry.kind -eq 'number') {{
            __psWriteSettingsValueButton (__psSettingValueText $Settings $entry) $selectedRow
        }} else {{
            __psWriteSettingsToggleButton (__psSettingEnabled $Settings $entry) $selectedRow
        }}

        Write-Host ''
    }}

    Write-Host ''
    Write-Host 'Use Up/Down. Enter toggles or edits. Left/Right adjusts numbers. Esc closes.' -ForegroundColor DarkGray
}}

function global:__psSettingsEntryLine {{
    param(
        [object[]]$Entries,
        [int]$EntryIndex,
        [int]$Top
    )

    $line = $Top + 2
    $lastGroup = ''

    for ($index = 0; $index -lt $Entries.Count; $index += 1) {{
        $entry = $Entries[$index]
        $group = [string]$entry.group

        if ($group -ne $lastGroup) {{
            if (-not [string]::IsNullOrWhiteSpace($lastGroup)) {{
                $line += 1
            }}

            $line += 1
            $lastGroup = $group
        }}

        if ([int]$entry.index -eq $EntryIndex) {{
            return $line
        }}

        $line += 1
    }}

    return $Top
}}

function global:__psRenderSettingsEntryLine {{
    param(
        [object[]]$Entries,
        [object]$Settings,
        [int]$Selected,
        [int]$Top,
        [int]$EntryIndex
    )

    if ($EntryIndex -lt 0 -or $EntryIndex -ge $Entries.Count) {{ return }}

    $entry = $Entries[$EntryIndex]
    $line = __psSettingsEntryLine $Entries $EntryIndex $Top

    if (-not (__psClearMenuLine $line)) {{ return }}

    Write-Host '  ' -NoNewline
    Write-Host ([string]$entry.label).PadRight(22) -NoNewline

    if ($entry.kind -eq 'number') {{
        __psWriteSettingsValueButton (__psSettingValueText $Settings $entry) ([int]$entry.index -eq $Selected)
    }} else {{
        __psWriteSettingsToggleButton (__psSettingEnabled $Settings $entry) ([int]$entry.index -eq $Selected)
    }}

    Write-Host ''
    try {{ [Console]::Out.Flush() }} catch {{ }}
}}

function global:__psRenderSettingsSelectionChange {{
    param(
        [object[]]$Entries,
        [object]$Settings,
        [int]$OldSelected,
        [int]$Selected,
        [int]$Top
    )

    if ($OldSelected -eq $Selected) {{
        __psRenderSettingsEntryLine $Entries $Settings $Selected $Top $Selected
        return
    }}

    __psRenderSettingsEntryLine $Entries $Settings $Selected $Top $OldSelected
    __psRenderSettingsEntryLine $Entries $Settings $Selected $Top $Selected
}}

function global:__psRenderSettingsNumberInput {{
    param(
        [object]$Entry,
        [string]$Value,
        [int]$Top,
        [int]$Height
    )

    __psClearMenuRegion $Top $Height

    Write-Host ([string]$Entry.label) -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '[ ' -NoNewline
    Write-Host $Value -NoNewline -ForegroundColor Black -BackgroundColor Yellow
    Write-Host ' ]'
    Write-Host ''
    Write-Host 'Type a number. Enter saves. Esc cancels.' -ForegroundColor DarkGray
}}

function global:__psReadSettingsNumberInput {{
    param(
        [object]$Entry,
        [object]$Settings,
        [int]$Top,
        [int]$Height
    )

    $value = __psSettingValueText $Settings $Entry
    if ([string]::IsNullOrWhiteSpace($value)) {{
        $value = '0'
    }}

    while ($true) {{
        __psRenderSettingsNumberInput $Entry $value $Top $Height
        $keyInfo = [Console]::ReadKey($true)

        switch ($keyInfo.Key) {{
            'Escape' {{
                return (__psLoadSettingsConfig)
            }}
            'Enter' {{
                try {{
                    return (__psSetNumericSetting ([string]$Entry.key) ([int]$value))
                }} catch {{
                    $value = __psSettingValueText (__psLoadSettingsConfig) $Entry
                    break
                }}
            }}
            'Backspace' {{
                if ($value.Length -gt 0) {{
                    $value = $value.Substring(0, $value.Length - 1)
                }}

                if ([string]::IsNullOrWhiteSpace($value)) {{
                    $value = ''
                }}

                break
            }}
            default {{
                if ([char]::IsDigit($keyInfo.KeyChar) -and $value.Length -lt 5) {{
                    if ($value -eq '0') {{
                        $value = [string]$keyInfo.KeyChar
                    }} else {{
                        $value = "$value$($keyInfo.KeyChar)"
                    }}
                }}

                break
            }}
        }}
    }}
}}

function global:__psInvokeSettingsMenu {{
    $settings = __psLoadSettingsConfig
    $entries = @(__psSettingsMenuEntries)

    if ($entries.Count -eq 0) {{ return }}

    Write-Host ''
    Write-Host ''

    $selected = 0
    $top = [Console]::CursorTop
    $height = [Math]::Max(14, $entries.Count + 10)
    $oldCursorVisible = $true

    try {{
        $oldCursorVisible = [Console]::CursorVisible
        [Console]::CursorVisible = $false
    }} catch {{ }}

    try {{
        __psRenderSettingsMenu $entries $settings $selected $top $height

        while ($true) {{
            $oldSelected = $selected
            $renderFull = $false
            $renderSelected = $false
            $keyInfo = [Console]::ReadKey($true)

            switch ($keyInfo.Key) {{
                'UpArrow' {{
                    if ($selected -gt 0) {{ $selected -= 1 }}
                    break
                }}
                'DownArrow' {{
                    if ($selected + 1 -lt $entries.Count) {{ $selected += 1 }}
                    break
                }}
                'Home' {{
                    $selected = 0
                    break
                }}
                'End' {{
                    $selected = $entries.Count - 1
                    break
                }}
                'LeftArrow' {{
                    if ($entries[$selected].kind -eq 'number') {{
                        $settings = __psAdjustSetting ([string]$entries[$selected].key) -50
                        $renderSelected = $true
                    }}
                    break
                }}
                'RightArrow' {{
                    if ($entries[$selected].kind -eq 'number') {{
                        $settings = __psAdjustSetting ([string]$entries[$selected].key) 50
                        $renderSelected = $true
                    }}
                    break
                }}
                'Escape' {{
                    __psClearMenuRegion $top $height
                    return
                }}
                'Enter' {{
                    if ($entries[$selected].kind -eq 'number') {{
                        $settings = __psReadSettingsNumberInput $entries[$selected] $settings $top $height
                        $renderFull = $true
                    }} else {{
                        $settings = __psToggleSetting ([string]$entries[$selected].key)
                        $renderSelected = $true
                    }}
                    break
                }}
            }}

            if ($renderFull) {{
                __psRenderSettingsMenu $entries $settings $selected $top $height
            }} elseif ($oldSelected -ne $selected) {{
                __psRenderSettingsSelectionChange $entries $settings $oldSelected $selected $top
            }} elseif ($renderSelected) {{
                __psRenderSettingsEntryLine $entries $settings $selected $top $selected
            }}
        }}
    }} finally {{
        try {{
            [Console]::CursorVisible = $oldCursorVisible
        }} catch {{ }}
    }}
}}

function global:__psRenderPathMenu {{
    param(
        [object[]]$Entries,
        [int]$Selected,
        [int]$Top,
        [int]$Height,
        [string]$CurrentPath
    )

    __psClearMenuRegion $Top $Height

    $pathEntries = @($Entries | Where-Object {{ $_.section -eq 'paths' }})
    $actionEntries = @($Entries | Where-Object {{ $_.section -eq 'actions' }})

    Write-Host 'Actions' -ForegroundColor DarkGray

    for ($index = 0; $index -lt $actionEntries.Count; $index += 1) {{
        __psWritePathMenuButton $actionEntries[$index] ([int]$actionEntries[$index].index -eq $Selected)
        Write-Host ''
    }}

    Write-Host ''
    Write-Host 'Saved Paths' -ForegroundColor DarkGray

    $savedPathEntries = @($pathEntries | Where-Object {{ $_.kind -eq 'open' }})
    if ($savedPathEntries.Count -eq 0) {{
        Write-Host 'No saved paths yet.' -ForegroundColor DarkGray
    }} else {{
        for ($index = 0; $index -lt $savedPathEntries.Count; $index += 1) {{
            $pathEntry = $savedPathEntries[$index]
            $deleteEntry = @($pathEntries | Where-Object {{
                $_.kind -eq 'delete' -and (__psSamePath ([string]$_.path) ([string]$pathEntry.path))
            }} | Select-Object -First 1)[0]

            __psWritePathMenuPath `
                $pathEntry `
                $deleteEntry `
                ([int]$pathEntry.index -eq $Selected) `
                ($null -ne $deleteEntry -and [int]$deleteEntry.index -eq $Selected)
        }}
    }}

    Write-Host ''
    Write-Host ''
    Write-Host 'Use arrows. Enter selects. Esc cancels.' -ForegroundColor DarkGray
}}

function global:__psPathEntryLine {{
    param(
        [object[]]$Entries,
        [int]$EntryIndex,
        [int]$Top
    )

    if ($EntryIndex -lt 0 -or $EntryIndex -ge $Entries.Count) {{ return $Top }}

    $entry = $Entries[$EntryIndex]

    if ($entry.section -eq 'actions') {{
        $actionEntries = @($Entries | Where-Object {{ $_.section -eq 'actions' }})
        for ($index = 0; $index -lt $actionEntries.Count; $index += 1) {{
            if ([int]$actionEntries[$index].index -eq $EntryIndex) {{
                return ($Top + 1 + $index)
            }}
        }}
    }}

    if ($entry.section -eq 'paths') {{
        $openIndex = $EntryIndex
        if ($entry.kind -eq 'delete') {{
            $openIndex = __psOpenPathIndexForDelete $Entries $entry
        }}

        $savedPathEntries = @($Entries | Where-Object {{ $_.section -eq 'paths' -and $_.kind -eq 'open' }})
        for ($index = 0; $index -lt $savedPathEntries.Count; $index += 1) {{
            if ([int]$savedPathEntries[$index].index -eq [int]$openIndex) {{
                return ($Top + 5 + $index)
            }}
        }}
    }}

    return $Top
}}

function global:__psRenderPathEntryLine {{
    param(
        [object[]]$Entries,
        [int]$Selected,
        [int]$Top,
        [int]$EntryIndex
    )

    if ($EntryIndex -lt 0 -or $EntryIndex -ge $Entries.Count) {{ return }}

    $entry = $Entries[$EntryIndex]

    if ($entry.section -eq 'actions') {{
        $line = __psPathEntryLine $Entries $EntryIndex $Top
        if (-not (__psClearMenuLine $line)) {{ return }}

        __psWritePathMenuButton $entry ([int]$entry.index -eq $Selected)
        Write-Host ''
        try {{ [Console]::Out.Flush() }} catch {{ }}
        return
    }}

    if ($entry.section -eq 'paths') {{
        $pathEntry = $entry
        if ($entry.kind -eq 'delete') {{
            $pathEntry = $Entries[(__psOpenPathIndexForDelete $Entries $entry)]
        }}

        $deleteEntry = $null
        $deleteIndex = __psDeleteIndexForOpenPath $Entries $pathEntry
        if ($deleteIndex -ge 0 -and $deleteIndex -lt $Entries.Count -and $Entries[$deleteIndex].kind -eq 'delete') {{
            $deleteEntry = $Entries[$deleteIndex]
        }}

        $line = __psPathEntryLine $Entries ([int]$pathEntry.index) $Top
        if (-not (__psClearMenuLine $line)) {{ return }}

        __psWritePathMenuPath `
            $pathEntry `
            $deleteEntry `
            ([int]$pathEntry.index -eq $Selected) `
            ($null -ne $deleteEntry -and [int]$deleteEntry.index -eq $Selected)

        try {{ [Console]::Out.Flush() }} catch {{ }}
    }}
}}

function global:__psRenderPathSelectionChange {{
    param(
        [object[]]$Entries,
        [int]$OldSelected,
        [int]$Selected,
        [int]$Top
    )

    $oldLine = __psPathEntryLine $Entries $OldSelected $Top
    $newLine = __psPathEntryLine $Entries $Selected $Top

    if ($oldLine -eq $newLine) {{
        __psRenderPathEntryLine $Entries $Selected $Top $Selected
        return
    }}

    __psRenderPathEntryLine $Entries $Selected $Top $OldSelected
    __psRenderPathEntryLine $Entries $Selected $Top $Selected
}}

function global:__psSaveCurrentPath {{
    param([string]$CurrentPath)

    $config = __psLoadPathsConfig
    $saved = @(@($config.saved) | Where-Object {{ $null -ne $_ }})
    $now = (__psNowUnixSeconds)
    $found = $false

    foreach ($item in $saved) {{
        if (__psSamePath ([string]$item.path) $CurrentPath) {{
            $item.path = $CurrentPath
            $item.last_accessed = $now
            $found = $true
        }}
    }}

    if (-not $found) {{
        $baseName = __psPathName $CurrentPath
        $saved += [pscustomobject]@{{
            name = (__psUniquePathName $saved $baseName)
            path = $CurrentPath
            last_accessed = $now
        }}
    }}

    __psSavePathsConfig $saved
    Write-Host "Saved path: $CurrentPath" -ForegroundColor Green
}}

function global:__psOpenSavedPath {{
    param([string]$Path)

    $clean = __psNormalizePathText $Path
    $config = __psLoadPathsConfig
    $saved = @(@($config.saved) | Where-Object {{ $null -ne $_ }})
    $now = (__psNowUnixSeconds)

    foreach ($item in $saved) {{
        if (__psSamePath ([string]$item.path) $clean) {{
            $item.path = $clean
            $item.last_accessed = $now
        }}
    }}

    __psSavePathsConfig $saved
    Set-Location -LiteralPath $clean
}}

function global:__psClearSavedPaths {{
    __psSavePathsConfig @()
    Write-Host 'Cleared saved paths.' -ForegroundColor Red
}}

function global:__psDeleteSavedPath {{
    param([string]$Path)

    $clean = __psNormalizePathText $Path
    $config = __psLoadPathsConfig
    $saved = @(@($config.saved) | Where-Object {{
        $null -ne $_ -and -not (__psSamePath ([string]$_.path) $clean)
    }})

    __psSavePathsConfig $saved
    Write-Host "Deleted saved path: $clean" -ForegroundColor Red
}}

function global:__psRenderClearSavedPathsConfirm {{
    param(
        [int]$Top,
        [int]$Height,
        [int]$Selected
    )

    __psClearMenuRegion $Top $Height

    Write-Host 'Clear all saved paths?' -ForegroundColor Red
    Write-Host ''

    $buttons = @(
        [pscustomobject]@{{ label = 'Cancel' }},
        [pscustomobject]@{{ label = 'Clear saved paths' }}
    )

    for ($index = 0; $index -lt $buttons.Count; $index += 1) {{
        if ($index -gt 0) {{
            Write-Host '  ' -NoNewline
        }}

        __psWritePathMenuButton $buttons[$index] ($index -eq $Selected)
    }}

    Write-Host ''
    Write-Host ''
    Write-Host 'Use Left/Right. Enter selects. Esc cancels.' -ForegroundColor DarkGray
}}

function global:__psRenderConfirmButtonLine {{
    param(
        [object[]]$Buttons,
        [int]$Selected,
        [int]$Line
    )

    if (-not (__psClearMenuLine $Line)) {{ return }}

    for ($index = 0; $index -lt $Buttons.Count; $index += 1) {{
        if ($index -gt 0) {{
            Write-Host '  ' -NoNewline
        }}

        __psWritePathMenuButton $Buttons[$index] ($index -eq $Selected)
    }}

    Write-Host ''
    try {{ [Console]::Out.Flush() }} catch {{ }}
}}

function global:__psConfirmClearSavedPaths {{
    param(
        [int]$Top,
        [int]$Height
    )

    $selected = 0
    $buttons = @(
        [pscustomobject]@{{ label = 'Cancel' }},
        [pscustomobject]@{{ label = 'Clear saved paths' }}
    )

    __psRenderClearSavedPathsConfirm $Top $Height $selected

    while ($true) {{
        $oldSelected = $selected
        $keyInfo = [Console]::ReadKey($true)

        switch ($keyInfo.Key) {{
            'LeftArrow' {{
                if ($selected -gt 0) {{ $selected -= 1 }}
                break
            }}
            'RightArrow' {{
                if ($selected -lt 1) {{ $selected += 1 }}
                break
            }}
            'Home' {{
                $selected = 0
                break
            }}
            'End' {{
                $selected = 1
                break
            }}
            'Escape' {{
                return $false
            }}
            'Enter' {{
                return ($selected -eq 1)
            }}
        }}

        if ($oldSelected -ne $selected) {{
            __psRenderConfirmButtonLine $buttons $selected ($Top + 2)
        }}
    }}
}}

function global:__psRenderDeleteSavedPathConfirm {{
    param(
        [string]$Path,
        [int]$Top,
        [int]$Height,
        [int]$Selected
    )

    __psClearMenuRegion $Top $Height

    Write-Host 'Delete saved path?' -ForegroundColor Red
    Write-Host (__psNormalizePathText $Path) -ForegroundColor Red
    Write-Host ''

    $buttons = @(
        [pscustomobject]@{{ label = 'Cancel' }},
        [pscustomobject]@{{ label = 'Delete path' }}
    )

    for ($index = 0; $index -lt $buttons.Count; $index += 1) {{
        if ($index -gt 0) {{
            Write-Host '  ' -NoNewline
        }}

        __psWritePathMenuButton $buttons[$index] ($index -eq $Selected)
    }}

    Write-Host ''
    Write-Host ''
    Write-Host 'Use Left/Right. Enter selects. Esc cancels.' -ForegroundColor DarkGray
}}

function global:__psConfirmDeleteSavedPath {{
    param(
        [string]$Path,
        [int]$Top,
        [int]$Height
    )

    $selected = 0
    $buttons = @(
        [pscustomobject]@{{ label = 'Cancel' }},
        [pscustomobject]@{{ label = 'Delete path' }}
    )

    __psRenderDeleteSavedPathConfirm $Path $Top $Height $selected

    while ($true) {{
        $oldSelected = $selected
        $keyInfo = [Console]::ReadKey($true)

        switch ($keyInfo.Key) {{
            'LeftArrow' {{
                if ($selected -gt 0) {{ $selected -= 1 }}
                break
            }}
            'RightArrow' {{
                if ($selected -lt 1) {{ $selected += 1 }}
                break
            }}
            'Home' {{
                $selected = 0
                break
            }}
            'End' {{
                $selected = 1
                break
            }}
            'Escape' {{
                return $false
            }}
            'Enter' {{
                return ($selected -eq 1)
            }}
        }}

        if ($oldSelected -ne $selected) {{
            __psRenderConfirmButtonLine $buttons $selected ($Top + 3)
        }}
    }}
}}

function global:__psVerticalMenuIndexes {{
    param([object[]]$Entries)

    return @(@($Entries) | Where-Object {{ $_.kind -ne 'delete' }} | ForEach-Object {{ [int]$_.index }})
}}

function global:__psOpenPathIndexForDelete {{
    param(
        [object[]]$Entries,
        [object]$DeleteEntry
    )

    foreach ($entry in $Entries) {{
        if ($entry.kind -eq 'open' -and (__psSamePath ([string]$entry.path) ([string]$DeleteEntry.path))) {{
            return [int]$entry.index
        }}
    }}

    return 0
}}

function global:__psDeleteIndexForOpenPath {{
    param(
        [object[]]$Entries,
        [object]$OpenEntry
    )

    foreach ($entry in $Entries) {{
        if ($entry.kind -eq 'delete' -and (__psSamePath ([string]$entry.path) ([string]$OpenEntry.path))) {{
            return [int]$entry.index
        }}
    }}

    return [int]$OpenEntry.index
}}

function global:__psMoveMenuSelectionVertical {{
    param(
        [object[]]$Entries,
        [int]$Selected,
        [int]$Delta
    )

    $indexes = @(__psVerticalMenuIndexes $Entries)
    if ($indexes.Count -eq 0) {{ return $Selected }}

    $baseline = $Selected
    if ($Entries[$Selected].kind -eq 'delete') {{
        $baseline = __psOpenPathIndexForDelete $Entries $Entries[$Selected]
    }}

    $position = 0
    for ($index = 0; $index -lt $indexes.Count; $index += 1) {{
        if ([int]$indexes[$index] -eq [int]$baseline) {{
            $position = $index
            break
        }}
    }}

    $position = [Math]::Max(0, [Math]::Min($indexes.Count - 1, $position + $Delta))
    return [int]$indexes[$position]
}}

function global:__psInvokePathMenu {{
    $currentPath = __psGetCurrentPath
    $config = __psLoadPathsConfig
    $entries = @(__psPathMenuEntries $config $currentPath)

    if ($entries.Count -eq 0) {{ return }}

    Write-Host ''
    Write-Host ''

    $selected = 0
    $top = [Console]::CursorTop
    $height = [Math]::Max(12, $entries.Count + 9)
    $oldCursorVisible = $true

    try {{
        $oldCursorVisible = [Console]::CursorVisible
        [Console]::CursorVisible = $false
    }} catch {{ }}

    try {{
        __psRenderPathMenu $entries $selected $top $height $currentPath

        while ($true) {{
            $oldSelected = $selected
            $renderFull = $false
            $keyInfo = [Console]::ReadKey($true)

            switch ($keyInfo.Key) {{
                'UpArrow' {{
                    $selected = __psMoveMenuSelectionVertical $entries $selected -1
                    break
                }}
                'DownArrow' {{
                    $selected = __psMoveMenuSelectionVertical $entries $selected 1
                    break
                }}
                'LeftArrow' {{
                    if ($entries[$selected].kind -eq 'delete') {{
                        $selected = __psOpenPathIndexForDelete $entries $entries[$selected]
                    }}
                    break
                }}
                'RightArrow' {{
                    if ($entries[$selected].kind -eq 'open') {{
                        $selected = __psDeleteIndexForOpenPath $entries $entries[$selected]
                    }}
                    break
                }}
                'Home' {{
                    $indexes = @(__psVerticalMenuIndexes $entries)
                    if ($indexes.Count -gt 0) {{ $selected = [int]$indexes[0] }}
                    break
                }}
                'End' {{
                    $indexes = @(__psVerticalMenuIndexes $entries)
                    if ($indexes.Count -gt 0) {{ $selected = [int]$indexes[$indexes.Count - 1] }}
                    break
                }}
                'Escape' {{
                    __psClearMenuRegion $top $height
                    return
                }}
                'Enter' {{
                    $entry = $entries[$selected]

                    if ($entry.kind -eq 'save') {{
                        __psClearMenuRegion $top $height
                        __psSaveCurrentPath $currentPath
                        return
                    }} elseif ($entry.kind -eq 'clear') {{
                        __psClearMenuRegion $top $height
                        if (__psConfirmClearSavedPaths $top $height) {{
                            __psClearMenuRegion $top $height
                            __psClearSavedPaths
                            return
                        }}

                        $renderFull = $true
                        break
                    }} elseif ($entry.kind -eq 'delete') {{
                        __psClearMenuRegion $top $height
                        if (__psConfirmDeleteSavedPath ([string]$entry.path) $top $height) {{
                            __psClearMenuRegion $top $height
                            __psDeleteSavedPath ([string]$entry.path)
                            return
                        }}

                        $renderFull = $true
                        break
                    }} else {{
                        __psClearMenuRegion $top $height
                        __psOpenSavedPath ([string]$entry.path)
                        return
                    }}
                }}
            }}

            if ($renderFull) {{
                __psRenderPathMenu $entries $selected $top $height $currentPath
            }} elseif ($oldSelected -ne $selected) {{
                __psRenderPathSelectionChange $entries $oldSelected $selected $top
            }}
        }}
    }} finally {{
        try {{
            [Console]::CursorVisible = $oldCursorVisible
        }} catch {{ }}
    }}
}}

try {{
    Register-EngineEvent -SourceIdentifier PowerShell.Exiting -Action {{
        try {{
            __psMarkSessionClosed
        }} catch {{ }}
    }} | Out-Null
}} catch {{ }}

try {{
    if (-not [bool]$global:PsSessionRestoreAttempted) {{
        $global:PsSessionRestoreAttempted = $true
        __psMaybeRestoreSession
    }}
}} catch {{
    Write-Warning "ps session restore failed: $($_.Exception.Message)"
}}

foreach ($__psUtilityAlias in @('doctor', 'ports', 'procs', 'processes', 'history', 'envs', 'workspaces', 'zip', 'compress', 'pack', 'reload', 'mkcd')) {{
    if (Test-Path -LiteralPath "Alias:$__psUtilityAlias") {{
        Remove-Item -LiteralPath "Alias:$__psUtilityAlias" -Force -ErrorAction SilentlyContinue
    }}
}}

function global:reload {{
    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile($PROFILE, [ref]$tokens, [ref]$errors) | Out-Null

    if ($null -ne $errors -and $errors.Count -gt 0) {{
        foreach ($parseError in $errors) {{
            Write-Error $parseError.Message
        }}

        return
    }}

    . $PROFILE
    Write-Host 'Profile reloaded.' -ForegroundColor Green
}}

try {{
    if (Get-Command Set-PSReadLineKeyHandler -ErrorAction SilentlyContinue) {{
        Set-PSReadLineKeyHandler -Key Enter -ScriptBlock {{
            param($key, $arg)

            $__psLine = ''
            $__psCursor = 0
            [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$__psLine, [ref]$__psCursor)

            $__psTrimmedStart = $__psLine.TrimStart()
            $__psTrimmed = $__psLine.Trim()

            if ($__psTrimmedStart.StartsWith('@')) {{
                [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine($key, $arg)

                if ($__psTrimmed -eq '@') {{
                    __psInvokePathMenu
                }} else {{
                    Write-Host ''
                    Write-Error 'The @ path shortcut does not accept text yet. Use @ by itself.'
                }}

                [Microsoft.PowerShell.PSConsoleReadLine]::InvokePrompt($key, $arg)
                return
            }}

            if ($__psTrimmedStart.StartsWith('%')) {{
                [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine($key, $arg)

                if ($__psTrimmed -eq '%') {{
                    __psInvokeSettingsMenu
                }} else {{
                    Write-Host ''
                    Write-Error 'The % settings shortcut does not accept text yet. Use % by itself.'
                }}

                [Microsoft.PowerShell.PSConsoleReadLine]::InvokePrompt($key, $arg)
                return
            }}

            try {{
                __psRecordTerminalHistory $__psTrimmed
            }} catch {{ }}
            [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine($key, $arg)
        }}
    }}
}} catch {{
    Write-Warning "ps shortcut menus failed to bind: $($_.Exception.Message)"
}}
{END_MARKER}
"#
    )
}

pub fn profile_paths(shell: ShellTarget) -> Result<Vec<PathBuf>> {
    let documents = documents_dir()?;
    let mut paths = Vec::new();

    match shell {
        ShellTarget::All => {
            paths.push(
                documents
                    .join("PowerShell")
                    .join("Microsoft.PowerShell_profile.ps1"),
            );
            paths.push(
                documents
                    .join("WindowsPowerShell")
                    .join("Microsoft.PowerShell_profile.ps1"),
            );
        }
        ShellTarget::Pwsh => {
            paths.push(
                documents
                    .join("PowerShell")
                    .join("Microsoft.PowerShell_profile.ps1"),
            );
        }
        ShellTarget::WindowsPowerShell => {
            paths.push(
                documents
                    .join("WindowsPowerShell")
                    .join("Microsoft.PowerShell_profile.ps1"),
            );
        }
    }

    Ok(paths)
}

fn upsert_profile_block(path: &Path, block: &str) -> Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create profile directory {}", parent.display()))?;
    }

    let existing = fs::read_to_string(path).unwrap_or_default();
    let updated = replace_or_append_block(&existing, block);

    if existing == updated {
        return Ok(false);
    }

    fs::write(path, updated)
        .with_context(|| format!("failed to update profile {}", path.display()))?;
    Ok(true)
}

fn replace_or_append_block(existing: &str, block: &str) -> String {
    if let Some(start) = existing.find(START_MARKER)
        && let Some(relative_end) = existing[start..].find(END_MARKER)
    {
        let end = start + relative_end + END_MARKER.len();
        let mut updated = String::new();
        updated.push_str(&existing[..start]);
        updated.push_str(block.trim_end());
        updated.push_str(&existing[end..]);
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        return updated;
    }

    let mut updated = existing.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str(block.trim_end());
    updated.push('\n');
    updated
}

pub(crate) fn documents_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let output = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "[Environment]::GetFolderPath('MyDocuments')",
            ])
            .output()
            .context("failed to ask PowerShell for the Documents folder")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let path = stdout.trim();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }

    if let Some(home) = config::home_dir() {
        return Ok(home.join("Documents"));
    }

    bail!("could not resolve the Documents folder")
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_profile_block() {
        let updated = replace_or_append_block("Write-Host 'hello'\n", "BLOCK\n");
        assert!(updated.contains("Write-Host 'hello'"));
        assert!(updated.contains("BLOCK"));
    }

    #[test]
    fn replaces_existing_profile_block() {
        let existing = format!("before\n{START_MARKER}\nold\n{END_MARKER}\nafter\n");
        let updated =
            replace_or_append_block(&existing, &format!("{START_MARKER}\nnew\n{END_MARKER}\n"));

        assert!(updated.contains("before"));
        assert!(updated.contains("new"));
        assert!(!updated.contains("old"));
        assert!(updated.contains("after"));
    }

    #[test]
    fn generated_profile_keeps_ps_shortcut_valid() {
        let block = render_block(Path::new("C:\\Program Files\\ps\\ps.exe"));
        assert!(block.contains("function global:ps"));
        assert!(!block.contains("function global:__psInvokeShortcut"));
        assert!(block.contains("function global:__psInvokePathMenu"));
        assert!(block.contains("function global:__psRenderPathMenu"));
        assert!(block.contains("function global:__psRenderPathSelectionChange"));
        assert!(block.contains("function global:__psInvokeSettingsMenu"));
        assert!(block.contains("function global:__psRenderSettingsMenu"));
        assert!(block.contains("function global:__psRenderSettingsSelectionChange"));
        assert!(block.contains("function global:__psClearMenuLine"));
        assert!(block.contains("function global:__psRenderConfirmButtonLine"));
        assert!(block.contains("function global:__psToggleSetting"));
        assert!(block.contains("function global:__psAdjustSetting"));
        assert!(block.contains("function global:__psSetNumericSetting"));
        assert!(block.contains("function global:__psReadSettingsNumberInput"));
        assert!(block.contains("function global:__psRenderSettingsNumberInput"));
        assert!(block.contains("function global:__psPromptPathText"));
        assert!(block.contains("function global:__psPromptLabelText"));
        assert!(block.contains("function global:prompt"));
        assert!(block.contains("function global:__psRecordTerminalHistory"));
        assert!(block.contains("function global:__psProcessStartUnixSeconds"));
        assert!(block.contains("function global:__psUpdateSessionSnapshot"));
        assert!(block.contains("function global:__psMarkSessionClosed"));
        assert!(block.contains("function global:__psSessionCanRestore"));
        assert!(block.contains("function global:__psRestoreCandidateSessions"));
        assert!(block.contains("function global:__psMarkSessionsRestored"));
        assert!(block.contains("function global:__psMaybeRestoreSession"));
        assert!(block.contains("function global:__psLaunchSessionPath"));
        assert!(block.contains("function global:__psSaveCurrentPath"));
        assert!(block.contains("function global:__psOpenSavedPath"));
        assert!(block.contains("function global:__psClearSavedPaths"));
        assert!(block.contains("function global:__psDeleteSavedPath"));
        assert!(block.contains("function global:__psConfirmClearSavedPaths"));
        assert!(block.contains("function global:__psConfirmDeleteSavedPath"));
        assert!(block.contains("function global:__psIsDestructiveMenuItem"));
        assert!(block.contains("function global:__psMoveMenuSelectionVertical"));
        assert!(block.contains("$oldSelected = $selected"));
        assert!(
            block.contains("__psRenderPathSelectionChange $entries $oldSelected $selected $top")
        );
        assert!(block.contains(
            "__psRenderSettingsSelectionChange $entries $settings $oldSelected $selected $top"
        ));
        assert!(block.contains("function global:reload"));
        assert!(block.contains("Remove-Item -LiteralPath \"Alias:$__psUtilityAlias\""));
        assert!(block.contains("ParseFile($PROFILE, [ref]$tokens, [ref]$errors)"));
        assert!(!block.contains("function global:ports"));
        assert!(!block.contains("function global:doctor"));
        assert!(!block.contains("function global:zip"));
        assert!(!block.contains("function global:compress"));
        assert!(!block.contains("function global:pack"));
        assert!(!block.contains("function global:procs"));
        assert!(!block.contains("function global:processes"));
        assert!(!block.contains("function global:history"));
        assert!(!block.contains("function global:envs"));
        assert!(!block.contains("function global:workspaces"));
        assert!(!block.contains("function global:mkcd"));
        assert!(!block.contains("Set-Item -Path \"function:global:$name\""));
        assert!(!block.contains("& $script:PsCliPath history --result $resultPath @args"));
        assert!(!block.contains("& $script:PsCliPath workspaces --result $resultPath @args"));
        assert!(block.contains("section = 'paths'"));
        assert!(block.contains("section = 'actions'"));
        assert!(block.contains("kind = 'delete'"));
        assert!(block.contains("label = 'Clear saved paths'"));
        assert!(block.contains("label = 'Delete'"));
        assert!(block.contains("BackgroundColor DarkRed"));
        assert!(block.contains("ForegroundColor Red"));
        assert!(block.contains("Write-Host 'Delete saved path?' -ForegroundColor Red"));
        assert!(block.contains("Write-Host \"Deleted saved path: $clean\" -ForegroundColor Red"));
        assert!(block.contains("'doctor', 'ports', 'procs'"));
        assert!(!block.contains("[Alias('p')]"));
        assert!(!block.contains("$__psPortsArgs = @('ports')"));
        assert!(!block.contains("$__psPortsArgs += @('-p', [string]$Port)"));
        assert!(!block.contains("& $script:PsCliPath @__psPortsArgs"));
        assert!(!block.contains("function global:__psInvokePortsMenu"));
        assert!(!block.contains("Get-NetTCPConnection -State Listen"));
        assert!(block.contains("$script:PsPathsPath"));
        assert!(block.contains("$script:PsSettingsPath"));
        assert!(block.contains("short_pwd = $false"));
        assert!(block.contains("display_timestamps = $false"));
        assert!(block.contains("terminal_history = $false"));
        assert!(block.contains("terminal_history_max_length = 200"));
        assert!(block.contains("session_history = $false"));
        assert!(block.contains("label = 'Short PWD'"));
        assert!(block.contains("label = 'Display Timestamps'"));
        assert!(block.contains("label = 'Terminal History'"));
        assert!(block.contains("label = 'History Max Length'"));
        assert!(block.contains("label = 'Session History'"));
        assert!(block.contains("group = 'Prompt'"));
        assert!(block.contains("group = 'Terminal History'"));
        assert!(block.contains("group = 'Session History'"));
        assert!(block.contains("Write-Host $group -ForegroundColor DarkGray"));
        assert!(block.contains("Type a number. Enter saves. Esc cancels."));
        assert!(
            block
                .contains("__psReadSettingsNumberInput $entries[$selected] $settings $top $height")
        );
        assert!(block.contains("$script:PsHistoryDir"));
        assert!(block.contains("$script:PsSessionsPath"));
        assert!(block.contains("$entries = @(@($existing.commands)"));
        assert!(block.contains("commands = @($entries)"));
        assert!(block.contains("sessions = @($Sessions)"));
        assert!(block.contains("status = 'open'"));
        assert!(block.contains("status = 'closed'"));
        assert!(block.contains("Value 'restored' -Force"));
        assert!(
            block.contains(
                "Add-Member -MemberType NoteProperty -Name status -Value 'restored' -Force"
            )
        );
        assert!(block.contains("process_start = (__psProcessStartUnixSeconds $PID)"));
        assert!(block.contains("Register-EngineEvent -SourceIdentifier PowerShell.Exiting"));
        assert!(block.contains("Get-Process -Id $sessionPid"));
        assert!(block.contains("$processStartedAt -gt $sessionUpdatedAt"));
        assert!(block.contains("PS_SKIP_SESSION_RESTORE"));
        assert!(block.contains("$escapedProfile = ([string]$PROFILE).Replace"));
        assert!(block.contains("$command = \"`$env:PS_SKIP_SESSION_RESTORE = '1';"));
        assert!(block.contains("$arguments = @('-NoExit', '-NoProfile', '-Command', $command)"));
        assert!(
            block
                .contains("Start-Process wt.exe -ArgumentList (@('new-tab', $shell) + $arguments)")
        );
        assert!(block.contains("Start-Process -FilePath $shell -ArgumentList $arguments"));
        assert!(!block.contains("cmd.exe"));
        assert!(block.contains("if (-not [bool]$global:PsSessionRestoreAttempted)"));
        assert!(block.contains("$global:PsSessionRestoreAttempted = $true"));
        assert!(block.contains("__psRecordTerminalHistory $__psTrimmed"));
        assert!(block.contains("__psMaybeRestoreSession"));
        assert!(!block.contains(".session-restore-stamp"));
        assert!(block.contains("Get-Date -Format 'HH:mm:ss'"));
        assert!(block.contains("return ''"));
        assert!(block.contains("if (-not [string]::IsNullOrWhiteSpace($label))"));
        assert!(!block.contains("return 'ps'"));
        assert!(block.contains("Set-PSReadLineKeyHandler -Key Enter"));
        assert!(!block.contains("Write-Host '@ saved paths'"));
        assert!(block.contains("Write-Host 'Actions'"));
        assert!(block.contains("Write-Host 'Saved Paths'"));
        assert!(!block.contains("Write-Host '> ' -NoNewline"));
        assert!(block.contains("$saved = @(@($config.saved)"));
        assert!(block.contains("Write-Host ''\n    Write-Host ''\n\n    $selected = 0"));
        assert!(block.contains("__psInvokePathMenu"));
        assert!(block.contains("__psInvokeSettingsMenu"));
        assert!(
            block.contains("The % settings shortcut does not accept text yet. Use % by itself.")
        );
        assert!(block.contains("InvokePrompt($key, $arg)"));
        assert!(block.contains("AcceptLine($key, $arg)"));
        assert!(block.contains("RevertLine($key, $arg)"));
        assert!(!block.contains("Insert('__psInvokePathMenu')"));
        assert!(!block.contains("path-menu --current"));
        assert!(!block.contains("--result $__psResultPath"));
        assert!(!block.contains("InvokePrompt()"));
        assert!(!block.contains("$__psEmitted = & $script:PsCliPath path-menu"));
        assert!(!block.contains("$script:PsThemePath"));
        assert!(!block.contains("__psApplyTheme"));
        assert!(!block.contains("$Host.UI.RawUI 'BackgroundColor'"));
        assert!(block.contains("Remove-Item Alias:ps"));
        assert!(!block.contains("@args | Invoke-Expression"));
        assert!(crate::shortcut::valid_shortcut_name("workbench"));
    }
}
