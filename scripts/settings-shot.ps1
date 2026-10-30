# Look at the settings window without anybody having to click it - and click a
# few things that nobody has to click to see whether they work.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\settings-shot.ps1
#
# Starts the app against a private LOCALAPPDATA, asks it to open its settings
# (WM_APP+15), captures the panel with PrintWindow, then posts messages at the
# shortcut boxes and the buttons: a box clicked is listening, a box whose × was
# clicked is empty, and Save writes what is on screen to the settings file.
#
# The PNGs land in %TEMP%\shaiwin-settings-shots.
#
# Like the other tests this stops any running ShaiWin first, so close nothing you
# care about - and the real settings are never read or written.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe")
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

Add-Type -AssemblyName System.Drawing
if (-not ("Win.Print" -as [type])) {
  Add-Type -Namespace Win -Name Print -MemberDefinition @'
    public delegate bool EnumProc(IntPtr h, IntPtr lp);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr lp);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)]
    public static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder s, int n);
'@
}

$shotDir = Join-Path $env:TEMP "shaiwin-settings-shots"
New-Item -ItemType Directory -Force -Path $shotDir | Out-Null
Get-ChildItem $shotDir -Filter "*.png" -ErrorAction SilentlyContinue | Remove-Item -Force

$script:Down = [uint32]0x0201
$script:Up = [uint32]0x0202
$script:Move = [uint32]0x0200
$script:KeyDown = [uint32]0x0100
$script:VKEscape = [IntPtr]0x1B

function Get-Children([IntPtr]$parent, [string]$want) {
  $script:kids = New-Object System.Collections.ArrayList
  $cb = [Win.Print+EnumProc]{
    param($h, $lp)
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win.Print]::GetClassNameW($h, $sb, 256)
    if ($sb.ToString() -eq $want) { [void]$script:kids.Add($h) }
    return $true
  }
  [void][Win.Print]::EnumChildWindows($parent, $cb, [IntPtr]::Zero)
  return $script:kids.ToArray()
}

function Get-Box([IntPtr]$h) {
  $r = Get-Rect $h
  if (-not $r) { Fail "no rect for a child window" }
  # One subtraction per line: PowerShell binds the comma tighter than the minus,
  # so @($r.Right - $r.Left, $r.Bottom - $r.Top) asks an array to subtract.
  $w = $r.Right - $r.Left
  $t = $r.Bottom - $r.Top
  if ($w -lt 2 -or $t -lt 2) { Fail "the control has no size ($w x $t)" }
  return @{ w = $w; t = $t }
}

function Click-At([IntPtr]$h, [int]$x, [int]$y) {
  # lParam is the position in the control's own client coordinates.
  $lp = Pack-Lp $x $y
  Invoke-Wm $h $script:Down ([IntPtr]::Zero) $lp
  Invoke-Wm $h $script:Up ([IntPtr]::Zero) $lp
  Start-Sleep -Milliseconds 250
}

function Click-Middle([IntPtr]$h) {
  $box = Get-Box $h
  $x = [int]($box.w / 2)
  $y = [int]($box.t / 2)
  Click-At $h $x $y
}

function Pack-Lp([int]$x, [int]$y) {
  return [IntPtr](($y -shl 16) -bor ($x -band 0xFFFF))
}

# Press, move, release. The control sees a drag, not two clicks.
function Drag-Vertical([IntPtr]$h, [int]$x, [int]$from, [int]$to) {
  Invoke-Wm $h $script:Down ([IntPtr]::Zero) (Pack-Lp $x $from)
  Invoke-Wm $h $script:Move ([IntPtr]::Zero) (Pack-Lp $x $to)
  Invoke-Wm $h $script:Up ([IntPtr]::Zero) (Pack-Lp $x $to)
  Start-Sleep -Milliseconds 250
}

