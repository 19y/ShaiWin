# Regression test for the card layer's place in the z-order (Windows).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\zorder-test.ps1
#
# The line keeps the top of the screen only while the thing the pointer is on is
# part of it, and the rope is the only handle that brings a demoted layer back.
# The pointer is fed to the app with the WM_POINT_AT hook, so the test does not
# need a session that lets it move the real cursor. Running in resident mode is
# the point: the line never tucks itself away here, so the only thing that can
# stop the cards covering the user's work is the layer giving the top back.
# Covers:
#   1. a hung card does not hold the top while the pointer is off the line
#   2. moving onto his work window (no click needed: the pointer leaving the
#      line is the whole rule) puts the cards behind it — behind, not merely
#      "not topmost", which leaves it at the head of the normal band and still
#      over the window that was just reached — while the rope's separate
#      click-through layer stays above it so the line is still visible
#   3. resting the pointer where a card used to be does NOT bring the cards
#      back: that strip is as tall as the cards and sits where his windows are
#   4. a card the user can actually see IS a handle: hovering it brings the
#      line up and takes the click-through bit off, without reaching for the
#      rope first (the reported "clicks on a card do nothing" case)
#   5. moving the pointer onto the rope does bring them back
#   6. once they are up, staying on a card keeps them up
#   7. leaving again sinks them again
#   8. the grab band around the rope is wider than the stroke but bounded: a
#      point 2x the band away does not raise, half a band off the rope does, and
#      a point below the band - where a card's own rectangle starts - does not
#   9. a window of his being dragged never brings the cards up, not even while
#      the pointer rests on the rope or on a visible card
#   10. Escape puts a resident line away as well
#
# It uses its own LOCALAPPDATA so it can never touch the real journal, and it
# takes any running instance's windows down before stopping it.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe")
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$root = Join-Path $env:TEMP "shaiwin-zorder"
# Resident, because a line that hides itself would answer the wrong question.
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=1`nexcluded=`n"
Initialize-Fixtures $root

$shotDir = Join-Path $env:TEMP "shaiwin-zorder-shots"
New-Item -ItemType Directory -Force -Path $shotDir | Out-Null
Get-ChildItem $shotDir -Filter "hits-*.txt" -ErrorAction SilentlyContinue | Remove-Item -Force
$env:SHAIWIN_DUMP_DIR = $shotDir

Stop-ShaiwinWithEverythingDown

$proc = Start-Shaiwin $Exe $state
Start-Sleep -Milliseconds 1400

function Toggle([IntPtr]$h) { Invoke-Wm $h $KitHotkey ([IntPtr]2) ([IntPtr]::Zero) }
function Get-Hits([IntPtr]$overlay) { return @(Get-CardRects $shotDir $proc.Id $overlay) }
function Test-Cards([IntPtr]$overlay) {
  return @(Get-Hits $overlay | Where-Object { -not $_.Peek }).Count
}

