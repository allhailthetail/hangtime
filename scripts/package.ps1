<#
.SYNOPSIS
    Builds the Hangtime release for Windows x64: an MSI and a portable zip.

.DESCRIPTION
    Runs the checks (format, clippy, tests), builds the release exe, and
    writes to dist\:

      hangtime-<version>-x64.msi           installer (per machine; Intune-ready)
      hangtime-<version>-windows-x64.zip   portable: exe and docs, no install
      SHA256SUMS-<version>.txt

    Every release regenerates README.html, the user guide the installer
    offers at the end, from docs\QUICKSTART.md and the release notes in
    CHANGELOG.md. CHANGELOG.md must have a section for this version.

    WiX v5 is downloaded from nuget.org on first use (checksums pinned
    below) into %LOCALAPPDATA%\hangtime-build and run on the installed
    .NET runtime; no .NET SDK or admin rights needed. See docs\RELEASING.md.

    Needs PowerShell 7 (pwsh) on Windows.

.PARAMETER SkipChecks
    Skips format, clippy, and tests, for a quick local build.
#>
#Requires -Version 7
param([switch]$SkipChecks)

$ErrorActionPreference = 'Stop'
$Root = Split-Path $PSScriptRoot -Parent

# WiX v5 and its extensions, from nuget.org.
$WixVersion = '5.0.2'
$WixPackages = [ordered]@{
    'wix'                       = 'f30ef0c74e2a986126539c5780be93ac24e8136eaf723b1937b26272703ae173'
    'WixToolset.UI.wixext'       = '5ef2c707614b9f70b6bbadd2d4abcb4124efee215e9b16bfbc80113079a604c7'
    'WixToolset.Util.wixext'     = 'dda1cc1b4d3b2305b1246f167c6af6fcded5000be84b3bbff3dda4f442bf4740'
    'WixToolset.Firewall.wixext' = 'c9a1c99afab82225127b34036b719dea6c98c451e21b341ae1c62a674d0ba5ef'
}

function Invoke-Step([string]$Name, [scriptblock]$Command) {
    Write-Host "== $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Name failed" }
}

# Downloads (once) and unpacks the WiX packages. Returns the wix.dll path
# and the extension DLL paths.
function Get-Wix {
    $cache = Join-Path $env:LOCALAPPDATA 'hangtime-build'
    $paths = @{}
    foreach ($name in $WixPackages.Keys) {
        $dir = Join-Path $cache "$($name.ToLower())-$WixVersion"
        if (-not (Test-Path $dir)) {
            $nupkg = "$dir.nupkg"
            Write-Host "   downloading $name $WixVersion"
            Invoke-WebRequest "https://www.nuget.org/api/v2/package/$name/$WixVersion" -OutFile $nupkg
            $hash = (Get-FileHash $nupkg -Algorithm SHA256).Hash.ToLower()
            if ($hash -ne $WixPackages[$name]) {
                Remove-Item $nupkg
                throw "$name $WixVersion checksum mismatch: $hash"
            }
            Expand-Archive $nupkg $dir
        }
        $paths[$name] = $dir
    }
    @{
        Wix        = Join-Path $paths['wix'] 'tools\net6.0\any\wix.dll'
        Extensions = @($WixPackages.Keys | Where-Object { $_ -ne 'wix' } | ForEach-Object {
            Join-Path $paths[$_] "wixext5\$_.dll"
        })
    }
}

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
    Write-Host "   notices: $($packages.Count) packages"
}

