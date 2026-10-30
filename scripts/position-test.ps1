# Where a window comes back to.
#
# The user's report was "click a card and the window does not come back where it
# was", and when asked what he saw he said 出现位置不对 - the position is wrong,
# not missing. So this test is not about whether the window appears, it is about
# whether it appears at the rectangle it left.
#
# Each state a window can be hung up in is its own case, because each one stores
# a different rectangle: a plain window keeps what you see, a maximized window
# keeps the size it would restore to, a snapped window keeps the half it was
# pushed into, and a window hung up while minimized keeps a position Windows
# keeps for itself.
#
# The one exception is a window dragged onto the line: it comes back in the
# middle of the screen the line is on, so its record asks for the centre rather
# than the rectangle it was grabbed at. Case 7 is that path, drag and all.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/position-test.ps1

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$canPointer = Test-CursorControl

$root = Join-Path $env:TEMP "shaiwin-position"
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=1`nexcluded=`n"
Initialize-Fixtures $root
$journal = Join-Path $state "ShaiWin\parked.txt"
$shotDir = Join-Path $root "shots"
$env:SHAIWIN_DUMP_DIR = $shotDir

Stop-ShaiwinWithEverythingDown

# The rectangle the app should put back, and the rectangle it did put back.
function Compare-Rect($want, [IntPtr]$h) {
  if (-not (Test-Window $h)) { Fail "the fixture window is gone" }
  $got = Get-Rect $h
  if (-not $got) { Fail "fixture $label has no rectangle at all" }
  $dx = [Math]::Abs($got.Left - $want.Left)
  $dy = [Math]::Abs($got.Top - $want.Top)
  $dw = [Math]::Abs(($got.Right - $got.Left) - ($want.Right - $want.Left))
  $dh = [Math]::Abs(($got.Bottom - $got.Top) - ($want.Bottom - $want.Top))
  return [pscustomobject]@{
    Left = $got.Left; Top = $got.Top; Right = $got.Right; Bottom = $got.Bottom
    D = ($dx + $dy + $dw + $dh)
    Detail = "got {0},{1} {2}x{3} - wanted {4},{5} {6}x{7} (off by {8} px)" -f `
      $got.Left, $got.Top, ($got.Right - $got.Left), ($got.Bottom - $got.Top), `
      $want.Left, $want.Top, ($want.Right - $want.Left), ($want.Bottom - $want.Top), ($dx + $dy + $dw + $dh)
  }
}

# Hang one window, put it back, and say where it landed.
function Invoke-Case([string]$name, [IntPtr]$h, [bool]$wantZoomed) {
  Write-Host "== $name =="
  $before = Get-Rect $h
  if (-not $before) { Fail "${name}: the fixture has no window to hang" }
  Write-Host ("  leaving {0},{1} {2}x{3}" -f $before.Left, $before.Top, ($before.Right - $before.Left), ($before.Bottom - $before.Top))

  Invoke-Wm (Get-Overlay) $script:KitHang ([IntPtr]::Zero) ([IntPtr]$h)
  # A maximized window is taken off screen by minimizing it (moving it would
  # drop it out of the maximized state); a normal window is parked past the
  # right edge. Both mean the window is on the line.
  $wentUp = if ($wantZoomed) { { [W]::IsIconic($h) } } else { { Test-Parked $h } }
  if (-not (Wait-Until $wentUp 6000)) { Fail "${name}: the window never went up to the line" }
  # A window hung up by hotkey (or by message, which is the same path) comes
  # back where it was, so its record must not ask for the middle of the screen.
  if (-not (Wait-Until ({ Get-Record $journal $h }) 4000)) { Fail "${name}: no journal record after hanging" }
  if ((Get-Record $journal $h).Centred) {
    Fail "${name}: a hotkey-hung window was recorded as centred - it must reopen where it was"
  }

  Invoke-Wm (Get-Overlay) $script:KitTakeOne ([IntPtr]::Zero) ([IntPtr]$h)
  if (-not (Wait-Until ({ Test-OnDesktop $h -or [W]::IsZoomed($h) }) 6000)) {
    Fail "${name}: the window never came back down"
  }
  Start-Sleep -Milliseconds 600   # the app writes its journal after the move

  $back = Compare-Rect $before $h
  if ($wantZoomed) {
    if (-not [W]::IsZoomed($h)) { Fail "${name}: a maximized window came back un-maximized - the user sees the size change too" }
  } else {
    if ([W]::IsZoomed($h)) { Fail "${name}: a plain window came back maximized" }
  }
  if ([W]::IsIconic($h)) { Fail "${name}: the window came back minimized - clicking a card must show it" }
  if ($back.D -gt 4) {
    Fail "${name}: it did not return where it was - $($back.Detail)"
  }
  Write-Host ("  $($back.Detail)")
  Write-Host "  ok"
}