try {
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay window was not created" }
  $rope = Get-Rope
  if (-not (Test-Window $rope)) { Fail "the rope layer was not created" }
  Write-Host "shaiwin pid $($proc.Id), overlay $overlay, rope $rope"

  Write-Host "== 0. a window hung from the shortcut does not hold the top =="
  $a = New-Fixture "A" 0
  $b = New-Fixture "B" 1
  Invoke-Wm $overlay $KitHang ([IntPtr]::Zero) ([IntPtr]$a)
  if (-not (Wait-Until ({ (Test-Cards $overlay) -ge 1 }) 8000)) {
    Fail "the window did not come up as a card"
  }

  $v = Get-VirtualBounds
  $strip = Get-Rect $overlay
  $cards = @(Get-Hits $overlay | Where-Object { -not $_.Peek })
  # A card's clickable rectangle starts at its clothespin, which is on the rope,
  # so the top edge of the card IS the rope.
  $cardX = [int]($strip.Left + ($cards[0].Left + $cards[0].Right) / 2)
  $cardY = [int]($strip.Top + ($cards[0].Top + $cards[0].Bottom) / 2)
  $ropeX = $cardX
  $ropeY = [int]($strip.Top + $cards[0].Top)
  # Design sizes are logical pixels; at 150% the band and gaps are 1.5x wider.
  $scale = Get-WindowScale $overlay

  # The window he works in, put behind the cards without being activated: getting
  # it into place must not itself be the event under test. It has to cover the
  # card's own centre, or "the place a card used to be" is not covered at all on
  # a wide screen and the tests below would be asking a different question.
  $workX = [int]($v.Left + 60)
  $workY = [int]($strip.Top + 2)
  $workW = [int]([Math]::Max(1200, $cardX - $workX + 400))
  [void][W]::SetWindowPos($b, [IntPtr]::Zero, $workX, $workY, $workW, 700, 0x10 -bor 0x40)
  Start-Sleep -Milliseconds 400
  $workPointX = $workX + 500
  $workPointY = $workY + 300

  # The pointer is wherever the last run left it; claim it is on his window.
  Point-At $overlay $workPointX $workPointY
  if (-not (Wait-Until ({ -not [W]::IsTopmost($overlay) }) 2500)) {
    Fail "hanging a card left the cards on top while the pointer was off the line"
  }
  if (-not (Wait-Until ({ [W]::IsWindowVisible($rope) -and [W]::IsTopmost($rope) }) 2000)) {
    Fail "the rope layer is not marking the line while the cards are down"
  }
  Write-Host "  the hang left the cards down and the rope layer marks the line"

  Write-Host "== 1. his work window is in front of the cards =="
  if (-not (Wait-Until ({ -not [W]::IsAbove($overlay, $b) }) 2500)) {
    Fail "the cards stayed painted over the window he moved to"
  }
  if (-not [W]::IsWindowVisible($overlay)) {
    Fail "the resident line hid itself; only its place in the z-order should change"
  }
  if (-not [W]::IsAbove($rope, $b)) {
    Fail "the rope was left behind the window he went to"
  }
  Write-Host "  the cards are behind his window, and the rope still marks the line above it"

  Write-Host "== 2. the place a card used to be is not a handle =="
  Point-At $overlay $cardX $cardY
  Start-Sleep -Milliseconds 700
  if ([W]::IsTopmost($overlay) -or [W]::IsAbove($overlay, $b)) {
    Fail "resting on a card's old position pulled the cards back over his work"
  }
  Write-Host "  it stayed behind his window after 0.7 s of resting there"

  Write-Host "== 2b. a card he can actually see is a handle =="
  # Move his window off the cards first, so the raise below can only be the
  # card: the pointer is on the bare desktop while the window moves.
  Point-At $overlay $workPointX $workPointY
  [void][W]::SetWindowPos($b, [IntPtr]::Zero, [int]($v.Right - 320), $workY, 320, 500, 0x10 -bor 0x40)
  Start-Sleep -Milliseconds 400
  if ([W]::IsTopmost($overlay)) {
    Fail "moving his window off the card raised the cards by itself"
  }
  # "See" is the app's word: a card is a handle only where nothing is painted
  # over it. In a session with no composited desktop the input host is a
  # full-screen CoreWindow that DWM does not cloak, and every point on the line
  # is behind it - the raise cannot be observed there, and the app is right.
  $cover = Get-Coverer $overlay $cardX $cardY
  Point-At $overlay $cardX $cardY
  if ($cover) {
    Write-Host "  SKIPPED raise: '$cover' covers the card's place in this session"
  } elseif (-not (Wait-Until ({ [W]::IsTopmost($overlay) -and [W]::IsAbove($overlay, $b) }) 2000)) {
    Fail "resting on a card he can see did not bring the line up; the card is only a place to look at"
  } else {
    Write-Host "  the visible card raised the line"
  }
  if (-not (Wait-Until ({ -not [W]::IsClickThrough($overlay) }) 1500)) {
    Fail "the card is still click-through: a click or right-click on it would go straight to the desktop"
  }
  Write-Host "  a card under the pointer takes clicks"

  Point-At $overlay $workPointX $workPointY
  if (-not (Wait-Until ({ -not [W]::IsTopmost($overlay) }) 2500)) {
    Fail "the cards stayed up after the pointer left the line"
  }
  # Put his window back where the rest of the test expects it.
  [void][W]::SetWindowPos($b, [IntPtr]::Zero, $workX, $workY, $workW, 700, 0x10 -bor 0x40)
  Start-Sleep -Milliseconds 400
  Write-Host "  leaving sank them again; his window is back over the card's place"

  Write-Host "== 3. the rope brings the cards back =="
  Point-At $overlay $ropeX $ropeY
  if (-not (Wait-Until ({ [W]::IsTopmost($overlay) -and [W]::IsAbove($overlay, $b) }) 2000)) {
    Fail "resting on the rope did not bring the cards back to the top"
  }
  if (-not (Wait-Until ({ -not [W]::IsWindowVisible($rope) }) 1500)) {
    Fail "the rope layer stayed visible over the cards that have the top again"
  }
  Write-Host "  the rope raised the cards and stepped out of the way"

  Write-Host "== 4. once up, staying on a card keeps them up =="
  Point-At $overlay $cardX $cardY
  Start-Sleep -Milliseconds 700
  if (-not ([W]::IsTopmost($overlay) -and [W]::IsAbove($overlay, $b))) {
    Fail "the cards sank while the pointer was resting on one of them"
  }
  Write-Host "  the line stayed up while it was being used"

  Write-Host "== 5. leaving again sinks them again =="
  Point-At $overlay $workPointX $workPointY
  if (-not (Wait-Until ({ -not [W]::IsAbove($overlay, $b) }) 2500)) {
    Fail "the cards did not sink when the pointer went back to his window"
  }
  if (-not (Wait-Until ({ [W]::IsWindowVisible($rope) -and [W]::IsTopmost($rope) }) 2000)) {
    Fail "the rope layer did not come back when the cards sank"
  }
  Write-Host "  behind his window again, rope on top"

  Write-Host "== 6. the grab band is wider than the rope, but bounded =="
  # 14 logical px each way (core::layout::ROPE_REACH). The stroke is a couple of
  # pixels wide; a band the width of the drawing is what made reaching for the
  # rope feel like threading a needle. The band has to stay above the card tops,
  # because below the rope is the strip where the user's windows live.
  $reach = 14.0 * $scale
  $far = [int]($ropeY - $reach * 2)
  $half = [int]($ropeY - $reach * 0.5)
  $below = [int]($ropeY + $reach * 1.5)
  $halfBelow = [int]($ropeY + $reach * 0.5)

  Point-At $overlay $cardX $far
  Start-Sleep -Milliseconds 700
  if ([W]::IsTopmost($overlay) -or [W]::IsAbove($overlay, $b)) {
    Fail "a point $([int]($reach * 2)) px above the rope raised the cards; the band is too wide"
  }
  Write-Host "  $([int]($reach * 2)) px above the rope is cold"

  Point-At $overlay $cardX $half
  if (-not (Wait-Until ({ [W]::IsTopmost($overlay) -and [W]::IsAbove($overlay, $b) }) 2000)) {
    Fail "half a band above the rope did not raise the cards; the band is still too thin"
  }
  Write-Host "  half a band above the rope raises"

  Point-At $overlay $workPointX $workPointY
  if (-not (Wait-Until ({ -not [W]::IsTopmost($overlay) }) 2500)) {
    Fail "the cards did not sink again before the band's lower edge was tested"
  }

  Point-At $overlay $cardX $below
  Start-Sleep -Milliseconds 700
  if ([W]::IsTopmost($overlay) -or [W]::IsAbove($overlay, $b)) {
    Fail "a point below the band pulled the cards back; that height belongs to the cards' strip, not the rope"
  }
  Write-Host "  just below the band, over a card's own place, is cold"

  Point-At $overlay $cardX $halfBelow
  if (-not (Wait-Until ({ [W]::IsTopmost($overlay) -and [W]::IsAbove($overlay, $b) }) 2000)) {
    Fail "the lower half of the grab band did not raise the cards"
  }
  Write-Host "  the band reaches below the rope too"

  Write-Host "== 7. a window being dragged never brings the cards up =="
  # His own drag outranks the line's claim on the top. The rope would normally
  # raise the cards after the dwell; it must not do it while a window is being
  # moved past, and neither must a card the pointer happens to cross. A session
  # with no input desktop cannot start a real move loop, so the drag is fed to
  # the poll directly: the rule is the poll's, not the loop's.
  Point-At $overlay $workPointX $workPointY
  if (-not (Wait-Until ({ -not [W]::IsTopmost($overlay) }) 2500)) {
    Fail "the cards did not go down before the drag test"
  }
  Drag-Pretend $overlay $true
  Point-At $overlay $ropeX $ropeY
  Start-Sleep -Milliseconds 900   # several RAISE_DWELLs
  if ([W]::IsTopmost($overlay) -or [W]::IsAbove($overlay, $b)) {
    Fail "resting on the rope during a drag brought the cards up over the window being moved"
  }
  Write-Host "  the rope stayed cold for 0.9 s while a drag was underway"

  Drag-Pretend $overlay $false
  if (-not (Wait-Until ({ [W]::IsTopmost($overlay) -and [W]::IsAbove($overlay, $b) }) 2000)) {
    Fail "the rope did not raise the cards once the drag ended"
  }
  Write-Host "  ending the drag let the rope raise them again"

  # The same with a card he can see: no wall of cards over the window he is
  # moving, whatever the pointer crosses on the way.
  Point-At $overlay $workPointX $workPointY
  if (-not (Wait-Until ({ -not [W]::IsTopmost($overlay) }) 2500)) {
    Fail "the cards did not sink before the second drag case"
  }
  [void][W]::SetWindowPos($b, [IntPtr]::Zero, [int]($v.Right - 320), $workY, 320, 500, 0x10 -bor 0x40)
  Start-Sleep -Milliseconds 400
  Drag-Pretend $overlay $true
  Point-At $overlay $cardX $cardY
  Start-Sleep -Milliseconds 700
  if ([W]::IsTopmost($overlay) -or [W]::IsAbove($overlay, $b)) {
    Fail "a visible card brought the cards up in the middle of a drag"
  }
  Write-Host "  the card stayed cold as well while a drag was underway"

  Drag-Pretend $overlay $false
  if ($cover) {
    Write-Host "  SKIPPED raise: '$cover' still covers the card's place in this session"
  } elseif (-not (Wait-Until ({ [W]::IsTopmost($overlay) }) 2000)) {
    Fail "the card did not raise the line after the drag ended"
  } else {
    Write-Host "  the card raised the line once the drag ended"
  }
  [void][W]::SetWindowPos($b, [IntPtr]::Zero, $workX, $workY, $workW, 700, 0x10 -bor 0x40)
  Start-Sleep -Milliseconds 400

  Point-Real $overlay

  Write-Host "== 8. Escape puts a resident line away =="
  if ([W]::GetForegroundWindow() -eq [IntPtr]::Zero) {
    # Keys need somewhere to go. A session with no foreground window (a runner
    # with no input desktop) drops them, and that is the harness, not the app.
    Write-Host "  SKIPPED: this session has no foreground window"
  } else {
    [W]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero)      # ESC down
    Start-Sleep -Milliseconds 60
    [W]::keybd_event(0x1B, 0, 2, [UIntPtr]::Zero)      # ESC up
    # In resident mode "away" is the strip hiding itself - there is no pill to fall
    # back to. This waits on a window state rather than on a frame dump, because a
    # line that is on its way out answers a dump whenever the animation feels like
    # it, and that is the app's instrumentation, not his promise.
    if (-not (Wait-Until ({ -not [W]::IsWindowVisible($overlay) }) 2500)) {
      Fail "Escape left the line out"
    }
    Write-Host "  away"
  }

  Invoke-Wm $overlay $KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero)
  Start-Sleep -Milliseconds 500
  Write-Host "PASS"
} finally {
  Stop-ShaiwinWithEverythingDown
  Remove-Fixtures
  Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "cleaned up"
}
