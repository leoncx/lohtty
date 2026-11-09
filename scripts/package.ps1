# Builds a release and packs a portable zip: dist/lohtty-<version>-windows-x64.zip
#   pwsh -File scripts/package.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    cargo build --release -p lohtty
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    $version = ((cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages |
        Where-Object name -eq 'lohtty').version
    $name = "lohtty-$version-windows-x64"
    $dist = Join-Path $root 'dist'
    $stage = Join-Path $dist $name

    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item (Join-Path $root 'target/release/lohtty.exe'), (Join-Path $root 'README.md') $stage

    $zip = Join-Path $dist "$name.zip"
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -Force
    Write-Output $zip
}
finally {
    Pop-Location
}
