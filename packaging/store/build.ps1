param(
    [Parameter(Mandatory = $true)][string]$IdentityName,
    [Parameter(Mandatory = $true)][string]$Publisher,
    [Parameter(Mandatory = $true)][string]$PublisherDisplayName,
    [string]$Version = "0.2.0.0"
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot\..\.."
$layout = Join-Path $root "target\store\layout"
$output = Join-Path $root "target\store\Quarz_${Version}_x64.msix"
$kit = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Recurse -Filter makeappx.exe | Where-Object { $_.FullName -match "\\x64\\" } | Sort-Object FullName | Select-Object -Last 1

Push-Location $root
cargo build --release --workspace
if ($LASTEXITCODE) { throw "Build fehlgeschlagen" }
Pop-Location

if (Test-Path $layout) { Remove-Item $layout -Recurse -Force }
New-Item -ItemType Directory -Force "$layout\Assets" | Out-Null
Copy-Item "$root\target\release\quarz.exe", "$root\target\release\qz.exe", "$root\target\release\quarz_shell.dll" $layout
Copy-Item "$root\LICENSE" "$layout\LICENSE.txt"
Copy-Item "$root\THIRD-PARTY-LICENSES.txt" $layout
Copy-Item "$PSScriptRoot\Assets\*" "$layout\Assets"

$manifest = Get-Content "$PSScriptRoot\AppxManifest.xml" -Raw -Encoding UTF8
$manifest = $manifest.Replace("{IDENTITY_NAME}", $IdentityName).Replace("{PUBLISHER}", $Publisher).Replace("{PUBLISHER_DISPLAY_NAME}", $PublisherDisplayName).Replace("{VERSION}", $Version)
[IO.File]::WriteAllText("$layout\AppxManifest.xml", $manifest, (New-Object Text.UTF8Encoding $false))

& $kit.FullName pack /d $layout /p $output /o
if ($LASTEXITCODE) { throw "Paket konnte nicht erstellt werden" }
Write-Host "Fertig: $output"
