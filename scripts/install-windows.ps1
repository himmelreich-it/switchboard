# Windows counterpart of `install-local`: build the unpacked app (no installer) and install it for this user, with a
# Start menu shortcut so it shows up in Start search and can be pinned to the taskbar. No admin rights.
# -Beta installs "Switchboard Beta" next to it, from this checkout (a worktree, say): same data in ~/.switchboard, own
# name, folder, shortcut and icon. Only one of the two runs at a time (electron/main.js).
param([switch]$Beta)
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$Name = if ($Beta) { "Switchboard Beta" } else { "Switchboard" }
$Dest = Join-Path $env:LOCALAPPDATA "Programs\$Name"
$Exe = Join-Path $Dest "$Name.exe"
$Shortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\$Name.lnk"

# Quit the installed copy first, which stops its running agents and terminals. Closing its window lets it stop its
# server (will-quit in electron/main.js); a kill would skip that. Matching the install path leaves other copies alone.
$running = @(Get-Process $Name -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Exe })
if ($running) {
  $running | Where-Object { $_.MainWindowHandle -ne 0 } | ForEach-Object { $_.CloseMainWindow() | Out-Null }
  $running | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
  $running | Where-Object { -not $_.HasExited } | Stop-Process -Force
}

# A fresh worktree has no node_modules yet; the app runs its server from the copied ones.
if (-not (Test-Path node_modules)) { bun install; if ($LASTEXITCODE) { exit $LASTEXITCODE } }
if ($Beta) {
  # The channel goes into the packaged package.json; the app reads it at start.
  bunx electron-builder --win --dir "-c.productName=$Name" "-c.appId=dev.himmelreich.switchboard.beta" `
    "-c.win.icon=images/beta-icons/flat/switchboard-beta.ico" "-c.extraMetadata.name=switchboard-beta" "-c.extraMetadata.productName=$Name" `
    "-c.extraMetadata.channel=beta"
} else {
  bun run package:win
}
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
Write-Host "Installed $Dest (Start menu: $Name)"
