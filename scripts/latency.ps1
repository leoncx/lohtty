# Keystroke-to-screen latency, Typometer style.
# Launches a terminal, types 'W' / Backspace pairs with real key events, and
# polls a few screen pixels where the character appears. Keep the mouse and
# keyboard still while it runs: it aborts if the terminal loses focus.
# Examples:
#   pwsh -File scripts/latency.ps1 -Name lohtty -Exe target/release/lohtty.exe -ProcName lohtty -OutDir $env:TEMP
#   pwsh -File scripts/latency.ps1 -Name wt -Exe wt.exe -ArgList '-w new' -ProcName WindowsTerminal -OutDir $env:TEMP
param(
    [string]$Name,          # label
    [string]$Exe,           # program to launch
    [string]$ArgList = '',  # its arguments
    [string]$ProcName,      # process that owns the window (WindowsTerminal for wt.exe)
    [int]$Samples = 60,
    [string]$OutDir
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing, System.Drawing.Common, System.Drawing.Primitives @"
using System; using System.Drawing; using System.Runtime.InteropServices; using System.Diagnostics;
public static class L {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
  [DllImport("gdi32.dll")] public static extern IntPtr CreateCompatibleDC(IntPtr dc);
  [DllImport("gdi32.dll")] public static extern IntPtr CreateCompatibleBitmap(IntPtr dc, int w, int h);
  [DllImport("gdi32.dll")] public static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
  [DllImport("gdi32.dll")] public static extern bool BitBlt(IntPtr d, int x, int y, int w, int h, IntPtr s, int sx, int sy, uint rop);
  [DllImport("gdi32.dll")] public static extern int GetBitmapBits(IntPtr bmp, int n, byte[] bits);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  public struct RECT { public int L, T, R, B; }

  static IntPtr screen, mem, bmp; static int W, H;
  public static void Init(int w, int h) {
    W = w; H = h; screen = GetDC(IntPtr.Zero); mem = CreateCompatibleDC(screen);
    bmp = CreateCompatibleBitmap(screen, w, h); SelectObject(mem, bmp);
  }
  public static byte[] Grab(int x, int y) {
    BitBlt(mem, 0, 0, W, H, screen, x, y, 0x00CC0020);
    var b = new byte[W * H * 4]; GetBitmapBits(bmp, b.Length, b); return b;
  }
  public static int Diff(byte[] a, byte[] b) {
    int n = 0; for (int i = 0; i < a.Length; i += 4)
      if (Math.Abs(a[i]-b[i]) + Math.Abs(a[i+1]-b[i+1]) + Math.Abs(a[i+2]-b[i+2]) > 60) n++;
    return n;
  }
  public static void Key(byte vk) { keybd_event(vk, 0, 0, IntPtr.Zero); keybd_event(vk, 0, 2, IntPtr.Zero); }
  // Sends vk, then polls (x,y) until at least minPx pixels differ from the
  // baseline. Returns ms, or -1 after timeoutMs. polls = number of grabs.
  public static double Measure(byte vk, int x, int y, int minPx, int timeoutMs, out int polls) {
    var baseline = Grab(x, y); polls = 0;
    var sw = Stopwatch.StartNew(); Key(vk);
    while (sw.ElapsedMilliseconds < timeoutMs) {
      var now = Grab(x, y); polls++;
      if (Diff(baseline, now) >= minPx) return sw.Elapsed.TotalMilliseconds;
    }
    return -1;
  }
  // Bounding box of pixels that differ between two full-window grabs.
  public static int[] BBox(Bitmap a, Bitmap b) {
    int x0 = int.MaxValue, y0 = int.MaxValue, x1 = -1, y1 = -1;
    for (int y = 0; y < a.Height; y++) for (int x = 0; x < a.Width; x++) {
      Color p = a.GetPixel(x, y), q = b.GetPixel(x, y);
      if (Math.Abs(p.R-q.R) + Math.Abs(p.G-q.G) + Math.Abs(p.B-q.B) > 60) {
        if (x < x0) x0 = x; if (y < y0) y0 = y; if (x > x1) x1 = x; if (y > y1) y1 = y; } }
    return new[] { x0, y0, x1, y1 };
  }
}
"@
[L]::SetProcessDPIAware() | Out-Null
Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue

function Shot($r) {
    $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
    $g = [System.Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size); $g.Dispose(); $bmp
}
function FindWindow {
    $h = [L]::GetForegroundWindow(); $procId = 0
    [L]::GetWindowThreadProcessId($h, [ref]$procId) | Out-Null
    $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
    if ($p -and $p.ProcessName -eq $ProcName) { return @{ h = $h; p = $p } }
    return $null
}
function Guard($w) {
    $r = New-Object L+RECT; [L]::GetWindowRect($w.h, [ref]$r) | Out-Null
    if (($r.R - $r.L) -lt 300 -or ($r.B - $r.T) -lt 200) { throw "ABORT: implausible window" }
    if ([L]::GetForegroundWindow() -ne $w.h) { throw "ABORT: $Name not foreground" }
    $r
}

$before = @(Get-Process -Name $ProcName -ErrorAction SilentlyContinue | ForEach-Object Id)
if ($ArgList) { Start-Process -FilePath $Exe -ArgumentList $ArgList | Out-Null } else { Start-Process -FilePath $Exe -RedirectStandardError (Join-Path $OutDir "$Name-stderr.txt") | Out-Null }
$w = $null
for ($i = 0; $i -lt 60 -and -not $w; $i++) { Start-Sleep -Milliseconds 250; $w = FindWindow
    if ($w -and $before -contains $w.p.Id -and $ProcName -ne 'WindowsTerminal') { $w = $null } }
if (-not $w) { throw "ABORT: no new $ProcName window in the foreground" }
try {
    Start-Sleep -Seconds 4   # shell + prompt
    $r = Guard $w
    # Calibrate: where does a typed W appear?
    $a = Shot $r; [L]::Key(0x57); Start-Sleep -Milliseconds 700; $b = Shot $r
    $box = [L]::BBox($a, $b); $b.Save((Join-Path $OutDir "$Name-calib.png")); $a.Dispose(); $b.Dispose()
    [L]::Key(0x08); Start-Sleep -Milliseconds 700
    if ($box[2] -lt 0) { throw "ABORT: typing W changed nothing" }
    $bw = $box[2] - $box[0] + 1; $bh = $box[3] - $box[1] + 1
    Write-Output ("{0}: change box {1}x{2} at ({3},{4})" -f $Name, $bw, $bh, $box[0], $box[1])
    # Watch the left part of the box: the W glyph (not the cursor moving away).
    $gw = [Math]::Max(4, [int]($bw / 2)); $gx = $r.L + $box[0]; $gy = $r.T + $box[1]
    [L]::Init($gw, $bh)
    $down = @(); $polls = @()
    $rand = New-Object System.Random 7
    for ($i = 0; $i -lt $Samples; $i++) {
        Guard $w | Out-Null
        $n = 0
        $ms = [L]::Measure(0x57, $gx, $gy, 6, 1000, [ref]$n)
        if ($ms -ge 0) { $down += $ms; $polls += $n }
        Start-Sleep -Milliseconds 150
        [L]::Key(0x08)
        Start-Sleep -Milliseconds (250 + $rand.Next(0, 60))
    }
    $s = $down | Sort-Object
    Write-Output ("{0} samples: {1}" -f $Name, (($s | ForEach-Object { '{0:N1}' -f $_ }) -join ' '))
    if ($s.Count -eq 0) { throw "no samples" }
    $pct = { param($q) $s[[Math]::Min($s.Count - 1, [int]([Math]::Floor($q * $s.Count)))] }
    $avgPoll = ($down | Measure-Object -Sum).Sum / (($polls | Measure-Object -Sum).Sum)
    Write-Output ("{0}: n={1} min={2:N1} median={3:N1} p90={4:N1} max={5:N1} ms  (poll every {6:N2} ms)" -f `
        $Name, $s.Count, $s[0], (& $pct 0.5), (& $pct 0.9), $s[-1], $avgPoll)
} finally {
    if ($ProcName -eq 'WindowsTerminal') {
        # Close only the window we opened (Alt+F4 while it's foreground).
        if ([L]::GetForegroundWindow() -eq $w.h) { [System.Windows.Forms.SendKeys] | Out-Null
            Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('%{F4}') }
    } elseif (-not $w.p.HasExited) { $w.p.Kill() }
}