# README.html: the user guide plus this release's notes, styled, with the
# icon inline so the file stands alone.
function Write-Readme([string]$Path, [string]$Version) {
    $guide = (ConvertFrom-Markdown -Path docs\QUICKSTART.md).Html
    $notes = (ConvertFrom-Markdown -Path CHANGELOG.md).Html
    $icon = [Convert]::ToBase64String([IO.File]::ReadAllBytes("$Root\icons\windows\png\hangtime-64.png"))
    $html = @"
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Hangtime $Version</title>
<style>
  :root { color-scheme: light dark; --accent: #1A4FD0; --muted: #667; }
  body { font: 16px/1.55 "Segoe UI", system-ui, sans-serif; max-width: 46rem;
         margin: 0 auto; padding: 1.5rem 1rem 4rem; }
  header { display: flex; align-items: center; gap: 1rem; margin-bottom: 1rem; }
  header img { width: 56px; height: 56px; }
  header h1 { margin: 0; }
  header p { margin: 0; color: var(--muted); }
  h1, h2, h3 { line-height: 1.25; }
  h2 { margin-top: 2.2rem; border-bottom: 2px solid #3B93FF33; padding-bottom: .2rem; }
  code { font-family: Consolas, monospace; background: #8881; padding: .1rem .3rem; border-radius: 4px; }
  pre { background: #8881; padding: .8rem 1rem; border-radius: 8px; overflow-x: auto; }
  pre code { background: none; padding: 0; }
  a { color: #3B93FF; }
  section.notes > h1 { font-size: 1.6rem; margin-top: 3rem; }
  @media (prefers-color-scheme: dark) { :root { --muted: #99a; } body { background: #15171c; color: #e6e6e6; } }
</style>
</head>
<body>
<header>
  <img alt="" src="data:image/png;base64,$icon">
  <div><h1>Hangtime $Version</h1><p>Push-to-talk voice between computers</p></div>
</header>
<main>
$($guide -replace '<h1[^>]*>.*?</h1>', '')
<section class="notes">
$($notes -replace '<h1([^>]*)>Changelog</h1>', '<h1$1>Release notes</h1>')
</section>
</main>
</body>
</html>
"@
    Set-Content -Path $Path -Value $html -Encoding utf8
}

# The license shown in the installer, as RTF.
function Write-LicenseRtf([string]$Path) {
    $text = "Hangtime is licensed under either of the MIT license (below) or the " +
        "Apache License, Version 2.0 (LICENSE-APACHE, installed with Hangtime), " +
        "at your option.`n`n" + (Get-Content -Raw LICENSE-MIT)
    $escaped = $text.Replace('\', '\\').Replace('{', '\{').Replace('}', '\}') -replace "\r?\n", "\par`n"
    $rtf = "{\rtf1\ansi\deff0{\fonttbl{\f0 Segoe UI;}}\fs18 $escaped}"
    Set-Content -Path $Path -Value $rtf -Encoding ascii
}

# The installer's side and top images, in the icon's blue.
function Write-InstallerImages([string]$Dir) {
    Add-Type -AssemblyName System.Drawing
    $icon = [System.Drawing.Image]::FromFile("$Root\icons\windows\png\hangtime-256.png")
    $light = [System.Drawing.Color]::FromArgb(0x3B, 0x93, 0xFF)
    $dark = [System.Drawing.Color]::FromArgb(0x1A, 0x4F, 0xD0)
    try {
        # Welcome and finish pages: blue panel on the left, white behind the text.
        $dialog = [System.Drawing.Bitmap]::new(493, 312)
        $g = [System.Drawing.Graphics]::FromImage($dialog)
        $g.InterpolationMode = 'HighQualityBicubic'
        $g.Clear([System.Drawing.Color]::White)
        $panel = [System.Drawing.Rectangle]::new(0, 0, 164, 312)
        $brush = [System.Drawing.Drawing2D.LinearGradientBrush]::new($panel, $light, $dark, 90)
        $g.FillRectangle($brush, $panel)
        $g.DrawImage($icon, 22, 96, 120, 120)
        $g.Dispose()
        $dialog.Save((Join-Path $Dir 'dialog.bmp'), [System.Drawing.Imaging.ImageFormat]::Bmp)
        $dialog.Dispose()

        # Banner on the other pages: white, icon at the right.
        $banner = [System.Drawing.Bitmap]::new(493, 58)
        $g = [System.Drawing.Graphics]::FromImage($banner)
        $g.InterpolationMode = 'HighQualityBicubic'
        $g.Clear([System.Drawing.Color]::White)
        $g.DrawImage($icon, 427, 5, 48, 48)
        $g.Dispose()
        $banner.Save((Join-Path $Dir 'banner.bmp'), [System.Drawing.Imaging.ImageFormat]::Bmp)
        $banner.Dispose()
    } finally {
        $icon.Dispose()
    }
}

Push-Location $Root
try {
    $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
    if (-not (Select-String -Path CHANGELOG.md -Pattern "^## $([regex]::Escape($version)) " -Quiet)) {
        throw "CHANGELOG.md has no '## $version (...)' section; add the release notes first."
    }

    if (-not $SkipChecks) {
        Invoke-Step 'format' { cargo fmt --all -- --check }
        Invoke-Step 'clippy' { cargo clippy --workspace --all-targets --locked -- -D warnings }
        Invoke-Step 'tests' { cargo test --workspace --locked }
    }
    Invoke-Step 'release build' { cargo build --release --locked }
    $exe = Join-Path $Root 'target\release\hangtime.exe'
    $built = (& $exe --version)
    if ($built -ne "hangtime $version") {
        throw "built exe reports '$built', expected 'hangtime $version'"
    }

    # Files that ship next to the exe, in both the MSI and the zip.
    $work = Join-Path $Root 'target\package'
    $docs = Join-Path $work 'docs'
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $docs | Out-Null
    Write-Host '== docs'
    Write-Readme (Join-Path $docs 'README.html') $version
    Copy-Item CHANGELOG.md, LICENSE-MIT, LICENSE-APACHE $docs
    Write-Notices (Join-Path $docs 'THIRD-PARTY-NOTICES.txt')

    $dist = Join-Path $Root 'dist'
    New-Item -ItemType Directory -Force $dist | Out-Null
    $msi = Join-Path $dist "hangtime-$version-x64.msi"
    $zip = Join-Path $dist "hangtime-$version-windows-x64.zip"
    $sums = Join-Path $dist "SHA256SUMS-$version.txt"
    Remove-Item -Force $msi, $zip, $sums -ErrorAction SilentlyContinue

    Write-LicenseRtf (Join-Path $work 'license.rtf')
    Write-InstallerImages $work
    $wix = Get-Wix
    $extensions = $wix.Extensions | ForEach-Object { '-ext', $_ }
    Invoke-Step 'msi' {
        dotnet $wix.Wix build installer\hangtime.wxs -arch x64 @extensions `
            -d "Version=$version" -d "ExeFile=$exe" `
            -d "ReadmeFile=$docs\README.html" -d "DocsDir=$docs" `
            -d "LicenseRtf=$work\license.rtf" `
            -d "BannerBmp=$work\banner.bmp" -d "DialogBmp=$work\dialog.bmp" `
            -o $msi
    }
    # Windows Installer's consistency checks (ICE), as Intune and Orca run.
    Invoke-Step 'msi validation' { dotnet $wix.Wix msi validate $msi }
    Remove-Item -Force ([IO.Path]::ChangeExtension($msi, 'wixpdb')) -ErrorAction SilentlyContinue

    Write-Host '== zip'
    $stage = Join-Path $work "hangtime-$version-windows-x64"
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item $exe, "$docs\*" $stage
    Compress-Archive -Path $stage -DestinationPath $zip

    $lines = foreach ($file in $msi, $zip) {
        "$((Get-FileHash $file -Algorithm SHA256).Hash.ToLower())  $(Split-Path $file -Leaf)"
    }
    Set-Content -Path $sums -Value $lines -Encoding ascii

    Write-Host ""
    Write-Host "Built $msi"
    Write-Host "      $zip"
    $lines | ForEach-Object { Write-Host "  $_" }
} finally {
    Pop-Location
}