# Grabs a fixture by its title bar, carries it into the hang band and lets go -
# the gesture the user makes to put a window on the line. Returns the rectangle
# the window had before it was grabbed.
function Drag-ToLine([IntPtr]$h) {
  $r = Get-Rect $h
  if (-not $r) { Fail "the fixture has no window to drag" }
  $grabX = [int](($r.Left + $r.Right) / 2)
  $grabY = [int]($r.Top + 18)
  if (-not (Move-Cursor $grabX $grabY)) { Fail "lost cursor control" }
  [W]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
  Start-Sleep -Milliseconds 200
  $v = Get-VirtualBounds
  $bandY = [int]($v.Top + 60)
  for ($i = 1; $i -le 8; $i++) {
    [void](Move-Cursor $grabX ($grabY + [int](($bandY - $grabY) * $i / 8)))
  }
  Start-Sleep -Milliseconds 400
  [W]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
  return $r
}

try {
  $proc = Start-Shaiwin (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe") $state
  Start-Sleep -Milliseconds 2500
  if (-not (Test-Window (Get-Overlay))) { Fail "no overlay to talk to" }

  # 1. a plain window, the case the user hit most
  $a = New-Fixture "A" 0
  Invoke-Case "1. plain window" $a $false

  # 2. maximized: rcNormalPosition is not what is on screen while it hangs
  $b = New-Fixture "B" 1
  [void][W]::ShowWindow($b, 3)   # SW_MAXIMIZE
  if (-not (Wait-Until ({ [W]::IsZoomed($b) }) 4000)) { Fail "the fixture would not maximize" }
  # Maximized, Windows reports the whole monitor as its rectangle, so compare
  # against the size it restores to rather than the frame on screen.
  $bBefore = Get-Rect $b
  $work = Get-VirtualBounds
  Write-Host "case 2 maximized - on screen at $($bBefore.Left),$($bBefore.Top) monitor ends at $($work.Right),$($work.Bottom)"
  Invoke-Case "2. maximized window" $b $true

  # 3. snapped to one half by hand: the same rectangle a snap button would leave
  $c = New-Fixture "C" 2
  $w = Get-VirtualBounds
  $half = New-Object W+RECT -Property @{
    Left = $w.Left; Top = $w.Top
    Right = $w.Left + [int](($w.Right - $w.Left) / 2); Bottom = $w.Bottom
  }
  [void][W]::SetWindowPos($c, [IntPtr]::Zero, $half.Left, $half.Top, ($half.Right - $half.Left), ($half.Bottom - $half.Top), 0x0040)
  Start-Sleep -Milliseconds 500
  Invoke-Case "3. half-screen window" $c $false

  # 4. hung up from the minimized state: nothing is moved, so the rectangle it
  #    comes back to is the one Windows kept for it while it was out of sight.
  #    (Asking for a hang while minimized is a refusal, on purpose - this is the
  #    path where a hooked window is minimized by the user.)
  $d = New-Fixture "D" 3
  Invoke-Case "4a. plain, then taken down" $d $false
  $dRect = Get-Rect $d
  if (-not (Get-Record $journal $d)) { Fail "4. the window is not hooked after coming down - the test lost its premise" }
  $flash = Minimize-Hang $d $journal
  if ($flash) { Fail "4. minimizing a hooked window did not hang it up quietly: $flash" }
  Write-Host "  minimized on the desktop, so it went up in place"
  Invoke-Wm (Get-Overlay) $script:KitTakeOne ([IntPtr]::Zero) ([IntPtr]$d)
  if (-not (Wait-Until ({ Test-OnDesktop $d }) 6000)) { Fail "4. the window hung up minimized never came back" }
  Start-Sleep -Milliseconds 600
  $dBack = Compare-Rect $dRect $d
  if ($dBack.D -gt 4) {
    Fail "4. window hung up minimized: came back somewhere else - $($dBack.Detail)"
  }
  Write-Host "== 4b. back where it stood before the minimize =="
  Write-Host "  $($dBack.Detail)"
  Write-Host "  ok"

  # 5. the window is moved between two hangings. The second hang must remember
  #    where it stands now, not where it stood the first time - the report was
  #    点击无法恢复原来的位置, and an old record left in the card is exactly that.
  $f = New-Fixture "F" 5
  Invoke-Case "5a. hung and taken down once" $f $false
  $moved = New-Object W+RECT -Property @{ Left = 300; Top = 160; Right = 990; Bottom = 640 }
  [void][W]::SetWindowPos($f, [IntPtr]::Zero, $moved.Left, $moved.Top, ($moved.Right - $moved.Left), ($moved.Bottom - $moved.Top), 0x0010)
  Start-Sleep -Milliseconds 500
  $atMove = Get-Rect $f
  if ([Math]::Abs($atMove.Left - $moved.Left) -gt 4) { Fail "5. the fixture would not move" }
  $flash = Minimize-Hang $f $journal
  if ($flash) { Fail "5. minimizing the moved window did not hang it up quietly: $flash" }
  Write-Host ("  moved to {0},{1} and hung up again" -f $atMove.Left, $atMove.Top)
  Invoke-Wm (Get-Overlay) $script:KitTakeOne ([IntPtr]::Zero) ([IntPtr]$f)
  if (-not (Wait-Until ({ Test-OnDesktop $f }) 6000)) { Fail "5. the second hanging never came back down" }
  Start-Sleep -Milliseconds 600
  $fBack = Compare-Rect $atMove $f
  if ($fBack.D -gt 4) {
    Fail "5. hung twice, moved in between: it went back to the first spot - $($fBack.Detail)"
  }
  Write-Host "== 5b. the second hang remembered the move =="
  Write-Host "  $($fBack.Detail)"
  Write-Host "  ok"

  # 6. across a restart: the journal is the only thing that knows where it was.
  $e = New-Fixture "E" 4
  $eBefore = Get-Rect $e
  Invoke-Wm (Get-Overlay) $script:KitHang ([IntPtr]::Zero) ([IntPtr]$e)
  if (-not (Wait-Until ({ Test-Parked $e }) 6000)) { Fail "case 6: the window never went up to the line" }
  Stop-Process -Id $proc.Id -Force
  Start-Sleep -Milliseconds 800
  $proc = Start-Shaiwin (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe") $state
  Start-Sleep -Milliseconds 2500
  $overlay = Get-Overlay
  if (-not (Wait-Until ({ (Get-Record $journal $e) }) 6000)) { Fail "case 6: the restart lost the record" }
  Invoke-Wm $overlay $script:KitTakeOne ([IntPtr]::Zero) ([IntPtr]$e)
  if (-not (Wait-Until ({ Test-OnDesktop $e }) 6000)) { Fail "case 6: the window never came down after the restart" }
  Start-Sleep -Milliseconds 600
  $eBack = Compare-Rect $eBefore $e
  if ($eBack.D -gt 4) {
    Fail "6. across a restart: came back somewhere else - $($eBack.Detail)"
  }
  Write-Host "== 6. across a restart =="
  Write-Host "  $($eBack.Detail)"
  Write-Host "  ok"

  # 7. a window dragged onto the line comes back in the middle of the screen the
  #    line is on, not where it was grabbed - that is the difference between
  #    "put it away" and "hang it up by hand". It has to survive a restart too,
  #    because the journal is what remembers the choice.
  if ($canPointer) {
    Write-Host "== 7. dragged onto the line, comes back centred =="
    $g = New-Fixture "G" 6
    $gBefore = Get-Rect $g
    if (-not $gBefore) { Fail "7. the fixture has no window to drag" }
    $work = Get-WorkArea
    if (-not $work) { Fail "7. the work area could not be read" }
    $centre = Get-Center $work
    Write-Host ("  grabbed at {0},{1} {2}x{3}, work area centre {4},{5}" -f `
      $gBefore.Left, $gBefore.Top, ($gBefore.Right - $gBefore.Left), ($gBefore.Bottom - $gBefore.Top), $centre.X, $centre.Y)

    $cursorHome = New-Object W+POINT
    [void][W]::GetCursorPos([ref]$cursorHome)
    [void](Drag-ToLine $g)
    if (-not (Wait-Until ({ Test-Parked $g }) 6000)) {
      Fail "7. letting go in the band did not hang the window up by the drag"
    }
    [void][W]::SetCursorPos($cursorHome.X, $cursorHome.Y)
    if (-not (Wait-Until ({ Get-Record $journal $g }) 4000)) { Fail "7. no journal record after the drag" }
    if (-not (Get-Record $journal $g).Centred) {
      Fail "7. a window hung up by dragging was not recorded as centred"
    }
    Write-Host "  hung up by the drag"

    # The choice lives in the journal, so a restart must not turn a centred
    # window back into one that reopens where it was grabbed.
    Stop-Process -Id $proc.Id -Force
    Start-Sleep -Milliseconds 800
    $proc = Start-Shaiwin (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe") $state
    Start-Sleep -Milliseconds 2500
    $overlay = Get-Overlay
    if (-not (Wait-Until ({ (Get-Record $journal $g) }) 6000)) { Fail "7. the restart lost the record" }
    Invoke-Wm $overlay $script:KitTakeOne ([IntPtr]::Zero) ([IntPtr]$g)
    if (-not (Wait-Until ({ Test-OnDesktop $g }) 6000)) { Fail "7. the dragged window never came back down" }
    Start-Sleep -Milliseconds 600

    $gBack = Get-Rect $g
    if (-not $gBack) { Fail "7. the dragged window has no rectangle" }
    $got = Get-Center $gBack
    $offX = [Math]::Abs($got.X - $centre.X)
    $offY = [Math]::Abs($got.Y - $centre.Y)
    $dw = [Math]::Abs(($gBack.Right - $gBack.Left) - ($gBefore.Right - $gBefore.Left))
    $dh = [Math]::Abs(($gBack.Bottom - $gBack.Top) - ($gBefore.Bottom - $gBefore.Top))
    Write-Host ("  came back at {0},{1} {2}x{3}, centre {4},{5} - off centre by {6},{7} px" -f `
      $gBack.Left, $gBack.Top, ($gBack.Right - $gBack.Left), ($gBack.Bottom - $gBack.Top), $got.X, $got.Y, $offX, $offY)
    if ($offX -gt 10 -or $offY -gt 10) {
      Fail "7. a dragged window came back off centre by $offX,$offY px - wanted the work area centre $($centre.X),$($centre.Y)"
    }
    if ($dw -gt 4 -or $dh -gt 4) {
      Fail "7. centring changed the window's size (off by $dw,$dh px)"
    }
    if ([W]::IsZoomed($g)) { Fail "7. a plain dragged window came back maximized" }
    Write-Host "  ok"

    # 7b. the same window, but hooked by hotkey first. Dragging the window back
    #     onto the line is a new decision, so it has to come back centred even
    #     though its record was written by the hotkey path.
    Write-Host "== 7b. a hotkey-hung window dragged back up comes back centred =="
    $hb = New-Fixture "H" 7
    Invoke-Wm (Get-Overlay) $script:KitHang ([IntPtr]::Zero) ([IntPtr]$hb)
    if (-not (Wait-Until ({ Test-Parked $hb }) 6000)) { Fail "7b. the fixture never went up by hotkey" }
    Invoke-Wm (Get-Overlay) $script:KitTakeOne ([IntPtr]::Zero) ([IntPtr]$hb)
    if (-not (Wait-Until ({ Test-OnDesktop $hb }) 6000)) { Fail "7b. the hotkey-hung window never came back down" }
    Start-Sleep -Milliseconds 600
    if ((Get-Record $journal $hb).Centred) { Fail "7b. the hotkey hang was already centred - the test lost its premise" }

    # Park it somewhere clear of the line and of the screen edges: a later
    # fixture index lands mostly below the screen on a scaled display, and the
    # title bar has to be on screen for the drag to reach it.
    [void][W]::SetWindowPos($hb, [IntPtr]::Zero, 300, 400, 690, 480, 0x0010)
    Start-Sleep -Milliseconds 500
    $beforeH = Get-Rect $hb
    [void](Drag-ToLine $hb)
    if (-not (Wait-Until ({ Test-Parked $hb }) 6000)) { Fail "7b. dragging the hooked window back up did not hang it" }
    [void][W]::SetCursorPos($cursorHome.X, $cursorHome.Y)
    if (-not (Wait-Until ({ (Get-Record $journal $hb).Centred }) 4000)) {
      Fail "7b. a hotkey-hung window dragged back onto the line was not recorded as centred"
    }

    Invoke-Wm (Get-Overlay) $script:KitTakeOne ([IntPtr]::Zero) ([IntPtr]$hb)
    if (-not (Wait-Until ({ Test-OnDesktop $hb }) 6000)) { Fail "7b. the re-hung window never came back down" }
    Start-Sleep -Milliseconds 600
    $hBack = Get-Rect $hb
    $hGot = Get-Center $hBack
    $hOffX = [Math]::Abs($hGot.X - $centre.X)
    $hOffY = [Math]::Abs($hGot.Y - $centre.Y)
    Write-Host ("  grabbed at {0},{1}, came back at {2},{3} - off centre by {4},{5} px" -f `
      $beforeH.Left, $beforeH.Top, $hBack.Left, $hBack.Top, $hOffX, $hOffY)
    if ($hOffX -gt 10 -or $hOffY -gt 10) {
      Fail ("7b. dragging a hooked window back onto the line left it at {0},{1} instead of the centre {2},{3}" -f `
        $hBack.Left, $hBack.Top, $centre.X, $centre.Y)
    }
    Write-Host "  ok"
  } else {
    Write-Host "== 7. SKIPPED: this session has no pointer to move =="
  }

  Write-Host "== 8. take everything down =="
  Invoke-Wm (Get-Overlay) $script:KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero)
  if (-not (Wait-Until ({ -not (Test-Path $journal) }) 8000)) { Fail "the journal survived taking everything down" }
  Write-Host "PASS"
} finally {
  Stop-ShaiwinWithEverythingDown
  Remove-Fixtures
  Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
}
