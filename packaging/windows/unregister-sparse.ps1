<#
.SYNOPSIS
  Removes the Fast Copy sparse package for the current user (the Windows 11 top-level menu
  disappears; the classic "Show more options" verbs are not touched).
.PARAMETER AllUsers
  Remove it for every user (admin). Normally not needed: the package is registered per user.
#>
[CmdletBinding()]
param([switch]$AllUsers)
$ErrorActionPreference = 'Stop'

$pkgs = if ($AllUsers) { Get-AppxPackage -AllUsers -Name 'FastCopy.ShellExtension' } else { Get-AppxPackage -Name 'FastCopy.ShellExtension' }
if (-not $pkgs) { Write-Host 'FastCopy.ShellExtension is not registered.'; return }
foreach ($p in $pkgs) {
    if ($AllUsers) { Remove-AppxPackage -Package $p.PackageFullName -AllUsers } else { Remove-AppxPackage -Package $p.PackageFullName }
    Write-Host "Removed $($p.PackageFullName)"
}
# Explorer keeps fc_shellext_win.dll loaded until it restarts; the file can be deleted afterwards.
