param(
    [string]$Repo = $env:PS_INSTALL_REPO,
    [string]$Version = $env:PS_INSTALL_VERSION,
    [string]$InstallRoot = $env:PS_INSTALL_ROOT,
    [switch]$NoProfile
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Repo)) {
    $Repo = "https://github.com/KrisPowers/ps-utils"
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = "latest"
}

if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
    $InstallRoot = Join-Path $env:LOCALAPPDATA "ps"
}

$BinDir = Join-Path $InstallRoot "bin"
$ExePath = Join-Path $BinDir "ps.exe"
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

function Add-UserPath {
    param([string]$PathToAdd)

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = @()
    if (-not [string]::IsNullOrWhiteSpace($userPath)) {
        $parts = $userPath -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }

    if ($parts -notcontains $PathToAdd) {
        $newPath = (@($parts) + $PathToAdd) -join ";"
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    }

    if (($env:Path -split ";") -notcontains $PathToAdd) {
        $env:Path = "$PathToAdd;$env:Path"
    }
}

function Install-FromRelease {
    param(
        [string]$RepoUrl,
        [string]$RequestedVersion
    )

    $slug = $RepoUrl -replace "^https://github.com/", ""
    $slug = $slug.TrimEnd("/")
    if ($slug -notmatch "^[^/]+/[^/]+$") {
        throw "Repo must look like https://github.com/owner/repo"
    }

    if ($RequestedVersion -eq "latest") {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$slug/releases/latest"
    } else {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$slug/releases/tags/$RequestedVersion"
    }

    $asset = $release.assets |
        Where-Object { $_.name -eq "ps-windows-x86_64.zip" -or $_.name -eq "ps.exe" } |
        Select-Object -First 1

    if (-not $asset) {
        throw "No ps-windows-x86_64.zip or ps.exe asset was found on release $($release.tag_name)."
    }

    $temp = Join-Path ([IO.Path]::GetTempPath()) ("ps-install-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Force -Path $temp | Out-Null

    try {
        $downloadPath = Join-Path $temp $asset.name
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $downloadPath

        if ($asset.name.EndsWith(".zip")) {
            Expand-Archive -Path $downloadPath -DestinationPath $temp -Force
            $found = Get-ChildItem -Path $temp -Recurse -Filter "ps.exe" | Select-Object -First 1
            if (-not $found) {
                throw "Release archive did not contain ps.exe."
            }
            Copy-Item -Path $found.FullName -Destination $ExePath -Force
        } else {
            Copy-Item -Path $downloadPath -Destination $ExePath -Force
        }
    } finally {
        Remove-Item -Path $temp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Install-FromCargo {
    param([string]$RepoUrl)

    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "Cargo was not found, and no prebuilt release could be installed. Install Rust or publish a ps-windows-x86_64.zip release."
    }

    $cargoToml = Join-Path (Get-Location) "Cargo.toml"
    if (Test-Path -LiteralPath $cargoToml) {
        cargo build --release
        Copy-Item -Path (Join-Path (Get-Location) "target\release\ps.exe") -Destination $ExePath -Force
    } else {
        cargo install --git $RepoUrl --root $InstallRoot --force
    }
}

try {
    Install-FromRelease -RepoUrl $Repo -RequestedVersion $Version
} catch {
    Write-Warning "Release install failed: $($_.Exception.Message)"
    Write-Host "Falling back to Cargo source install..."
    Install-FromCargo -RepoUrl $Repo
}

if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "Install failed; ps.exe was not found at $ExePath"
}

Add-UserPath -PathToAdd $BinDir

if ($NoProfile) {
    & $ExePath install-commands
} else {
    & $ExePath init --yes
}

Write-Host ""
Write-Host "Installed ps to $ExePath"
Write-Host "Restart PowerShell or run: Import-Module PsUtils -Force"
if (-not $NoProfile) {
    Write-Host "To load profile hooks immediately, also run: . `$PROFILE"
}
