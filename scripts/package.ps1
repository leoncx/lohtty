# Builds a release and packs it for distribution:
#   dist/lohtty-<version>-windows-<arch>.zip      exe, README, LICENSE, CHANGELOG
#   dist/lohtty-<version>-windows-<arch>-pdb.zip  debug symbols, for crash reports
#
#   pwsh -File scripts/package.ps1 [-Target aarch64-pc-windows-msvc]
# Prints the paths of the archives it made, one per line.
param(
    [string]$Target = 'x86_64-pc-windows-msvc'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$arch = switch -Wildcard ($Target) {
    'x86_64-*' { 'x64' }
    'aarch64-*' { 'arm64' }
    default { throw "unsupported target $Target" }
}

Push-Location $root
try {
    # --locked: build exactly the dependency versions in Cargo.lock.
    cargo build --release --locked -p lohtty --target $Target
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    $version = ((cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json).packages |
        Where-Object name -eq 'lohtty').version
    $name = "lohtty-$version-windows-$arch"
    $dist = Join-Path $root 'dist'
    $bin = Join-Path $root "target/$Target/release"

    $stage = Join-Path $dist $name
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item (Join-Path $bin 'lohtty.exe'), 'README.md', 'LICENSE', 'CHANGELOG.md' $stage
    $zip = Join-Path $dist "$name.zip"
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -Force

    # Symbols matching this exact build (the release profile keeps line tables).
    $pdb = Join-Path $bin 'lohtty.pdb'
    if (-not (Test-Path $pdb)) { throw "no PDB at $pdb" }
    $pdbZip = Join-Path $dist "$name-pdb.zip"
    Compress-Archive -Path $pdb -DestinationPath $pdbZip -Force

    Write-Output $zip
    Write-Output $pdbZip
}
finally {
    Pop-Location
}