# EnumChildWindows walks the z-order, which is not the order the controls were
# made in, so a test must never read this list by index. Sorting by the rect
# puts the rows back top to bottom.
function Get-Children-Sorted([IntPtr]$parent, [string]$want) {
  @(Get-Children $parent $want | Sort-Object { (Get-Rect $_).Top })
}

function Save-Shot([IntPtr]$hwnd, [string]$path) {
  $r = New-Object W+RECT
  if (-not [W]::GetWindowRect($hwnd, [ref]$r)) { Fail "no rect for $path" }
  $w = $r.Right - $r.Left
  $h = $r.Bottom - $r.Top
  if ($w -lt 20 -or $h -lt 20) { Fail "the window is not really there ($w x $h)" }
  # PrintWindow, not the screen: this shell has no desktop to grab. Every window
  # in the panel paints itself for exactly that reason - an owner-drawn stock
  # button answers WM_DRAWITEM instead and comes back as a blank grey rectangle.
  $bmp = New-Object System.Drawing.Bitmap $w, $h, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $gfx = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $gfx.GetHdc()
  $ok = [Win.Print]::PrintWindow($hwnd, $hdc, 2)   # PW_RENDERFULLCONTENT
  $gfx.ReleaseHdc($hdc)
  $gfx.Dispose()
  if (-not $ok) { $bmp.Dispose(); Fail "PrintWindow failed for $path" }
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  Write-Host "shot: $path ($w x $h)"
}

function Open-Settings([string]$name, [string]$settings) {
  $root = Join-Path $env:TEMP "shaiwin-settings-$name"
  $state = New-Sandbox $root $settings
  $proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2200
  # The line window is the app window: one HWND, painted two ways.
  $app = Get-Overlay
  if (-not (Test-Window $app)) { Fail "the app window is not there" }
  Invoke-Wm $app ([uint32](0x8000 + 15)) ([IntPtr]::Zero) ([IntPtr]::Zero)
  if (-not (Wait-Until { Test-Window (Find-By-Class "ShaiWin.Settings") } 5000)) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Fail "the settings window never opened"
  }
  Start-Sleep -Milliseconds 900
  return @{
    state = $state
    proc = $proc
    wnd = Find-By-Class "ShaiWin.Settings"
  }
}

function Close-Settings($session) {
  if ($session) {
    Stop-Process -Id $session.proc.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 300
  }
}

$base = "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=0`ndrag_hang=1`nexcluded=`n"

Write-Host "== the panel, as it opens =="
Stop-ShaiwinWithEverythingDown

# Ctrl+Alt+2 and Ctrl+T are what the app ships with.
$session = Open-Settings "shipped" "$base`nsag=-1"
try {
  Save-Shot $session.wnd (Join-Path $shotDir "settings-shipped.png")

  Write-Host "== the shortcut box, clicked =="
  $keys = Get-Children-Sorted $session.wnd "ShaiWin.Settings.Key"
  if ($keys.Count -lt 2) { Fail "expected two shortcut boxes, found $($keys.Count)" }
  Click-Middle $keys[0]
  Save-Shot $session.wnd (Join-Path $shotDir "settings-listening.png")
  # Escape puts the box back the way it found it.
  Invoke-Wm $keys[0] $script:KeyDown $script:VKEscape ([IntPtr]::Zero)
  Start-Sleep -Milliseconds 400

  Write-Host "== the × clears a shortcut, and Save writes it =="
  $box = Get-Box $keys[1]
  # The × lives in the right-hand end of the field, 26 DIP wide at any scale.
  Click-At $keys[1] ($box.w - 13) ([int]($box.t / 2))
  Save-Shot $session.wnd (Join-Path $shotDir "settings-cleared.png")
  $buttons = Get-Children-Sorted $session.wnd "ShaiWin.Settings.Button"
  if ($buttons.Count -lt 2) { Fail "expected two buttons, found $($buttons.Count)" }
  # Save is the right-hand one of the pair.
  $save = ($buttons | Sort-Object { (Get-Rect $_).Left })[-1]
  Click-Middle $save
  Start-Sleep -Milliseconds 800
  $saved = Get-Content (Join-Path $session.state "ShaiWin\settings.txt") -Raw
  if ($saved -notmatch "toggle_vk=0") { Fail "the cleared shortcut was not saved:`n$saved" }
  if ($saved -notmatch "hang_vk=50") { Fail "the untouched shortcut moved:`n$saved" }
  Write-Host "saved: toggle_vk=0, hang_vk=50"
} finally {
  Close-Settings $session
}

