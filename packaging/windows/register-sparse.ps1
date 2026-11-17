<#
.SYNOPSIS
  Registers the Fast Copy sparse package for the current user (enables the Windows 11
  top-level "Fast Copy" context menu). No admin rights needed, but the package signature must
  be trusted (a self-signed dev cert must be in Cert:\LocalMachine\TrustedPeople).

.PARAMETER InstallDir
  Folder containing fast-copy.exe and fc_shellext_win.dll (the "external location").
  Defaults to the folder of FastCopy.msix.
.PARAMETER Package
  Path to FastCopy.msix. Defaults to <InstallDir>\FastCopy.msix, else .\out\FastCopy.msix.
.PARAMETER RestartExplorer
  Restart explorer.exe so the new menu shows immediately (otherwise it appears for new
  Explorer windows / after sign-out).
#>
[CmdletBinding()]
param(
    [string]$InstallDir,
    [string]$Package,
    [switch]$RestartExplorer
)
$ErrorActionPreference = 'Stop'

if (-not $Package) {
    $candidates = @()
    if ($InstallDir) { $candidates += (Join-Path $InstallDir 'FastCopy.msix') }
    $candidates += (Join-Path $PSScriptRoot 'FastCopy.msix'), (Join-Path $PSScriptRoot 'out\FastCopy.msix')
    $Package = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $Package) { throw 'FastCopy.msix not found; pass -Package.' }
}
$Package = (Resolve-Path $Package).Path
if (-not $InstallDir) { $InstallDir = Split-Path $Package -Parent }
$InstallDir = (Resolve-Path $InstallDir).Path.TrimEnd('\')

foreach ($f in 'fast-copy.exe', 'fc_shellext_win.dll') {
    if (-not (Test-Path (Join-Path $InstallDir $f))) { throw "$f missing in $InstallDir" }
}

Add-AppxPackage -Path $Package -ExternalLocation $InstallDir -ForceUpdateFromAnyVersion
Get-AppxPackage -Name 'FastCopy.ShellExtension' | Select-Object Name, Version, PackageFullName, InstallLocation | Format-List

if ($RestartExplorer) {
    Stop-Process -Name explorer -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 800
    if (-not (Get-Process explorer -ErrorAction SilentlyContinue)) { Start-Process explorer.exe }
}
