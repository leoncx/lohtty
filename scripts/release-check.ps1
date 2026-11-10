# Checks that a release tag matches the crate version and the changelog, and
# writes that version's changelog section as release notes.
#   pwsh -File scripts/release-check.ps1 -Tag v0.2.0 [-NotesOut notes.md]
# Exits non-zero (with the reason) when the tag can't be released.
param(
    [Parameter(Mandatory)] [string]$Tag,
    [string]$NotesOut
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

function Fail($message) {
    [Console]::Error.WriteLine("release-check: $message")
    exit 1
}

if ($Tag -notmatch '^v(\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?)$') {
    Fail "tag '$Tag' is not of the form vX.Y.Z"
}
$version = $Matches[1]

# The workspace version, read the same way cargo does.
Push-Location $root
try {
    $meta = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { Fail 'cargo metadata failed (is Cargo.lock up to date?)' }
} finally {
    Pop-Location
}
$crate = ($meta.packages | Where-Object name -eq 'lohtty').version
if ($crate -ne $version) {
    Fail "tag $Tag doesn't match the lohtty crate version $crate (Cargo.toml)"
}

# The section `## [X.Y.Z] ...` up to the next `## ` heading.
$lines = Get-Content (Join-Path $root 'CHANGELOG.md')
$start = -1
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match "^## \[$([regex]::Escape($version))\]") { $start = $i; break }
}
if ($start -lt 0) {
    Fail "CHANGELOG.md has no '## [$version]' section (move the Unreleased entries into one)"
}
$end = $lines.Count
for ($i = $start + 1; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match '^## ') { $end = $i; break }
}
$notes = ($lines[($start + 1)..($end - 1)] -join "`n").Trim()
if (-not $notes) { Fail "the changelog section for $version is empty" }

if ($NotesOut) {
    Set-Content -Path $NotesOut -Value $notes -Encoding utf8NoBOM
}
Write-Output "release-check: $Tag OK (lohtty $crate)"
