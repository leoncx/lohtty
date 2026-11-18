# Puts Microsoft's ConPTY (conpty.dll and OpenConsole.exe) next to lohtty.exe.
#
# lohtty's PTY library loads a conpty.dll found next to the exe before the
# system's, and that DLL starts the OpenConsole.exe beside it. Windows' own
# ConPTY may strip sequences it doesn't know, such as the kitty graphics
# protocol's APC, and update on its own schedule; the packaged one passes
# them through and stays the same until we bump the version here.
#
#   pwsh -File scripts/conpty.ps1 [-Arch x64|arm64] -Dest target/debug[,target/release]
#
# The package (Microsoft.Windows.Console.ConPTY on nuget.org, MIT) is
# downloaded once into target/conpty, checked against the pinned SHA-256, and
# its binaries must carry a valid Microsoft Authenticode signature.
param(
    [ValidateSet('x64', 'arm64')]
    [string]$Arch = 'x64',
    [Parameter(Mandatory)]
    [string[]]$Dest
)
$ErrorActionPreference = 'Stop'
$Version = '1.25.260930003'
$Sha256 = '02B07B349AF66D801159BDF9E440D4A1CE78BB951F37FC8609731665AFDAE7EE'

$root = Split-Path -Parent $PSScriptRoot
$cache = Join-Path $root "target/conpty/$Version"
$package = Join-Path $cache 'package.nupkg'
$unpacked = Join-Path $cache 'x'

if (-not (Test-Path $package)) {
    New-Item -ItemType Directory -Force $cache | Out-Null
    $url = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/$Version/microsoft.windows.console.conpty.$Version.nupkg"
    Invoke-WebRequest -Uri $url -OutFile "$package.part"
    Move-Item -Force "$package.part" $package
}
$actual = (Get-FileHash $package -Algorithm SHA256).Hash
if ($actual -ne $Sha256) {
    Remove-Item $package
    throw "ConPTY package hash mismatch: got $actual, expected $Sha256"
}
if (-not (Test-Path $unpacked)) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($package, $unpacked)
}

$files = @(
    (Join-Path $unpacked "runtimes/win-$Arch/native/conpty.dll"),
    (Join-Path $unpacked "build/native/runtimes/$Arch/OpenConsole.exe")
)
foreach ($file in $files) {
    $signature = Get-AuthenticodeSignature $file
    if ($signature.Status -ne 'Valid' -or
        $signature.SignerCertificate.Subject -notmatch '^CN=Microsoft Corporation,') {
        throw "$file isn't signed by Microsoft ($($signature.Status))"
    }
}
foreach ($dir in $Dest) {
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item $files $dir -Force
}
