# Windows counterpart of `install-local`: build the unpacked app (no installer) and install it for this user, with a
# Start menu shortcut so it shows up in Start search and can be pinned to the taskbar. No admin rights.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$Dest = Join-Path $env:LOCALAPPDATA "Programs\Switchboard"
$Exe = Join-Path $Dest "Switchboard.exe"
$Shortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Switchboard.lnk"

# Quit the installed copy first, which stops its running agents and terminals. Closing its window lets it stop its
# server (will-quit in electron/main.js); a kill would skip that. Matching the install path leaves other copies alone.
$running = @(Get-Process Switchboard -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Exe })
if ($running) {
  $running | Where-Object { $_.MainWindowHandle -ne 0 } | ForEach-Object { $_.CloseMainWindow() | Out-Null }
  $running | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
  $running | Where-Object { -not $_.HasExited } | Stop-Process -Force
}

bun run package:win
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$built = Get-ChildItem dist -Directory -Filter "win*-unpacked" | Select-Object -First 1
if (Test-Path $Dest) { Remove-Item $Dest -Recurse -Force }
New-Item -ItemType Directory -Force (Split-Path $Dest) | Out-Null
Move-Item $built.FullName $Dest
Remove-Item dist -Recurse -Force

$lnk = (New-Object -ComObject WScript.Shell).CreateShortcut($Shortcut)
$lnk.TargetPath = $Exe
$lnk.WorkingDirectory = $Dest
$lnk.Description = "Run TODOs as Claude Code sessions"
$lnk.Save()
Write-Host "Installed $Dest (Start menu: Switchboard)"
