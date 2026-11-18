# Builds a release and packs it for distribution:
#   dist/lohtty-<version>-windows-<arch>.zip      exe and ConPTY, README, LICENSE,
#                                                 CHANGELOG, THIRD-PARTY-NOTICES
#   dist/lohtty-<version>-windows-<arch>-pdb.zip  debug symbols, for crash reports
#   dist/lohtty-<version>-windows-<arch>.msi      per-user installer (with -Msi)
#
#   pwsh -File scripts/package.ps1 [-Target aarch64-pc-windows-msvc] [-Msi]
#
# -Msi needs WiX v5: the `wix` .NET tool with the UI extension
#   dotnet tool install --global wix --version 5.0.2
#   wix extension add --global WixToolset.UI.wixext/5.0.2
# or, without the .NET SDK, the `wix` NuGet package unpacked somewhere, run
# through the .NET runtime: -WixDll <...>\tools\net6.0\any\wix.dll
# -WixUiExt <...>\wixext5\WixToolset.UI.wixext.dll
#
# Prints the paths of the files it made, one per line.
param(
    [string]$Target = 'x86_64-pc-windows-msvc',
    [switch]$Msi,
    [string]$WixDll,
    [string]$WixUiExt = 'WixToolset.UI.wixext'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$arch = switch -Wildcard ($Target) {
    'x86_64-*' { 'x64' }
    'aarch64-*' { 'arm64' }
    default { throw "unsupported target $Target" }
}

# The license as RTF, for the installer's license page.
function ConvertTo-Rtf([string]$text) {
    $escaped = $text.Replace('\', '\\').Replace('{', '\{').Replace('}', '\}')
    $body = ($escaped -split "`r?`n") -join "\par`n"
    "{\rtf1\ansi\deff0{\fonttbl{\f0\fmodern Consolas;}}\f0\fs16`n$body`n}"
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
    # Microsoft's ConPTY next to the exe (see conpty.ps1); the MSI takes it
    # from there too.
    & (Join-Path $PSScriptRoot 'conpty.ps1') -Arch $arch -Dest $bin

    $stage = Join-Path $dist $name
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    New-Item -ItemType Directory -Force $stage | Out-Null
    $binaries = 'lohtty.exe', 'conpty.dll', 'OpenConsole.exe' | ForEach-Object { Join-Path $bin $_ }
    Copy-Item ($binaries + 'README.md', 'LICENSE', 'CHANGELOG.md', 'THIRD-PARTY-NOTICES.md') $stage
    $zip = Join-Path $dist "$name.zip"
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -Force

    # Symbols matching this exact build (the release profile keeps line tables).
    $pdb = Join-Path $bin 'lohtty.pdb'
    if (-not (Test-Path $pdb)) { throw "no PDB at $pdb" }
    $pdbZip = Join-Path $dist "$name-pdb.zip"
    Compress-Archive -Path $pdb -DestinationPath $pdbZip -Force

    Write-Output $zip
    Write-Output $pdbZip

    if ($Msi) {
        # MSI versions are numeric: 0.2.0-rc.1 installs as 0.2.0.
        $msiVersion = ($version -split '-')[0]
        $rtf = Join-Path $dist 'LICENSE.rtf'
        Set-Content -Path $rtf -Value (ConvertTo-Rtf (Get-Content LICENSE -Raw)) -Encoding ascii
        $msiPath = Join-Path $dist "$name.msi"
        $wixArgs = @(
            'build', 'installer/lohtty.wxs',
            '-arch', $arch,
            '-ext', $WixUiExt,
            '-d', "Version=$msiVersion",
            '-d', "BinDir=$bin",
            '-d', "LicenseRtf=$rtf",
            '-o', $msiPath
        )
        if ($WixDll) {
            # The WiX 5 tool targets .NET 6; run it on whatever runtime is newer.
            $env:DOTNET_ROLL_FORWARD = 'Major'
            dotnet $WixDll @wixArgs
        } else {
            wix @wixArgs
        }
        if ($LASTEXITCODE -ne 0) { throw 'wix build failed' }
        # The .wixpdb (installer debug data) isn't shipped.
        Remove-Item $rtf, ([IO.Path]::ChangeExtension($msiPath, 'wixpdb')) -ErrorAction SilentlyContinue
        Write-Output $msiPath
    }
}
finally {
    Pop-Location
}