Write-Host "== an unset shortcut, before anybody touches it =="
$session = Open-Settings "unset" "$base`nsag=6`nhang_mods=0`nhang_vk=0"
try {
  Save-Shot $session.wnd (Join-Path $shotDir "settings-unset.png")
} finally {
  Close-Settings $session
}

Write-Host "== the switch and the rope, operated =="
Stop-ShaiwinWithEverythingDown
$session = Open-Settings "controls" "$base`nsag=4"
try {
  Save-Shot $session.wnd (Join-Path $shotDir "settings-before.png")

  $switches = Get-Children-Sorted $session.wnd "ShaiWin.Settings.Switch"
  if ($switches.Count -lt 2) { Fail "expected two switches, found $($switches.Count)" }
  # The top one is 常驻显示 (always_on); the row order is the file order.
  Click-Middle $switches[0]

  $ropes = @(Get-Children $session.wnd "ShaiWin.Settings.Rope")
  if ($ropes.Count -ne 1) { Fail "expected one rope tile, found $($ropes.Count)" }
  $rope = $ropes[0]
  $box = Get-Box $rope
  # The tile is 88 logical pixels tall, so its device height is the scale.
  $scale = $box.t / 88.0
  $x = [int]($box.w / 2)
  $from = [int](10 * $scale)
  # Forty logical pixels of pull is four percent: the tile maps pixels to
  # percent at ten apiece, so the whole 0-16% range needs more travel than the
  # tile has, which is what the mouse capture is for.
  $to = [int]($from + 40 * $scale)
  Drag-Vertical $rope $x $from $to
  Save-Shot $session.wnd (Join-Path $shotDir "settings-rope.png")
  # Then the keyboard, which points the same way the hand does: down is deeper.
  Invoke-Wm $rope $script:KeyDown ([IntPtr]0x22) ([IntPtr]::Zero)
  Invoke-Wm $rope $script:KeyDown ([IntPtr]0x28) ([IntPtr]::Zero)
  Start-Sleep -Milliseconds 400
  Save-Shot $session.wnd (Join-Path $shotDir "settings-rope-keys.png")

  $buttons = Get-Children-Sorted $session.wnd "ShaiWin.Settings.Button"
  $save = ($buttons | Sort-Object { (Get-Rect $_).Left })[-1]
  Click-Middle $save
  Start-Sleep -Milliseconds 800
  $saved = Get-Content (Join-Path $session.state "ShaiWin\settings.txt") -Raw
  if ($saved -notmatch "always_on=1") { Fail "the switch click did not save:`n$saved" }
  if ($saved -notmatch "sag=9.1") { Fail "the pull did not save (wanted 4 + 4 + 1 + 0.1):`n$saved" }
  Write-Host "saved: always_on=1, sag=9.1"
} finally {
  Close-Settings $session
}

Write-Host "== a click on the rope does not spend the automatic curve =="
Stop-ShaiwinWithEverythingDown
$session = Open-Settings "autokeep" "$base`nsag=-1"
try {
  $ropes = @(Get-Children $session.wnd "ShaiWin.Settings.Rope")
  $box = Get-Box $ropes[0]
  # Straight down the middle, and back up again: no travel, so no pull.
  Click-At $ropes[0] ([int]($box.w / 2)) ([int]($box.t / 2))
  Start-Sleep -Milliseconds 300
  $buttons = Get-Children-Sorted $session.wnd "ShaiWin.Settings.Button"
  $save = ($buttons | Sort-Object { (Get-Rect $_).Left })[-1]
  Click-Middle $save
  Start-Sleep -Milliseconds 800
  $saved = Get-Content (Join-Path $session.state "ShaiWin\settings.txt") -Raw
  if ($saved -notmatch "sag=`r?`n") { Fail "a bare click froze the shipped curve:`n$saved" }
  Write-Host "saved: still automatic"
} finally {
  Close-Settings $session
}

