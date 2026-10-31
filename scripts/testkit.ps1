# Helpers shared by the PowerShell regression tests.
#
#   . "$PSScriptRoot\testkit.ps1"
#
# Everything here is about talking to the real window manager and to the app's
# automation hooks, and about producing throwaway windows to operate on. A test
# that uses these never touches the user's applications or the real
# %LOCALAPPDATA%\ShaiWin.

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class W {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lp);
  public delegate bool EnumProc(IntPtr h, IntPtr lp);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
  [DllImport("user32.dll")] public static extern bool SystemParametersInfoW(uint action, uint param, out RECT rect, uint winIni);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool GetWindowPlacement(IntPtr h, ref WINDOWPLACEMENT wp);
  [DllImport("user32.dll")] public static extern bool SetWindowPlacement(IntPtr h, ref WINDOWPLACEMENT wp);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr ctx);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtrW(IntPtr h, int index);
  [DllImport("user32.dll", EntryPoint="GetWindowLongW")] static extern int GetWindowLongW(IntPtr h, int index);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out int value, int size);

  // DWM paints nothing for a cloaked window, even when its style and rectangle
  // say it is on screen. The input host's CoreWindow is the usual one, and it
  // spans the whole desktop in some sessions.
  public static bool IsCloaked(IntPtr h) {
    int v = 0;
    return DwmGetWindowAttribute(h, 14, out v, 4) == 0 && v != 0;   // DWMWA_CLOAKED
  }

  // The overlay's promise to itself is that it only holds the top of the screen
  // while it is being used, so the tests have to be able to see who is on top.
  public static bool IsTopmost(IntPtr h) {
    long ex = (IntPtr.Size == 8)
      ? GetWindowLongPtrW(h, -20).ToInt64()          // GWL_EXSTYLE
      : (long)GetWindowLongW(h, -20);
    return (ex & 0x8) != 0;                          // WS_EX_TOPMOST
  }
  // Whether the layer lets clicks through. A demoted line still has to take a
  // click on a card the user can see; WS_EX_TRANSPARENT is what decides that.
  public static bool IsClickThrough(IntPtr h) {
    long ex = (IntPtr.Size == 8)
      ? GetWindowLongPtrW(h, -20).ToInt64()
      : (long)GetWindowLongW(h, -20);
    return (ex & 0x20) != 0;                         // WS_EX_TRANSPARENT
  }
  // Is `a` painted over `b`? Walking the z-order from `b` upwards is the only
  // way to ask: the style bit alone does not answer it, because clearing
  // WS_EX_TOPMOST puts a window at the head of the normal band - still above the
  // window the user just clicked into, which is the whole complaint.
  public static bool IsAbove(IntPtr a, IntPtr b) {
    IntPtr cur = GetWindow(b, 3);                    // GW_HWNDPREV = the one above
    for (int i = 0; i < 4096 && cur != IntPtr.Zero; i++) {
      if (cur == a) return true;
      cur = GetWindow(cur, 3);
    }
    return false;
  }
  public struct RECT { public int Left, Top, Right, Bottom; }
  public struct POINT { public int X, Y; }
  public struct WINDOWPLACEMENT {
    public int length, flags, showCmd;
    public POINT ptMinPosition, ptMaxPosition;
    public RECT rcNormalPosition;
  }
}
"@

# The app works in physical pixels; so must the test, or its coordinates are
# off by the scale factor. Old Windows refuse this call and just stay unaware,
# which the tests notice and work around.
[void][W]::SetThreadDpiAwarenessContext([IntPtr](-4))

$script:KitHang = 0x8000 + 10
$script:KitTakeAll = 0x8000 + 11
$script:KitDumpFrame = 0x8000 + 12
$script:KitTakeOne = 0x8000 + 13
$script:KitUnhook = 0x8000 + 14
# WM_APP + 17: answer pointer questions from (wParam, lParam) instead of the
# real cursor. Sessions that refuse SetCursorPos use this to drive the
# z-order rule; wParam 0xFFFFFFFF puts the real pointer back.
$script:KitPointAt = 0x8000 + 17
# WM_APP + 18: pretend a window of the user's is being dragged. A headless
# session cannot start a real move loop; wParam 0 ends the pretend drag.
$script:KitPretendDrag = 0x8000 + 18
$script:KitHotkey = 0x0312
$script:KitClose = 0x0010
$script:KitMouseMove = 0x0200
$script:KitMouseDown = 0x0201
$script:KitMouseUp = 0x0202

