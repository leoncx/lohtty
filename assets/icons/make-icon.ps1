# Generates the application icon, a ">_" prompt on a Tokyo Night tile:
#   lohtty.ico   Windows: multi-size ICO with PNG frames (Windows Vista and later)
#   lohtty.png   Linux: 256x256, for the .desktop entry and the AppImage
#   lohtty.icns  macOS: PNG entries from 16 to 1024 px, with the margin macOS
#                icons keep around the tile (the tile fills about 80%)
#   pwsh -File assets/icons/make-icon.ps1
Add-Type -AssemblyName System.Drawing

$sizes = 16, 24, 32, 48, 64, 128, 256
$out = Join-Path $PSScriptRoot 'lohtty.ico'

# A PNG of the icon, $n pixels square, with the tile filling $fill of it.
function New-Frame([int]$n, [double]$fill = 1.0) {
    if ($fill -lt 1.0) {
        $inner = [System.Drawing.Image]::FromStream([System.IO.MemoryStream]::new((New-Frame ([int][Math]::Round($n * $fill)))))
        $bmp = New-Object System.Drawing.Bitmap $n, $n
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.Clear([System.Drawing.Color]::Transparent)
        $at = [int](($n - $inner.Width) / 2)
        $g.DrawImage($inner, $at, $at, $inner.Width, $inner.Height)
        $ms = New-Object System.IO.MemoryStream
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose(); $inner.Dispose()
        return , $ms.ToArray()
    }
    $bmp = New-Object System.Drawing.Bitmap $n, $n
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)

    # Rounded tile.
    $r = [Math]::Max(2, $n * 0.2)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $w = $n - 1
    $path.AddArc(0, 0, $r * 2, $r * 2, 180, 90)
    $path.AddArc($w - $r * 2, 0, $r * 2, $r * 2, 270, 90)
    $path.AddArc($w - $r * 2, $w - $r * 2, $r * 2, $r * 2, 0, 90)
    $path.AddArc(0, $w - $r * 2, $r * 2, $r * 2, 90, 90)
    $path.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0x1a, 0x1b, 0x26))), $path)
    $g.DrawPath((New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(0x41, 0x48, 0x68)), ([Math]::Max(1, $n / 32))), $path)

    # ">" chevron (blue) and "_" cursor (magenta).
    $stroke = [Math]::Max(1.5, $n * 0.09)
    $blue = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(0x7a, 0xa2, 0xf7)), $stroke
    $blue.StartCap = $blue.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $blue.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $g.DrawLines($blue, [System.Drawing.PointF[]]@(
        (New-Object System.Drawing.PointF ($n * 0.24), ($n * 0.30)),
        (New-Object System.Drawing.PointF ($n * 0.46), ($n * 0.50)),
        (New-Object System.Drawing.PointF ($n * 0.24), ($n * 0.70))))
    $magenta = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(0xbb, 0x9a, 0xf7)), $stroke
    $magenta.StartCap = $magenta.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $g.DrawLine($magenta, ($n * 0.54), ($n * 0.70), ($n * 0.78), ($n * 0.70))

    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    return , $ms.ToArray()
}

$frames = foreach ($n in $sizes) { , (New-Frame $n) }

# ICONDIR + ICONDIRENTRY[] + PNG payloads.
$fs = [System.IO.File]::Create($out)
$bw = New-Object System.IO.BinaryWriter $fs
$bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $n = $sizes[$i]; $data = $frames[$i]
    $bw.Write([byte]($n % 256)); $bw.Write([byte]($n % 256))   # 256 is stored as 0
    $bw.Write([byte]0); $bw.Write([byte]0)                     # palette, reserved
    $bw.Write([UInt16]1); $bw.Write([UInt16]32)                # planes, bpp
    $bw.Write([UInt32]$data.Length); $bw.Write([UInt32]$offset)
    $offset += $data.Length
}
foreach ($data in $frames) { $bw.Write($data) }
$bw.Close()
"wrote $out ($((Get-Item $out).Length) bytes)"

$png = Join-Path $PSScriptRoot 'lohtty.png'
[System.IO.File]::WriteAllBytes($png, (New-Frame 256))
"wrote $png ($((Get-Item $png).Length) bytes)"

# ICNS: 'icns' + total length, then (type, length incl. 8-byte header, PNG)
# per entry. All integers are big-endian.
function Write-BE([System.IO.BinaryWriter]$w, [UInt32]$v) {
    $b = [BitConverter]::GetBytes($v); [Array]::Reverse($b); $w.Write($b)
}
$entries = [ordered]@{
    icp4 = 16; icp5 = 32; ic11 = 32; icp6 = 64; ic12 = 64; ic07 = 128
    ic08 = 256; ic13 = 256; ic09 = 512; ic14 = 512; ic10 = 1024
}
$icns = Join-Path $PSScriptRoot 'lohtty.icns'
$data = foreach ($e in $entries.GetEnumerator()) { , @($e.Key, (New-Frame $e.Value 0.8)) }
$fs = [System.IO.File]::Create($icns)
$bw = New-Object System.IO.BinaryWriter $fs
$bw.Write([Text.Encoding]::ASCII.GetBytes('icns'))
Write-BE $bw (8 + ($data | ForEach-Object { 8 + $_[1].Length } | Measure-Object -Sum).Sum)
foreach ($d in $data) {
    $bw.Write([Text.Encoding]::ASCII.GetBytes($d[0]))
    Write-BE $bw (8 + $d[1].Length)
    $bw.Write($d[1])
}
$bw.Close()
"wrote $icns ($((Get-Item $icns).Length) bytes)"
