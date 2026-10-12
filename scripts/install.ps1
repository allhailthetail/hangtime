<#
.SYNOPSIS
    Installs Hangtime for the current user.

.DESCRIPTION
    Copies hangtime.exe to %LOCALAPPDATA%\Programs\Hangtime, adds that
    folder to your PATH, and adds Hangtime to the Start menu. No admin
    rights needed.

    Run it as administrator to also let Hangtime through Windows Firewall
    on private and domain networks. Otherwise Windows asks the first time
    Hangtime runs; choose "Private networks".

    Settings and your identity live in %USERPROFILE%\.config\hangtime and
    are created on first run. Installing over an older version keeps them.

.PARAMETER Uninstall
    Removes the program, PATH entry, Start menu shortcut, and firewall
    rule. Keeps your settings and identity.
#>
param([switch]$Uninstall)

$ErrorActionPreference = 'Stop'

$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\Hangtime'
$Exe = Join-Path $InstallDir 'hangtime.exe'
$Shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Hangtime.lnk'
$RuleName = 'Hangtime'

function Test-Admin {
    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# Edits the user PATH in the registry directly, so entries like
# %USERPROFILE%\bin stay unexpanded.
function Update-UserPath([bool]$Include) {
    $key = Get-Item 'HKCU:\Environment'
    $current = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    $parts = @($current -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ne $InstallDir })
    if ($Include) { $parts += $InstallDir }
    $updated = $parts -join ';'
    if ($updated -ne $current) {
        Set-ItemProperty -Path 'HKCU:\Environment' -Name Path -Value $updated -Type ExpandString
        # Tell Explorer the environment changed, so new terminals see it.
        [Environment]::SetEnvironmentVariable('HANGTIME_INSTALLER', '1', 'User')
        [Environment]::SetEnvironmentVariable('HANGTIME_INSTALLER', $null, 'User')
    }
}

function Assert-NotRunning {
    $running = Get-Process -Name hangtime -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $Exe }
    if ($running) {
        throw 'Hangtime is running. Quit it (press Q) and run this again.'
    }
}

if ($Uninstall) {
    Assert-NotRunning
    Remove-Item -Recurse -Force $InstallDir -ErrorAction SilentlyContinue
    Remove-Item -Force $Shortcut -ErrorAction SilentlyContinue
    Update-UserPath $false
    if (Test-Admin) {
        Remove-NetFirewallRule -DisplayName $RuleName -ErrorAction SilentlyContinue
    }
    Write-Host 'Hangtime removed. Your settings are still in' (Join-Path $HOME '.config\hangtime')
    exit 0
}

$Source = Join-Path $PSScriptRoot 'hangtime.exe'
if (-not (Test-Path $Source)) {
    throw "hangtime.exe not found next to this script ($PSScriptRoot)."
}

Assert-NotRunning
New-Item -ItemType Directory -Force $InstallDir | Out-Null
Copy-Item -Force $Source $Exe
# Files from a downloaded zip are marked as from the internet.
Unblock-File $Exe

Update-UserPath $true

$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($Shortcut)
$link.TargetPath = $Exe
$link.WorkingDirectory = $HOME
$link.Description = 'Hangtime push-to-talk radio'
$link.Save()

if (Test-Admin) {
    Remove-NetFirewallRule -DisplayName $RuleName -ErrorAction SilentlyContinue
    New-NetFirewallRule -DisplayName $RuleName -Direction Inbound -Program $Exe `
        -Protocol UDP -Action Allow -Profile Domain, Private | Out-Null
    $firewall = 'Firewall: allowed on private and domain networks.'
} else {
    $firewall = 'Firewall: Windows will ask on first run; choose "Private networks".'
}

$version = & $Exe --version
Write-Host ""
Write-Host "Installed $version to $InstallDir"
Write-Host $firewall
Write-Host ""
Write-Host 'Start it from the Start menu (Hangtime), or open a new terminal and run:'
Write-Host '    hangtime'
Write-Host 'Pick the last talkgroup, Parrot, and hold SPACE to hear yourself back.'
