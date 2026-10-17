# Installer lifecycle on Windows: install into the user profile, run the binaries, then
# uninstall and confirm nothing is left. Uses a throwaway LOCALAPPDATA so the real
# profile is never touched. No administrator rights needed.
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Sandbox = Join-Path $env:RUNNER_TEMP ("hs-pkg-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $Sandbox | Out-Null
$oldLocal = $env:LOCALAPPDATA
$env:LOCALAPPDATA = $Sandbox
try {
  Write-Host "== install"
  & (Join-Path $Root "install.ps1") -NoService
  $bin = Join-Path $Sandbox "Programs\sesync\bin"
  foreach ($b in @("sesync-lock.exe", "sesync-conv.exe", "sesync.exe")) {
    if (-not (Test-Path (Join-Path $bin $b))) { throw "missing $b" }
  }
  Write-Host "== binaries run"
  & (Join-Path $bin "sesync-lock.exe") list | Out-Null
  $env:SESYNC_DIR = Join-Path $Sandbox "state"
  $env:OMP_AGENT_DIR = Join-Path $Sandbox "omp"
  $env:PI_AGENT_DIR = Join-Path $Sandbox "pi"
  $env:CLAUDE_CONFIG_DIR = Join-Path $Sandbox "claude"
  $env:OPENCODE_DB = Join-Path $Sandbox "none.db"
  $env:SESYNC_CONV = Join-Path $bin "sesync-conv.exe"
  & (Join-Path $bin "sesync.exe") --status | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "--status failed" }
  Write-Host "== uninstall"
  & (Join-Path $Root "install.ps1") -Uninstall
  if (Test-Path (Join-Path $Sandbox "Programs\sesync")) { throw "install folder left behind" }
  Write-Host "== install/run/uninstall passed"
} finally {
  $env:LOCALAPPDATA = $oldLocal
  Remove-Item -Recurse -Force $Sandbox -ErrorAction SilentlyContinue
}
