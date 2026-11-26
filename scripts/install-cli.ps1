[CmdletBinding()]
param(
    [string]$InstallDirectory
)

# Missions, Atlas and Beacon are native in the Luuna terminal client (ADR 0066):
# every component command is a launcher for `luuna <component>`. No Bun and no
# sibling component checkouts are needed.
$ErrorActionPreference = 'Stop'
if (!$InstallDirectory) { $InstallDirectory = Join-Path $env:LOCALAPPDATA 'Luuna\bin' }

New-Item -ItemType Directory -Force -Path $InstallDirectory | Out-Null
$launcher = (Join-Path $PSScriptRoot 'start-terminal.ps1').Replace('"', '""')
$launchers = @{
    'luuna.cmd' = "@echo off`r`npowershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$launcher`" %*`r`n"
    'lu.cmd' = "@echo off`r`n`"%~dp0luuna.cmd`" %*`r`n"
    'luuna-atlas.cmd' = "@echo off`r`n`"%~dp0luuna.cmd`" atlas %*`r`n"
    'luuna-beacon.cmd' = "@echo off`r`n`"%~dp0luuna.cmd`" beacon %*`r`n"
    'luuna-missions.cmd' = "@echo off`r`n`"%~dp0luuna.cmd`" missions %*`r`n"
    'lat.cmd' = "@echo off`r`n`"%~dp0luuna.cmd`" atlas %*`r`n"
    'lbc.cmd' = "@echo off`r`n`"%~dp0luuna.cmd`" beacon %*`r`n"
    'lms.cmd' = "@echo off`r`n`"%~dp0luuna.cmd`" missions %*`r`n"
}
foreach ($name in $launchers.Keys) {
    Set-Content -LiteralPath (Join-Path $InstallDirectory $name) -Value $launchers[$name] -Encoding Ascii
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$pathEntries = @($userPath -split ';' | Where-Object { $_ })
if ($pathEntries -notcontains $InstallDirectory) {
    [Environment]::SetEnvironmentVariable('Path', (@($pathEntries) + $InstallDirectory) -join ';', 'User')
}
if (($env:Path -split ';') -notcontains $InstallDirectory) { $env:Path = "$InstallDirectory;$env:Path" }

Write-Output "Installed Luuna CLI launchers to $InstallDirectory"
Write-Output 'Commands: luuna, lu, luuna-atlas/lat, luuna-beacon/lbc, luuna-missions/lms'
Write-Output 'Open a new terminal to use the updated user PATH.'
