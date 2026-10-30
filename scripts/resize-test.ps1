# Regression test for resizing and moving the cards on the line (Windows).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\resize-test.ps1
#
# The line holds more windows than its width allows, so a card's size and its
# place are the user's to set:
#   1. dragging a card's side edge makes it wider, and it stays that width
#   1c. dragging its bottom edge makes it taller without moving its top
#   2. the sizes are remembered: they survive a restart
#   3. dragging a card's face past its neighbour reorders the line, and the card
#      stays on the far side instead of springing back
#   4. two cards never overlap after either move, so both stay clickable
#
# The pointer has to be movable for 1, 1c and 3; without one the test still
# checks that the records survive a restart.
#
# Fixtures and a private LOCALAPPDATA, as with the other tests: nothing the user
# was working on is moved, closed or clicked.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe")
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$root = Join-Path $env:TEMP "shaiwin-resize"
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=1`nexcluded=`n"
Initialize-Fixtures $root
$journal = Join-Path $state "ShaiWin\parked.txt"

$shotDir = Join-Path $env:TEMP "shaiwin-resize-shots"
New-Item -ItemType Directory -Force -Path $shotDir | Out-Null
$env:SHAIWIN_DUMP_DIR = $shotDir

Stop-ShaiwinWithEverythingDown

$script:proc = $null
$canPointer = Test-CursorControl

