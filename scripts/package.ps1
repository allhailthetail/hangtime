<#
.SYNOPSIS
    Builds a release zip of Hangtime for Windows x64.

.DESCRIPTION
    Runs the checks (format, clippy, tests), builds the release exe, and
    writes dist\hangtime-<version>-windows-x64.zip with a .sha256 beside
    it. The zip holds hangtime.exe, install.cmd, install.ps1, the user
    docs, the licenses, and third-party notices. See docs\RELEASING.md.
    Needs PowerShell 7 (pwsh), whose zips use standard path separators.

.PARAMETER SkipChecks
    Skips format, clippy, and tests, for a quick local build.
#>
#Requires -Version 7
param([switch]$SkipChecks)

$ErrorActionPreference = 'Stop'
$Root = Split-Path $PSScriptRoot -Parent

# Writes the license of every dependency built into the exe, with the
# license files each crate ships (including bundled C libraries such as
# libopus), as the MIT, Apache, and BSD licenses require.
function Write-Notices([string]$Path) {
    $metadata = cargo metadata --format-version 1 --locked --filter-platform x86_64-pc-windows-msvc |
        ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $workspace = @($metadata.workspace_members)
    $nodes = @{}
    foreach ($node in $metadata.resolve.nodes) { $nodes[$node.id] = $node }
    # Walk normal and build dependencies from the binary, skipping dev ones.
    $root = $metadata.packages | Where-Object { $_.name -eq 'hangtime' }
    $seen = @{}
    $queue = [System.Collections.Generic.Queue[string]]::new()
    $queue.Enqueue($root.id)
    while ($queue.Count -gt 0) {
        $id = $queue.Dequeue()
        if ($seen.ContainsKey($id)) { continue }
        $seen[$id] = $true
        foreach ($dep in $nodes[$id].deps) {
            if ($dep.dep_kinds | Where-Object { $_.kind -ne 'dev' }) { $queue.Enqueue($dep.pkg) }
        }
    }
    $pattern = '^(LICEN[CS]E|COPYING|NOTICE)([-._].*)?$'
    $out = [System.Text.StringBuilder]::new()
    [void]$out.AppendLine('Hangtime includes the following third-party software.')
    $packages = $metadata.packages |
        Where-Object { $seen.ContainsKey($_.id) -and $workspace -notcontains $_.id } |
        Sort-Object name, version
    foreach ($package in $packages) {
        $dir = Split-Path $package.manifest_path -Parent
        [void]$out.AppendLine()
        [void]$out.AppendLine(('=' * 72))
        [void]$out.AppendLine("$($package.name) $($package.version)")
        [void]$out.AppendLine("License: $($package.license)")
        if ($package.repository) { [void]$out.AppendLine("Source: $($package.repository)") }
        $files = @(Get-ChildItem $dir -File | Where-Object { $_.Name -match $pattern })
        # Bundled C sources keep their own license one folder down.
        $files += @(Get-ChildItem $dir -Directory | ForEach-Object {
            Get-ChildItem $_.FullName -File | Where-Object { $_.Name -match $pattern }
        })
        foreach ($file in $files) {
            [void]$out.AppendLine(('-' * 72))
            [void]$out.AppendLine($file.FullName.Substring($dir.Length + 1))
            [void]$out.AppendLine()
            [void]$out.AppendLine((Get-Content -Raw $file.FullName).TrimEnd())
        }
    }
    Set-Content -Path $Path -Value $out.ToString() -Encoding utf8
    Write-Host "== notices: $($packages.Count) packages"
}

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
    Copy-Item CHANGELOG.md, LICENSE-MIT, LICENSE-APACHE $stage
    Write-Notices (Join-Path $stage 'THIRD-PARTY-NOTICES.txt')

    Compress-Archive -Path $stage -DestinationPath $zip
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    Set-Content -Path "$zip.sha256" -Value "$hash  $name.zip" -Encoding ascii

    Write-Host ""
    Write-Host "Built $zip"
    Write-Host "sha256 $hash"
} finally {
    Pop-Location
}
