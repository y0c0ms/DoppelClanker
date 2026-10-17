# sesync per-user installer for Windows. No administrator rights are needed.
#
#   powershell -ExecutionPolicy Bypass -File install.ps1               install
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall    remove
#   powershell -ExecutionPolicy Bypass -File install.ps1 -NoService    install, no login start
#
# Writes only under the user profile and HKCU:
#   %LOCALAPPDATA%\Programs\sesync\bin   the three .exe files
#   %LOCALAPPDATA%\Programs\sesync\share hook files and MANIFEST.txt
#   HKCU\Software\Microsoft\Windows\CurrentVersion\Run   login start (sesync)
#   user PATH (HKCU\Environment)                          adds the bin folder
param(
  [switch]$Uninstall,
  [switch]$NoService
)
$ErrorActionPreference = "Stop"

$Root    = Split-Path -Parent $MyInvocation.MyCommand.Path
$Prefix  = Join-Path $env:LOCALAPPDATA "Programs\sesync"
$Bin     = Join-Path $Prefix "bin"
$Share   = Join-Path $Prefix "share"
$Manifest = Join-Path $Share "MANIFEST.txt"
$RunKey  = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$EnvKey  = "HKCU:\Environment"

function Add-UserPath([string]$dir) {
  $current = (Get-ItemProperty -Path $EnvKey -Name Path -ErrorAction SilentlyContinue).Path
  if (-not $current) { $current = "" }
  $parts = $current -split ";" | Where-Object { $_ -ne "" }
  if ($parts -notcontains $dir) {
    $new = (@($parts) + $dir) -join ";"
    Set-ItemProperty -Path $EnvKey -Name Path -Value $new
  }
}

function Remove-UserPath([string]$dir) {
  $current = (Get-ItemProperty -Path $EnvKey -Name Path -ErrorAction SilentlyContinue).Path
  if (-not $current) { return }
  $parts = $current -split ";" | Where-Object { $_ -ne "" -and $_ -ne $dir }
  Set-ItemProperty -Path $EnvKey -Name Path -Value ($parts -join ";")
}

if ($Uninstall) {
  if (Test-Path $Manifest) {
    Get-Content $Manifest | Where-Object { $_ } | ForEach-Object {
      if (Test-Path $_) { Remove-Item -Force $_ }
    }
  }
  Remove-ItemProperty -Path $RunKey -Name "sesync" -ErrorAction SilentlyContinue
  Remove-UserPath $Bin
  if (Test-Path $Prefix) { Remove-Item -Recurse -Force $Prefix }
  Write-Host "removed sesync from $Prefix"
  exit 0
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  throw "cargo is required (install Rust stable, per-user, from rustup.rs)"
}

Write-Host "== building"
Push-Location (Join-Path $Root "lock");      cargo build --release --quiet; Pop-Location
Push-Location (Join-Path $Root "sesync-conv");    cargo build --release --quiet; Pop-Location
Push-Location (Join-Path $Root "daemon-rs"); cargo build --release --quiet; Pop-Location

New-Item -ItemType Directory -Force -Path $Bin, (Join-Path $Share "hooks") | Out-Null
$recorded = @()
function Install-File([string]$src, [string]$dst) {
  Copy-Item -Force $src $dst
  $script:recorded += $dst
}

Write-Host "== installing to $Prefix"
Install-File (Join-Path $Root "sesync-lock\target\release\sesync-lock.exe")          (Join-Path $Bin "sesync-lock.exe")
Install-File (Join-Path $Root "sesync-conv\target\release\sesync-conv.exe")        (Join-Path $Bin "sesync-conv.exe")
Install-File (Join-Path $Root "sesync\target\release\sesync.exe") (Join-Path $Bin "sesync.exe")
Get-ChildItem (Join-Path $Root "hooks") -File | ForEach-Object {
  Install-File $_.FullName (Join-Path $Share "hooks\$($_.Name)")
}

Add-UserPath $Bin

if (-not $NoService) {
  # Login start through the per-user Run key. No scheduled task, no service, no admin.
  $cmd = "`"$(Join-Path $Bin 'sesync.exe')`" --watch"
  Set-ItemProperty -Path $RunKey -Name "sesync" -Value $cmd
  Write-Host "login start registered (HKCU Run)"
}

$recorded | Set-Content -Path $Manifest
Write-Host "== installed. Run: $(Join-Path $Bin 'sesync.exe') --once"
