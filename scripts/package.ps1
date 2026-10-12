<#
.SYNOPSIS
    Builds a release zip of Hangtime for Windows x64.

.DESCRIPTION
    Runs the checks (format, clippy, tests), builds the release exe, and
    writes dist\hangtime-<version>-windows-x64.zip with a .sha256 beside
    it. The zip holds hangtime.exe, install.cmd, install.ps1, and the
    user docs. See docs\RELEASING.md.

.PARAMETER SkipChecks
    Skips format, clippy, and tests, for a quick local build.
#>
param([switch]$SkipChecks)

$ErrorActionPreference = 'Stop'
$Root = Split-Path $PSScriptRoot -Parent

function Invoke-Step([string]$Name, [scriptblock]$Command) {
    Write-Host "== $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Name failed" }
}

Push-Location $Root
try {
    $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
    $name = "hangtime-$version-windows-x64"

    if (-not $SkipChecks) {
        Invoke-Step 'format' { cargo fmt --all -- --check }
        Invoke-Step 'clippy' { cargo clippy --workspace --all-targets --locked -- -D warnings }
        Invoke-Step 'tests' { cargo test --workspace --locked }
    }
    Invoke-Step 'release build' { cargo build --release --locked }

    $built = (& target\release\hangtime.exe --version)
    if ($built -ne "hangtime $version") {
        throw "built exe reports '$built', expected 'hangtime $version'"
    }

    $dist = Join-Path $Root 'dist'
    $stage = Join-Path $dist $name
    $zip = Join-Path $dist "$name.zip"
    Remove-Item -Recurse -Force $stage, $zip, "$zip.sha256" -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item target\release\hangtime.exe, scripts\install.cmd, scripts\install.ps1 $stage
    Copy-Item docs\QUICKSTART.md (Join-Path $stage 'README.md')
    Copy-Item CHANGELOG.md $stage

    Compress-Archive -Path $stage -DestinationPath $zip
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    Set-Content -Path "$zip.sha256" -Value "$hash  $name.zip" -Encoding ascii

    Write-Host ""
    Write-Host "Built $zip"
    Write-Host "sha256 $hash"
} finally {
    Pop-Location
}