Write-Host "== the preview does not shake the line, and the tile sits still =="
Stop-ShaiwinWithEverythingDown
$stillRoot = Join-Path $env:TEMP "shaiwin-settings-still-fixtures"
New-Item -ItemType Directory -Force -Path $stillRoot | Out-Null
Initialize-Fixtures $stillRoot
$session = Open-Settings "still" "$base`nalways_on=1`nsag=4"
try {
  $app = Get-Overlay
  $before = (Get-Rect $app)
  $beforeH = $before.Bottom - $before.Top
  $fixture = New-Fixture "Still" 0
  Invoke-Wm $app ([uint32](0x8000 + 10)) ([IntPtr]::Zero) ([IntPtr]$fixture)
  Start-Sleep -Milliseconds 1500
  $ropes = @(Get-Children $session.wnd "ShaiWin.Settings.Rope")
  $box = Get-Box $ropes[0]
  $scale = $box.t / 88.0
  $x = [int]($box.w / 2)
  # Pull slowly and watch the line window while it happens. The preview used to
  # re-place the window from the bare strip on every step of the drag, which cut
  # the card off for a frame - the flashing that ran along with the rope.
  Invoke-Wm $ropes[0] $script:Down ([IntPtr]::Zero) (Pack-Lp $x ([int](10 * $scale)))
  $heights = @()
  for ($i = 1; $i -le 12; $i++) {
    $y = [int]((10 + $i * 4) * $scale)
    Invoke-Wm $ropes[0] $script:Move ([IntPtr]::Zero) (Pack-Lp $x $y)
    Start-Sleep -Milliseconds 60
    $r = Get-Rect $app
    $heights += ($r.Bottom - $r.Top)
  }
  Invoke-Wm $ropes[0] $script:Up ([IntPtr]::Zero) (Pack-Lp $x ([int](58 * $scale)))
  $tall = ($heights | Measure-Object -Maximum).Maximum
  $short = ($heights | Measure-Object -Minimum).Minimum
  # The panel is allowed to grow while the rope is pulled deeper — a lower rope
  # puts the card lower — but it is never allowed to fall back to the bare strip
  # mid-drag, which is what re-placing the window from `strip()` did.
  $drop = 0
  for ($i = 1; $i -lt $heights.Count; $i++) {
    $drop = [Math]::Max($drop, $heights[$i - 1] - $heights[$i])
  }
  Write-Host "  panel height: $beforeH before the card, $short..$tall while dragging, biggest step down $drop"
  if ($tall -le $beforeH) { Fail "hanging a window never made the panel taller" }
  if ($short -le $beforeH) { Fail "the panel fell back to the bare strip mid-preview: the card was clipped" }
  if ($drop -gt 2) {
    Fail "the line window shrank by $drop px while the rope was being pulled: the preview re-places the window"
  }
  # And once the hand is off, it has to stay put. The readout used to be driven
  # by a 60 ms timer that never stopped while the tile held focus, so the panel
  # flashed for the rest of the session.
  $a = Join-Path $shotDir "rope-still-a.png"
  $b = Join-Path $shotDir "rope-still-b.png"
  Save-Shot $ropes[0] $a
  Start-Sleep -Milliseconds 900
  Save-Shot $ropes[0] $b
  if ((Get-FileHash $a).Hash -ne (Get-FileHash $b).Hash) {
    Fail "the rope tile was still redrawing itself 900 ms after the drag ended"
  }
  Write-Host "  the tile holds still"
} finally {
  Close-Settings $session
  Remove-Fixtures
  Remove-Item $stillRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "done: $shotDir"
