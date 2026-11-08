$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
$cargoPath = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $cargoPath) { $cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
& $cargoPath build --release
if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'target\release\muisland.exe') -Destination (Join-Path $PSScriptRoot 'Muisland.exe') -Force
Start-Process -FilePath (Join-Path $PSScriptRoot 'Muisland.exe')
