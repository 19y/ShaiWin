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
#   10. reaching a card at the end of the line works: the corridor between two
#      cards, where the pointer is on neither the rope nor a card, does not
#      demote the layer — a demoted card is behind his window and stops being a
#      handle, so sinking there sent the user back to the rope every time
#   11. the strip is not a way back up, and it is not the whole overlay window
#      either: the cards linger `SINK_GRACE` after the pointer leaves the strip
#      and then go, and a point below the cards is already his work
#   11. Escape puts a resident line away as well
#   12. pressing the bubble's button is the other answer to the sentence: the
#       layer takes the click only where the button is - not on the sentence -
#       and the press stands the explanation down for good
#   13. a run that starts already knowing never explains again
#
#  The explanation of the raise gesture is checked where it happens: it is up
#  while the cards are down (0b); taking the rope only ends that showing, so the
#  next sink puts the bubble straight back up (5); a click on the sentence is not
#  an answer at all (12); and only pressing "don't remind me" stands it down for
#  good, for this run and every later one (12, 13).
#
# It uses its own LOCALAPPDATA so it can never touch the real journal, and it
# takes any running instance's windows down before stopping it.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe")
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$root = Join-Path $env:TEMP "shaiwin-zorder"
$ackRoot = Join-Path $env:TEMP "shaiwin-zorder-ack"
# Resident, because a line that hides itself would answer the wrong question.
$resident = "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=1`nexcluded=`n"
$state = New-Sandbox $root $resident
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

  Write-Host "== 0b. the rope explains the gesture while the cards are down =="
  # The one gesture nobody guesses from looking at a line: with the cards behind
  # his window, resting on the rope is the only way back. It has to be said on
  # the rope's own layer, because that is the only layer still above his work -
  # and it has to be said where a test can see it, not merely intended.
  $hint = Get-HintRect $shotDir $proc.Id $overlay
  if (-not $hint) {
    Fail "the rope never explained the raise gesture while the cards were down"
  }
  if ($hint.Top -lt $ropeY -or ($hint.Right - $hint.Left) -lt 40) {
    Fail "the explanation is not a bubble hanging under the rope: $hint"
  }
  # The button is a place of its own inside the sentence's bubble, not the whole
  # of it: that is what makes "only the button answers a click" testable at all.
  if ($hint.Chip.Left -le $hint.Body.Left -or $hint.Chip.Right -gt $hint.Body.Right -or
      ($hint.Body.Right - $hint.Body.Left) - 2 * ($hint.Chip.Right - $hint.Chip.Left) -lt 40) {
    Fail "the bubble has no sentence beside its button: body=$($hint.Body) chip=$($hint.Chip)"
  }
  $logText = @(Get-Content (Join-Path $state "ShaiWin\shaiwin.log") -ErrorAction SilentlyContinue)
  if (@($logText | Where-Object { $_ -match "the rope explains the raise gesture" }).Count -lt 1) {
    Fail "the bubble was drawn but the log never saw it"
  }
  Write-Host "  the bubble is up under the rope, $($hint.Right - $hint.Left) x $($hint.Bottom - $hint.Top), button $(($hint.Chip.Right - $hint.Chip.Left)) x $(($hint.Chip.Bottom - $hint.Chip.Top))"

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

  # Taking the rope is the gesture, not the acknowledgement. It ends this
  # showing because the layer the bubble is drawn on goes away with the raise -
  # it does not stand the explanation down. That distinction is only visible on
  # the next sink, which is what step 5 asserts; step 12 is where the press that
  # does say "don't remind me" is tested.
  $settingsText = @(Get-Content (Join-Path $state "ShaiWin\settings.txt") -ErrorAction SilentlyContinue)
  if ($settingsText -match "^raise_known=1$") {
    Fail "taking the rope stood the explanation down for good; only the press may do that"
  }
  $logText = @(Get-Content (Join-Path $state "ShaiWin\shaiwin.log") -ErrorAction SilentlyContinue)
  if (@($logText | Where-Object { $_ -match "stands down" }).Count -ne 0) {
    Fail "the line recorded an answer the user never gave"
  }
  Write-Host "  a rope raise only ended that showing; nothing has been answered"

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

  # And it says the sentence again, right away: one bubble per sinking of the
  # cards is the whole rule, so a second sink buys a second showing without any
  # quiet stretch in between. The log is what proves the second showing rather
  # than a lucky dump - waiting on the picture alone would also be satisfied by
  # the first bubble still standing.
  $again = Wait-Until ({
    @((Get-Content (Join-Path $state "ShaiWin\shaiwin.log") -ErrorAction SilentlyContinue) |
      Where-Object { $_ -match "the rope explains the raise gesture" }).Count -ge 2
  }) 6000
  if (-not $again) {
    Fail "the line stayed quiet on the second sinking after a rope raise he never acknowledged"
  }
  if (-not (Get-HintRect $shotDir $proc.Id $overlay)) {
    Fail "the log says it explained the gesture again, but no bubble was on the rope"
  }
  Write-Host "  explained again on this sinking: he has not pressed 'don't remind me'"

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

  Write-Host "== 8. reaching for a card at the end of the line =="
  # The reported loop: sweep the rope, the cards come up, then move towards a
  # card at the end of the line. Between two neighbouring cards there is a
  # corridor as tall as a card where the pointer is on neither the rope nor a
  # card. Crossing it used to demote the layer, and a demoted card is behind the
  # window he moved to, so it stopped being a handle - the only way back was the
  # rope again.
  Point-At $overlay $workPointX $workPointY
  if (-not (Wait-Until ({ -not [W]::IsTopmost($overlay) }) 2500)) {
    Fail "the cards did not go down before the crossing test"
  }
  $c = New-Fixture "C" 2
  Invoke-Wm $overlay $KitHang ([IntPtr]::Zero) ([IntPtr]$c)
  if (-not (Wait-Until ({ (Test-Cards $overlay) -ge 2 }) 8000)) {
    Fail "the second window did not come up as a card"
  }
  $cards = @(Get-Hits $overlay | Where-Object { -not $_.Peek } | Sort-Object Left)
  $near = $cards[0]
  $farCard = $cards[-1]
  # The line has recentred around two cards, so the rope is aimed at again: the
  # clothespin is on the rope, so a card's top edge IS the rope at that x.
  $ropeX = [int]($strip.Left + ($near.Left + $near.Right) / 2)
  $ropeY = [int]($strip.Top + $near.Top)
  # The corridor between the first two cards: below the rope's grab band, and
  # well above the bottom of the cards.
  $gapX = [int]($strip.Left + ($near.Right + $cards[1].Left) / 2)
  $gapY = [int]($strip.Top + $near.Top + $reach * 3)
  Write-Host "  the corridor is at ($gapX,$gapY), between cards ending at $($near.Right) and starting at $($cards[1].Left)"

  Point-At $overlay $ropeX $ropeY
  if (-not (Wait-Until ({ [W]::IsTopmost($overlay) }) 2000)) {
    Fail "the rope did not raise the cards for the crossing test"
  }
  Point-At $overlay $gapX $gapY
  Start-Sleep -Milliseconds 900     # longer than SINK_GRACE
  if (-not [W]::IsTopmost($overlay)) {
    Fail "crossing the corridor between two cards sank them; the card at the end is unreachable without going back to the rope"
  }
  Write-Host "  the cards stayed up across the corridor for 0.9 s"

  $farX = [int]($strip.Left + ($farCard.Left + $farCard.Right) / 2)
  $farY = [int]($strip.Top + $farCard.Top + ($farCard.Bottom - $farCard.Top) * 0.6)
  Point-At $overlay $farX $farY
  Start-Sleep -Milliseconds 400
  if (-not [W]::IsTopmost($overlay)) {
    Fail "the card at the end of the line was not reachable from the corridor"
  }
  if ([W]::IsClickThrough($overlay)) {
    Fail "the card at the end of the line does not take clicks after the crossing"
  }
  Write-Host "  the far card is reachable and clickable without reaching for the rope again"

  Write-Host "== 9. the cards linger, then go =="
  # The grace is what makes a crossing that strays out of the strip and back
  # survivable. It is short on purpose: the cards must not stay over his work,
  # and a click on it sinks them at once.
  Point-At $overlay $workPointX $workPointY
  Start-Sleep -Milliseconds 300
  if (-not [W]::IsTopmost($overlay)) {
    Fail "the cards sank at once; a crossing that strays out of the strip has nothing to come back to"
  }
  if (-not (Wait-Until ({ -not [W]::IsTopmost($overlay) }) 2500)) {
    Fail "the cards stayed up over his work well past the grace"
  }
  Write-Host "  still up at 0.3 s, behind his window by 2.5 s"

  Write-Host "== 10. the floor of the strip is the bottom of the cards =="
  # The overlay window is taller than the cards. The room under them belongs to
  # his windows, so a point there must be cold even while the cards are up.
  $underCards = [int]($strip.Top + $farCard.Bottom + ($strip.Bottom - $strip.Top - $farCard.Bottom) / 2)
  Point-At $overlay $ropeX $ropeY
  if (-not (Wait-Until ({ [W]::IsTopmost($overlay) }) 2000)) {
    Fail "the rope did not raise the cards for the floor test"
  }
  Point-At $overlay $farX $underCards
  if (-not (Wait-Until ({ -not [W]::IsTopmost($overlay) }) 2500)) {
    Fail "the room under the cards kept them up; that height belongs to his windows"
  }
  Write-Host "  a point under the cards is cold"

  Point-Real $overlay

  Write-Host "== 11. Escape puts a resident line away =="
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

  Write-Host "== 12. pressing the bubble's button is the other answer =="
  # Taking the rope is one proof the sentence landed; pressing "don't remind me"
  # is the other, and it is a click on a layer that is click-through everywhere
  # else. Both halves of that click have to be asked: where the window proc would
  # take a real one, and what the press actually does. The bubble's own body is
  # deliberately not a target - a click that lands on the sentence is a click on
  # his work, because turning a stray click into "he read it" is how a prompt
  # retires itself before anyone has. It needs a run of its own - by now this
  # one's settings already say the user knows.
  Stop-ShaiwinWithEverythingDown
  $ackState = New-Sandbox $ackRoot $resident
  $proc = Start-Shaiwin $Exe $ackState
  Start-Sleep -Milliseconds 1400
  $overlay = Get-Overlay
  $rope = Get-Rope
  if (-not (Test-Window $overlay)) { Fail "the restarted line was not created" }
  if (-not (Test-Window $rope)) { Fail "the restarted line has no rope layer" }
  Invoke-Wm $overlay $KitHang ([IntPtr]::Zero) ([IntPtr]$a)
  if (-not (Wait-Until ({ (Test-Cards $overlay) -ge 1 }) 8000)) {
    Fail "the restarted line did not take the window"
  }
  Point-At $overlay $workPointX $workPointY
  if (-not (Wait-Until ({ -not [W]::IsTopmost($overlay) }) 2500)) {
    Fail "the restarted line kept the top while the pointer was on his window"
  }
  $hint = Get-HintRect $shotDir $proc.Id $overlay
  if (-not $hint) { Fail "the restarted line never explained the gesture" }
  $strip = Get-Rect $overlay
  $chipX = [int]($strip.Left + ($hint.Chip.Left + $hint.Chip.Right) / 2)
  $chipY = [int]($strip.Top + ($hint.Chip.Top + $hint.Chip.Bottom) / 2)
  # The sentence's own half of the bubble: inside the body, clear of the button.
  $bodyX = [int]($strip.Left + $hint.Body.Left + 6)
  $bodyY = [int]($strip.Top + ($hint.Body.Top + $hint.Body.Bottom) / 2)
  if ($bodyX -ge ($strip.Left + $hint.Chip.Left)) {
    Fail "the button is the whole bubble, so the sentence has no place of its own: $hint"
  }

  # One button wide, and nothing else: the rope layer is the strip, so a hit-test
  # that said HTCLIENT anywhere would eat a click meant for the top of his work -
  # including the sentence, which is the part he only has to read.
  $bareX = [int]($strip.Left + 8)
  $bareY = [int]($strip.Top + 2)
  if ((Get-HitTest $rope $chipX $chipY) -ne 1) {
    Fail "a click on the button would not reach it; the rope layer answered HTTRANSPARENT"
  }
  if ((Get-HitTest $rope $bodyX $bodyY) -ne -1) {
    Fail "the rope layer would take a click on the sentence, which is not a button"
  }
  if ((Get-HitTest $rope $bareX $bareY) -ne -1) {
    Fail "the rope layer would take a click somewhere other than its button"
  }
  # The style bit has to agree with the hit-test, or the click is gone before the
  # window proc ever sees it. Over the sentence the layer stays transparent; over
  # the button it stops being so.
  Point-At $overlay $bodyX $bodyY
  if (-not (Wait-Until ({ [W]::IsClickThrough($rope) }) 1500)) {
    Fail "the layer stopped letting clicks through with the pointer on the sentence"
  }
  Point-At $overlay $chipX $chipY
  if (-not (Wait-Until ({ -not [W]::IsClickThrough($rope) }) 1500)) {
    Fail "the rope layer stayed click-through with the pointer on its button"
  }
  Write-Host "  the button is the one place on the rope layer that takes a click"

  $ropeRect = Get-Rect $rope
  # A click on the sentence first: it is his work's click, and it answers nothing.
  Post-Mouse $rope $KitMouseUp ($bodyX - $ropeRect.Left) ($bodyY - $ropeRect.Top)
  Start-Sleep -Milliseconds 500
  if (@((Get-Content (Join-Path $ackState "ShaiWin\settings.txt") -ErrorAction SilentlyContinue) |
        Where-Object { $_ -match "^raise_known=1$" }).Count -ne 0) {
    Fail "a click on the sentence stood the explanation down"
  }
  Point-At $overlay $chipX $chipY
  if (-not (Get-HintRect $shotDir $proc.Id $overlay)) {
    Fail "the bubble went away on a click that was not its button"
  }

  # Then the press that is its button, in the rope window's own client pixels.
  Post-Mouse $rope $KitMouseUp ($chipX - $ropeRect.Left) ($chipY - $ropeRect.Top)
  # Waiting on the settings the press writes, not on the log line: the message is
  # posted, so the app has not run the handler yet when this reads.
  if (-not (Wait-Until ({
        @((Get-Content (Join-Path $ackState "ShaiWin\settings.txt") -ErrorAction SilentlyContinue) |
          Where-Object { $_ -match "^raise_known=1$" }).Count -eq 1
      }) 3000)) {
    Write-Host "  last lines of the log:"
    Get-Content (Join-Path $ackState "ShaiWin\shaiwin.log") -Tail 10 -ErrorAction SilentlyContinue |
      ForEach-Object { Write-Host "    $_" }
    Fail "pressing the button did not stand the explanation down"
  }
  $ackLog = @(Get-Content (Join-Path $ackState "ShaiWin\shaiwin.log") -ErrorAction SilentlyContinue)
  if (@($ackLog | Where-Object { $_ -match "stands down: he pressed the bubble's button" }).Count -ne 1) {
    Fail "the press was remembered without recording how it was answered"
  }
  if (Get-HintRect $shotDir $proc.Id $overlay) {
    Fail "the bubble was still drawn after he pressed its button"
  }
  # Back to letting clicks through: the layer is the whole strip, and the strip
  # belongs to his work the moment its bubble is answered.
  if (-not (Wait-Until ({ [W]::IsClickThrough($rope) }) 1500)) {
    Fail "the rope layer kept taking clicks after its button was pressed"
  }
  Write-Host "  pressed once, explained never again on this run"

  Write-Host "== 13. a later run that already knows stays quiet =="
  Stop-ShaiwinWithEverythingDown
  $proc = Start-Shaiwin $Exe $ackState
  Start-Sleep -Milliseconds 1400
  $overlay = Get-Overlay
  $rope = Get-Rope
  if (-not (Test-Window $overlay)) { Fail "the second restart was not created" }
  Invoke-Wm $overlay $KitHang ([IntPtr]::Zero) ([IntPtr]$a)
  if (-not (Wait-Until ({ (Test-Cards $overlay) -ge 1 }) 8000)) {
    Fail "the second restart did not take the window"
  }
  Point-At $overlay $workPointX $workPointY
  if (-not (Wait-Until ({ -not [W]::IsTopmost($overlay) }) 2500)) {
    Fail "the second restart kept the top over his window"
  }
  if (-not (Wait-Until ({ [W]::IsWindowVisible($rope) -and [W]::IsTopmost($rope) }) 2000)) {
    Fail "the second restart left no rope on screen; the silence below would prove nothing"
  }
  if (Get-HintRect $shotDir $proc.Id $overlay) {
    Fail "a run that started already knowing explained the gesture again"
  }
  Write-Host "  no sentence for a user who pressed 'don't remind me' in an earlier run"

  Write-Host "PASS"
} finally {
  Stop-ShaiwinWithEverythingDown
  Remove-Fixtures
  Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item $ackRoot -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "cleaned up"
}