function Order { @(Read-Journal $journal | ForEach-Object { $_.Hwnd }) }
function Rects([IntPtr]$overlay) {
  $r = Get-Rect $overlay
  @(Get-CardRects $shotDir $script:proc.Id $overlay | ForEach-Object {
    [pscustomobject]@{
      Hwnd = $_.Hwnd
      Left = $_.Left + $r.Left; Top = $_.Top + $r.Top
      Right = $_.Right + $r.Left; Bottom = $_.Bottom + $r.Top
      Width = $_.Right - $_.Left
      Height = $_.Bottom - $_.Top
    }
  })
}
function Rect-For($rects, [IntPtr]$h) { $rects | Where-Object { $_.Hwnd -eq [long]$h } | Select-Object -First 1 }
function Hang([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitHang ([IntPtr]::Zero) ([IntPtr]$hwnd) }
# Pump a point of the screen that is nowhere near the line, so hover is off and
# the rectangles reported are the cards' resting rectangles.
function Park-Cursor {
  $v = Get-VirtualBounds
  [void](Move-Cursor ([int](($v.Left + $v.Right) / 2)) ([int]($v.Bottom - 80)))
}

try {
  Write-Host "== 0. start and hang two windows =="
  $script:proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2000
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay window was not created" }

  $a = New-Fixture "A" 0
  $b = New-Fixture "B" 1
  Hang $a; Hang $b
  if (-not (Wait-Until ({ @(Order).Count -eq 2 }) 6000)) { Fail "two windows did not become two records" }
  if (-not (Wait-Until ({ @(Rects $overlay).Count -eq 2 }) 6000)) { Fail "two cards were not drawn" }
  Park-Cursor
  Start-Sleep -Milliseconds 500

  $before = Rects $overlay
  $a0 = Rect-For $before $a
  $b0 = Rect-For $before $b
  if (-not $a0 -or -not $b0) { Fail "the two cards are not both on the line" }
  Write-Host "  A $($a0.Left),$($a0.Top) $($a0.Width)x$($a0.Height); B $($b0.Left),$($b0.Top) $($b0.Width)x$($b0.Height)"

  if ($canPointer) {
    Write-Host "== 1. drag A's left edge outwards =="
    $grabY = [int](($a0.Top + $a0.Bottom) / 2)
    $from = $a0.Left + 4
    $to = $a0.Left - 90
    if (-not (Drag-From-To $from $grabY $to $grabY 12)) { Fail "lost cursor control during the resize" }
    Start-Sleep -Milliseconds 500
    Park-Cursor
    Start-Sleep -Milliseconds 500
    $a1 = Rect-For (Rects $overlay) $a
    if (-not $a1) { Fail "card A vanished after the resize" }
    $grew = $a1.Width - $a0.Width
    Write-Host "  A is now $($a1.Width) wide (was $($a0.Width), grew $grew)"
    if ($grew -lt 140) { Fail "dragging the side edge did not widen the card (grew $grew px)" }
    if ($a1.Left -gt $a0.Left + 6) { Fail "the card grew the wrong way - its left edge moved from $($a0.Left) to $($a1.Left)" }
    if ([Math]::Abs($a1.Height - $a0.Height) -gt 8) { Fail "the card changed height too ($($a0.Height) -> $($a1.Height))" }

    Write-Host "== 1b. the other card made room instead of being covered =="
    $b1 = Rect-For (Rects $overlay) $b
    if (-not $b1) { Fail "card B vanished after A was widened" }
    if ($a1.Right -gt $b1.Left + 2) { Fail "the widened card covers B: A ends at $($a1.Right), B starts at $($b1.Left)" }

    Write-Host "== 1c. drag A's bottom edge down =="
    $dragX = [int](($a1.Left + $a1.Right) / 2)
    $fromY = $a1.Bottom - 4
    if (-not (Drag-From-To $dragX $fromY $dragX ($fromY + 70) 10)) { Fail "lost cursor control during the height resize" }
    Start-Sleep -Milliseconds 500
    Park-Cursor
    Start-Sleep -Milliseconds 500
    $aH = Rect-For (Rects $overlay) $a
    if (-not $aH) { Fail "card A vanished after the height resize" }
    $taller = $aH.Height - $a1.Height
    Write-Host "  A is now $($aH.Width)x$($aH.Height) (height grew $taller)"
    if ($taller -lt 40) { Fail "dragging the bottom edge did not make the card taller (grew $taller px)" }
    if ([Math]::Abs($aH.Width - $a1.Width) -gt 8) { Fail "the height drag changed the width too ($($a1.Width) -> $($aH.Width))" }
    if ([Math]::Abs($aH.Top - $a1.Top) -gt 6) { Fail "the card's top moved during a height resize ($($a1.Top) -> $($aH.Top))" }
    $zoomY = [double](Get-Record $journal $a).Fields[15]
    Write-Host "  recorded height zoom: $zoomY"
    if ($zoomY -lt 1.1) { Fail "the journal did not keep the new height (zoom_y $zoomY)" }
  } else {
    Write-Host "== 1. SKIPPED: this session has no pointer to move =="
    Write-Host "== 1c. the bottom edge, driven by window messages =="
    # The same press, moves and release the window manager would deliver, in
    # panel coordinates. It cannot test the hit-test or the capture; what it
    # does test is the height maths, the growing panel and the journal.
    $cards = @(Get-CardRects $shotDir $script:proc.Id $overlay)
    $ca = $cards | Where-Object { $_.Hwnd -eq [long]$a } | Select-Object -First 1
    if (-not $ca) { Fail "card A has no clickable rectangle" }
    $midX = [int](($ca.Left + $ca.Right) / 2)
    $fromY = $ca.Bottom - 3
    Drag-By-Messages $overlay $midX $fromY $midX ($fromY + 70) 8
    Start-Sleep -Milliseconds 400
    Park-Cursor
    Start-Sleep -Milliseconds 400
    $aH = Rect-For (Rects $overlay) $a
    if (-not $aH) { Fail "card A vanished after the height resize" }
    $taller = $aH.Height - $a0.Height
    Write-Host "  A is now $($aH.Width)x$($aH.Height) (height grew $taller)"
    if ($taller -lt 40) { Fail "the bottom edge did not make the card taller (grew $taller px)" }
    if ([Math]::Abs($aH.Width - $a0.Width) -gt 8) { Fail "the height drag changed the width too ($($a0.Width) -> $($aH.Width))" }
    if ([Math]::Abs($aH.Top - $a0.Top) -gt 6) { Fail "the card's top moved during a height resize ($($a0.Top) -> $($aH.Top))" }
    $zoomY = [double](Get-Record $journal $a).Fields[15]
    Write-Host "  recorded height zoom: $zoomY"
    if ($zoomY -lt 1.1) { Fail "the journal did not keep the new height (zoom_y $zoomY)" }
  }

  # The cards are drawn into the overlay window itself, so a taller card is cut
  # off at the bottom unless the window grows with it.
  $panel = Get-Rect $overlay
  Write-Host "  the panel ends at $($panel.Bottom), the tall card at $($aH.Bottom)"
  if ($panel.Bottom - $aH.Bottom -lt 6) {
    Fail "the window did not grow for the taller card: it ends at $($panel.Bottom), the card at $($aH.Bottom)"
  }

  Write-Host "== 2. the size is in the journal and survives a restart =="
  $rec = Get-Record $journal $a
  if (-not $rec) { Fail "the widened card has no record" }
  $zoom = [double]$rec.Fields[13]
  Write-Host "  recorded zoom: $zoom"
  if ($canPointer -and $zoom -lt 1.1) { Fail "the journal did not keep the new size (zoom $zoom)" }

  Stop-Process -Id $script:proc.Id -Force
  Start-Sleep -Milliseconds 800
  $script:proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2200
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay did not come back after the restart" }
  if (-not (Wait-Until ({ @(Rects $overlay).Count -eq 2 }) 6000)) { Fail "the cards did not come back after the restart" }
  $after = Rects $overlay
  $a2 = Rect-For $after $a
  if (-not $a2) { Fail "card A was not recovered" }
  Write-Host "  after the restart: A is $($a2.Width)x$($a2.Height)"
  if ($canPointer) {
    if ([Math]::Abs($a2.Width - $a1.Width) -gt 12) {
      Fail "the size did not survive the restart ($($a1.Width) -> $($a2.Width))"
    }
  }
  if ($aH) {
    if ([Math]::Abs($a2.Width - $aH.Width) -gt 12) {
      Fail "the size did not survive the restart ($($aH.Width) -> $($a2.Width))"
    }
    if ([Math]::Abs($a2.Height - $aH.Height) -gt 12) {
      Fail "the height did not survive the restart ($($aH.Height) -> $($a2.Height))"
    }
  }

  if ($canPointer) {
    Write-Host "== 3. drag A's face past B =="
    $b2 = Rect-For (Rects $overlay) $b
    $grab = Get-Center $a2
    $target = $b2.Right + 240
    if (-not (Drag-From-To $grab.X $grab.Y $target ([int](($a2.Top + $a2.Bottom) / 2)) 14)) {
      Fail "lost cursor control during the move"
    }
    Start-Sleep -Milliseconds 600
    Park-Cursor
    Start-Sleep -Milliseconds 500
    $order = @(Order)
    if (-not $order -or [long]$order[0] -ne [long]$b) {
      Fail "dragging A past B did not reorder the line: journal order $($order -join ',')"
    }
    $a3 = Rect-For (Rects $overlay) $a
    $b3 = Rect-For (Rects $overlay) $b
    if (-not $a3 -or -not $b3) { Fail "a card vanished after the move" }
    Write-Host "  order now B then A; A sits at $($a3.Left), B at $($b3.Left)"
    if ($a3.Left -lt $b3.Right - 2) {
      Fail "the dragged card covers B after the drop: A starts at $($a3.Left), B ends at $($b3.Right)"
    }
    if ([Math]::Abs($a3.Width - $a2.Width) -gt 12) {
      Fail "moving the card changed its size ($($a2.Width) -> $($a3.Width))"
    }
  }

  Write-Host "== 4. take everything down =="
  Invoke-Wm (Get-Overlay) $KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero)
  if (-not (Wait-Until ({ -not (Test-Path $journal) }) 8000)) { Fail "the journal survived taking everything down" }
  Write-Host "PASS"
} finally {
  Stop-ShaiwinWithEverythingDown
  Remove-Fixtures
  Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item $shotDir -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "cleaned up"
}