function Fail([string]$message) { throw "FAIL: $message" }

function Invoke-Wm([IntPtr]$hwnd, [uint32]$msg, [IntPtr]$wp, [IntPtr]$lp) {
  [void][W]::PostMessage($hwnd, $msg, $wp, $lp)
}

function Find-By-Class([string]$want) {
  $cb = [W+EnumProc]{
    param($h, $lp)
    $sb = New-Object System.Text.StringBuilder 256
    [void][W]::GetClassNameW($h, $sb, 256)
    if ($sb.ToString() -eq $want) { $script:found = $h; return $false }
    return $true
  }
  $script:found = [IntPtr]::Zero
  [void][W]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

function Find-By-Title([string]$want) {
  $cb = [W+EnumProc]{
    param($h, $lp)
    $sb = New-Object System.Text.StringBuilder 256
    [void][W]::GetWindowTextW($h, $sb, 256)
    if ($sb.ToString() -eq $want -and [W]::IsWindowVisible($h)) { $script:found = $h; return $false }
    return $true
  }
  $script:found = [IntPtr]::Zero
  [void][W]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

# A handle is never true or false to PowerShell on its own: IntPtr.Zero is
# truthy, so every comparison against it has to be spelled out.
function Test-Window([IntPtr]$h) { return $h -ne [IntPtr]::Zero }

function Get-Overlay { Find-By-Class "ShaiWin.Line" }

# The rope's own always-on-top layer, shown only while the cards have been
# demoted behind the user's work.
function Get-Rope { Find-By-Class "ShaiWin.Rope" }

# The app's design sizes are logical pixels; at 150% the rope's grab band and
# the gaps around it are 1.5x as wide on screen. Tests that reason about those
# distances have to multiply by this.
function Get-WindowScale([IntPtr]$h) {
  $dpi = [W]::GetDpiForWindow($h)
  if ($dpi -le 0) { return 1.0 }
  return $dpi / 96.0
}

function Get-Rect([IntPtr]$h) {
  $r = New-Object W+RECT
  if (-not [W]::GetWindowRect($h, [ref]$r)) { return $null }
  return $r
}

# The class of the window painted over `(x, y)` that is above `$below`, or "" if
# that point is bare. This is the app's own "is the card really visible" walk:
# visible, not click-through, not minimized, not cloaked by DWM, and containing
# the point. Tests use it to tell "the app refused to raise" from "this session
# has something covering the line".
function Get-Coverer([IntPtr]$below, [int]$x, [int]$y) {
  $cur = [W]::GetWindow($below, 3)             # GW_HWNDPREV = the one above
  for ($i = 0; $i -lt 512 -and $cur -ne [IntPtr]::Zero; $i++) {
    $r = Get-Rect $cur
    if ([W]::IsWindowVisible($cur) -and -not [W]::IsClickThrough($cur) -and
        -not [W]::IsIconic($cur) -and -not [W]::IsCloaked($cur) -and $r -and
        $x -ge $r.Left -and $x -lt $r.Right -and $y -ge $r.Top -and $y -lt $r.Bottom) {
      $sb = New-Object System.Text.StringBuilder 256
      [void][W]::GetClassNameW($cur, $sb, 256)
      return $sb.ToString()
    }
    $cur = [W]::GetWindow($cur, 3)
  }
  return ""
}

# What Windows remembers about a window: where it comes back to, and in what
# state. A minimized window keeps the rect it had before it was minimized, which
# is how a test tells "minimized where the user can find it" from "minimized
# somewhere off screen".
function Get-Placement([IntPtr]$h) {
  $wp = New-Object W+WINDOWPLACEMENT
  $wp.length = [System.Runtime.InteropServices.Marshal]::SizeOf($wp)
  if (-not [W]::GetWindowPlacement($h, [ref]$wp)) { return $null }
  return $wp
}

function Get-VirtualBounds {
  $x = [W]::GetSystemMetrics(76)   # SM_XVIRTUALSCREEN
  $y = [W]::GetSystemMetrics(77)
  $w = [W]::GetSystemMetrics(78)
  $h = [W]::GetSystemMetrics(79)
  return New-Object W+RECT -Property @{ Left = $x; Top = $y; Right = ($x + $w); Bottom = ($y + $h) }
}

# The primary monitor's work area: the rectangle the line centres a dragged
# window in, taskbar excluded. The app reads the same thing (rcWork) for the
# monitor the line is on, which is the primary one in the test sandbox.
function Get-WorkArea {
  $r = New-Object W+RECT
  if (-not [W]::SystemParametersInfoW(0x0030, 0, [ref]$r, 0)) { return $null }   # SPI_GETWORKAREA
  return $r
}

# Pretend the pointer is at (x, y). The app answers every hover, raise and
# visit question from this until Point-Real. A session with no input desktop
# refuses SetCursorPos, and the z-order rule is entirely "where is the pointer",
# so this is how those sessions test it.
function Point-At([IntPtr]$overlay, [int]$x, [int]$y) {
  Invoke-Wm $overlay $script:KitPointAt ([IntPtr]($x -band 0xFFFFFFFF)) ([IntPtr]$y)
}

function Point-Real([IntPtr]$overlay) {
  Invoke-Wm $overlay $script:KitPointAt ([IntPtr]0xFFFFFFFF) ([IntPtr]::Zero)
}

# Pretend one of his windows is being dragged, the way the system's move loop
# reports it. The z-order rule is the poll's, not the loop's, so this is enough
# to prove that a drag never brings the cards up.
function Drag-Pretend([IntPtr]$overlay, [bool]$on) {
  Invoke-Wm $overlay $script:KitPretendDrag ([IntPtr]([int]$on)) ([IntPtr]::Zero)
}

function Move-Cursor([int]$x, [int]$y) {  for ($try = 1; $try -le 4; $try++) {
    if (-not [W]::SetCursorPos($x, $y)) { Start-Sleep -Milliseconds 150; continue }
    Start-Sleep -Milliseconds 120
    $p = New-Object W+POINT
    if (-not [W]::GetCursorPos([ref]$p)) { continue }
    if ($p.X -eq $x -and $p.Y -eq $y) { return $true }
    # Somebody else moved the pointer - a hand on the same machine. Say so and
    # try again rather than failing the case on the first nudge.
    Write-Host "    the pointer drifted to ($($p.X),$($p.Y)); trying again"
  }
  return $false
}

# Real mouse input, because that is the only way to reach the line: the overlay
# is click-through until the pointer is over it, and a synthetic message would
# skip the hit-test and the capture the app sets for itself.
function Press-At([int]$x, [int]$y) {
  if (-not (Move-Cursor $x $y)) { return $false }
  Start-Sleep -Milliseconds 120   # one poll, so the strip stops being click-through
  [W]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
  return $true
}

function Release-Mouse {
  [W]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
  Start-Sleep -Milliseconds 80
}

# Drives the button from one point to another in steps, the way a hand does.
function Drag-From-To([int]$x1, [int]$y1, [int]$x2, [int]$y2, [int]$steps) {
  if (-not (Press-At $x1 $y1)) { return $false }
  for ($i = 1; $i -le $steps; $i++) {
    $x = [int]($x1 + ($x2 - $x1) * $i / $steps)
    $y = [int]($y1 + ($y2 - $y1) * $i / $steps)
    if (-not (Move-Cursor $x $y)) { Release-Mouse; return $false }
  }
  Release-Mouse
  return $true
}

function Click-At([int]$x, [int]$y) {
  if (-not (Press-At $x $y)) { return $false }
  Release-Mouse
  return $true
}

# Drives the overlay's own handlers without the real pointer: it posts the same
# messages the window manager would, in client coordinates. It skips the system
# hit-test and the capture, so it is no substitute for a real drag; what it does
# cover is the geometry, the journal and the redraw, on a machine that cannot be
# clicked (a locked screen, a session with no input desktop).
function Post-Mouse([IntPtr]$overlay, [uint32]$msg, [int]$x, [int]$y) {
  $lp = [IntPtr](($y -shl 16) -bor ($x -band 0xFFFF))
  Invoke-Wm $overlay $msg ([IntPtr]::Zero) $lp
}

function Drag-By-Messages([IntPtr]$overlay, [int]$x1, [int]$y1, [int]$x2, [int]$y2, [int]$steps) {
  Post-Mouse $overlay $script:KitMouseDown $x1 $y1
  Start-Sleep -Milliseconds 60
  for ($i = 1; $i -le $steps; $i++) {
    $x = [int]($x1 + ($x2 - $x1) * $i / $steps)
    $y = [int]($y1 + ($y2 - $y1) * $i / $steps)
    Post-Mouse $overlay $script:KitMouseMove $x $y
    Start-Sleep -Milliseconds 30
  }
  Post-Mouse $overlay $script:KitMouseUp $x2 $y2
  Start-Sleep -Milliseconds 120
}

# The lock screen is a full-screen window in front of everything and it takes
# every click, but moving the pointer behind it still succeeds. A test that
# cannot get past it has to skip its pointer steps instead of failing them.
function Test-LockScreen {
  $fg = [W]::GetForegroundWindow()
  if ($fg -eq [IntPtr]::Zero) { return $false }
  $fgPid = [uint32]0
  [void][W]::GetWindowThreadProcessId($fg, [ref]$fgPid)
  return (Get-Process -Id $fgPid -ErrorAction SilentlyContinue).ProcessName -eq "LockApp"
}

# A session with no input desktop (a service, or an agent shell) cannot move or
# even read the cursor. Anything that depends on the pointer has to be skipped
# rather than failed there. The pointer is put back where it was found.
function Test-CursorControl {
  if (Test-LockScreen) {
    Write-Host "  the lock screen is up; clicks cannot reach the line"
    return $false
  }
  $p = New-Object W+POINT
  [void][W]::GetCursorPos([ref]$p)
  $mid = New-Object W+POINT
  $mid.X = [int]([W]::GetSystemMetrics(0) / 2)
  $mid.Y = [int]([W]::GetSystemMetrics(1) / 2)
  $ok = Move-Cursor $mid.X $mid.Y
  if ($ok) { [void][W]::SetCursorPos($p.X, $p.Y) }
  return $ok
}

# The app parks a hung window by minimizing it where it stands. Minimizing is
# the one "parked" state Windows itself can undo: the window keeps the rectangle
# it came from, so the taskbar, Alt+Tab and a click on its button all bring it
# back where the user left it. Moving it past the right edge of the virtual
# desktop was the old way, and it was a trap - only the process that moved it
# could undo that, so a killed ShaiWin left the window off screen with nothing
# to click. "Parked" is that state: minimized, still remembering a place on
# screen to come back to.
function Test-Parked([IntPtr]$h) {
  if (-not [W]::IsIconic($h)) { return $false }
  $wp = Get-Placement $h
  if (-not $wp) { return $false }
  $v = Get-VirtualBounds
  $r = $wp.rcNormalPosition
  # "On screen" is what the app means by it - the rectangle meets a monitor -
  # not "fits inside one": a window can hang off an edge by a few pixels and
  # still be exactly where the user left it.
  return ($r.Left -lt $v.Right -and $r.Right -gt $v.Left -and
          $r.Top -lt $v.Bottom -and $r.Bottom -gt $v.Top)
}

function Test-OnDesktop([IntPtr]$h) {
  return ([W]::IsWindowVisible($h) -and -not [W]::IsIconic($h) -and -not (Test-Parked $h))
}

# A window that is already minimized is hung up where it stands: it stays
# minimized, and its record stops saying "on the desktop, still hooked".
function Test-Hung-Up([IntPtr]$h, [string]$journalPath) {
  $rec = Get-Record $journalPath $h
  return ([W]::IsIconic($h) -and $rec -and -not $rec.Hooked)
}

# Minimizes through the taskbar path and proves the window never appeared on the
# desktop on its way back up to the line. Restoring it first to move it off
# screen was the flash the user called chaos. The app waits out a grace period
# before it hangs a window up, so the whole window is sampled.
# Returns $null when it behaved, or the reason it did not.
function Minimize-Hang([IntPtr]$h, [string]$journalPath) {
  [void][W]::ShowWindow($h, 2)   # SW_MINIMIZE, the same thing the taskbar asks for
  if (-not (Wait-Until ({ [W]::IsIconic($h) }) 2000)) { return "the window never minimized" }
  $end = [Environment]::TickCount + 1600
  while ([Environment]::TickCount -lt $end) {
    if (Test-OnDesktop $h) {
      return "the window was shown on the desktop while it was being hung up - the flash the user called chaos"
    }
    if (Test-Hung-Up $h $journalPath) { return $null }
    Start-Sleep -Milliseconds 25
  }
  if (-not (Test-Hung-Up $h $journalPath)) { return "minimizing a hooked window did not put its card back on the line" }
  return $null
}

function Wait-Until([scriptblock]$condition, [int]$timeoutMs) {
  $end = [Environment]::TickCount + $timeoutMs
  while ([Environment]::TickCount -lt $end) {
    if ([bool](& $condition)) { return $true }
    Start-Sleep -Milliseconds 60
  }
  return [bool](& $condition)
}

function Wait-Visibility([IntPtr]$h, [bool]$want, [int]$timeoutMs) {
  $end = [Environment]::TickCount + $timeoutMs
  while ([Environment]::TickCount -lt $end) {
    if ([W]::IsWindowVisible($h) -eq $want) { return $true }
    Start-Sleep -Milliseconds 50
  }
  return ([W]::IsWindowVisible($h) -eq $want)
}

# --- the journal ---------------------------------------------------------------

# One tab-separated record per hung window; the thirteenth field says the
# window is back on the desktop but still hooked.
function Read-Journal([string]$path) {
  if (-not (Test-Path $path)) { return @() }
  $lines = @(Get-Content $path -Encoding UTF8 -ErrorAction SilentlyContinue)
  return @($lines | Where-Object { $_.Trim().Length -gt 0 } | ForEach-Object {
    $f = $_ -split "`t"
    [pscustomobject]@{
      Hwnd = [long]$f[0]
      Pid = [long]$f[1]
      Hooked = ($f.Length -ge 13 -and $f[12] -eq "1")
      # True when the window was dragged onto the line, so it comes back in the
      # middle of the screen rather than where it was grabbed.
      Centred = ($f.Length -ge 17 -and $f[16] -eq "1")
      Fields = $f
    }
  })
}

function Get-Record([string]$path, [IntPtr]$h) {
  Read-Journal $path | Where-Object { $_.Hwnd -eq [long]$h } | Select-Object -First 1
}

# --- the cards on screen -------------------------------------------------------

# The app writes the clickable rectangle of every card it just drew next to the
# frame dump. Asking for one is how a test learns where the cards are without
# guessing at the layout.
function Get-CardRects([string]$dir, [int]$appPid, [IntPtr]$overlay) {
  $path = Join-Path $dir "hits-$appPid.txt"
  Remove-Item $path -Force -ErrorAction SilentlyContinue
  Invoke-Wm $overlay $script:KitDumpFrame ([IntPtr]::Zero) ([IntPtr]::Zero)
  if (-not (Wait-Until ({ Test-Path $path }) 3000)) {
    Fail "the app did not report where its cards are (no $path)"
  }
  Start-Sleep -Milliseconds 120   # the file is written in place, not swapped
  return @(Get-Content $path | Where-Object { $_.Trim().Length -gt 0 } | ForEach-Object {
    $f = $_ -split "`t"
    $r = $f[2] -split ","
    # The "show the line" pill is reported as u64::MAX, which does not fit in a
    # signed long. Cards keep their normal id; the pill is called out separately.
    $id = [uint64]$f[0]
    [pscustomobject]@{
      Id = $id; Hwnd = [long]$f[1]; Peek = ($id -eq [uint64]::MaxValue)
      Left = [int]$r[0]; Top = [int]$r[1]; Right = [int]$r[2]; Bottom = [int]$r[3]
    }
  })
}

function Get-Center($rect) {
  return New-Object W+POINT -Property @{ X = [int](($rect.Left + $rect.Right) / 2); Y = [int](($rect.Top + $rect.Bottom) / 2) }
}

# --- throwaway windows ---------------------------------------------------------

$script:FixtureSource = @'
param([string]$Title, [int]$X, [int]$Y)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
# The console is this fixture's shell, not part of the test. Hiding the console
# window from inside is what works: starting the process with -WindowStyle
# Hidden also hides the form, because a new top-level window takes its show
# state from the process.
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Con {
  [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
}
"@
[void][Con]::ShowWindow([Con]::GetConsoleWindow(), 0)
$form = New-Object System.Windows.Forms.Form
$form.Text = $Title
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point($X, $Y)
$form.Size = New-Object System.Drawing.Size(460, 320)
[System.Windows.Forms.Application]::Run($form)
'@

$script:fixturePids = @()
$script:fixtureHandles = @()
$script:fixtureDir = ""

function Initialize-Fixtures([string]$dir) {
  $script:fixtureDir = $dir
  Set-Content -Path (Join-Path $dir "fixture.ps1") -Value $script:FixtureSource -Encoding ascii
}

function New-Fixture([string]$name, [int]$index) {
  $title = "ShaiWinFixture$name"
  $v = Get-VirtualBounds
  $x = $v.Left + 120 + $index * 90
  $y = $v.Top + 200 + $index * 70
  $shell = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
  $childArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-STA", "-File", (Join-Path $script:fixtureDir "fixture.ps1"), "-Title", $title, "-X", $x, "-Y", $y)
  $proc = Start-Process $shell -ArgumentList $childArgs -PassThru
  $script:fixturePids += @{ Title = $title; Id = $proc.Id }
  if (-not (Wait-Until ({ (Find-By-Title $title) -ne [IntPtr]::Zero }) 20000)) {
    Fail "fixture $title never showed a window (pid $($proc.Id))"
  }
  $h = Find-By-Title $title
  $script:fixtureHandles += $h
  return $h
}

function Get-FixturePid([string]$name) {
  $title = "ShaiWinFixture$name"
  ($script:fixturePids | Where-Object { $_.Title -eq $title } | Select-Object -First 1).Id
}

function Remove-Fixtures {
  foreach ($h in $script:fixtureHandles) {
    if ($h -ne [IntPtr]::Zero) { Invoke-Wm $h $script:KitClose ([IntPtr]::Zero) ([IntPtr]::Zero) }
  }
  Start-Sleep -Milliseconds 400
  foreach ($owned in $script:fixturePids) {
    Stop-Process -Id $owned.Id -Force -ErrorAction SilentlyContinue
  }
}

# --- the app itself ------------------------------------------------------------

function Stop-ShaiwinWithEverythingDown {
  $old = Get-Process shaiwin -ErrorAction SilentlyContinue
  if (-not $old) { return }
  $overlay = Get-Overlay
  if ($overlay -ne [IntPtr]::Zero) {
    Invoke-Wm $overlay $script:KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero)
    Start-Sleep -Milliseconds 1800
  }
  $old | Stop-Process -Force
  Start-Sleep -Milliseconds 500
}

# A private LOCALAPPDATA so the tests can never write the real journal or the
# real settings. The caller keeps whatever was there.
function New-Sandbox([string]$root, [string]$settings) {
  Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
  $state = Join-Path $root "localappdata"
  New-Item -ItemType Directory -Force (Join-Path $state "ShaiWin") | Out-Null
  Set-Content -Path (Join-Path $state "ShaiWin\settings.txt") -Value $settings -Encoding ascii
  return $state
}

function Start-Shaiwin([string]$exe, [string]$state) {
  $saved = $env:LOCALAPPDATA
  $env:LOCALAPPDATA = $state
  try { return Start-Process -FilePath $exe -PassThru } finally { $env:LOCALAPPDATA = $saved }
}
