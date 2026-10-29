# Builds the release exe and the installer: dist\Forge-Setup-<version>.exe
# Needs Inno Setup 6 (winget install JRSoftware.InnoSetup).
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

cargo build --release -p forge-app
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$iscc = @(
  "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
  "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
  "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found. Run: winget install JRSoftware.InnoSetup" }

& $iscc "$PSScriptRoot\forge.iss"
if ($LASTEXITCODE -ne 0) { throw "installer build failed" }
Get-ChildItem "$root\dist\Forge-Setup-*.exe" | Select-Object Name, @{n="MB";e={[math]::Round($_.Length/1MB,1)}}
