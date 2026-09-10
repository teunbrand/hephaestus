# Build the Windows installer with the Explorer shell extensions in it.
#
# Two artifacts have to exist before the bundler runs, and neither is
# something Tauri knows about:
#
#   hephaestus_explorer.dll   the thumbnail and preview handlers
#   (the installer hook)      packaging/installer-hooks.nsh, which regsvr32s it
#
# `tauri.windows.conf.json` maps the DLL into the install directory and points
# NSIS at the hook. The DLL has to be built for the same architecture as the
# Explorer that will load it.
#
#   .\package-windows.ps1
#   .\package-windows.ps1 -Target aarch64-pc-windows-msvc
#
# UNTESTED. Nobody has run a Windows build; this is written from the config
# schema. See ../hephaestus-explorer/CLAUDE.md.

param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$Bundles = "nsis"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "==> building the shell extensions ($Target)"
Push-Location ../hephaestus-explorer
cargo build --release --target $Target
Pop-Location

# The config names the x86_64 path; anything else needs the mapping adjusted.
if ($Target -ne "x86_64-pc-windows-msvc") {
    Write-Warning "tauri.windows.conf.json points at the x86_64 DLL; update its resources mapping for $Target."
}

Write-Host "==> cargo tauri build --bundles $Bundles"
cargo tauri build --bundles $Bundles

Write-Host @"

==> after installing:

  The installer runs regsvr32 itself (see packaging/installer-hooks.nsh), but
  Explorer caches handlers hard. To see thumbnails and the preview pane:

    taskkill /f /im explorer.exe ; start explorer.exe
    # thumbnails additionally cache per file:
    del /q %LocalAppData%\Microsoft\Windows\Explorer\thumbcache_*.db

  Alt+P toggles the preview pane. If nothing appears, work through
  "What to check first" in ../hephaestus-explorer/CLAUDE.md.
"@
