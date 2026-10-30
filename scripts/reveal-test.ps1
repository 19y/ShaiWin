# Regression test for the non-resident line (Windows).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\reveal-test.ps1
#
# It runs the app with `always_on=0`, the mode where the line is a short visit
# rather than furniture, and covers:
#   1. a launch shows the line even in this mode — tucked away at the top edge
#      there is no way to tell the app started — and then tucks it away
#   2. resting the pointer on the top edge shows the "show the line" pill, not
#      the line itself
#   3. leaving the edge dismisses the pill
#   4. clicking the pill opens the line; a click away from an empty line does
#      not end the visit at once (the user is off picking a window), and the
#      line leaves on the empty-line linger instead
#   5. the toggle shortcut also opens the empty line for a short visit, and it
#      tucks itself away instead of lingering
#   6. after an automatic tuck, leaving and returning re-arms the pill
#   7. a loaded line stays while the pointer is on it, then leaves after the
#      auto-hide clock once the pointer stops using it
#   8. another window taking the foreground ends a loaded visit (the empty-line
#      linger does not apply when there is a card to come back to)
#
# Steps 1-7 need a pointer that can actually be moved; a session with no input
# desktop skips them. Step 8 needs to be able to move the foreground and skips
# the same way if Windows will not let this process do it.
#
# It uses its own LOCALAPPDATA so it can never touch the real journal, and it
# takes any running instance's windows down before stopping it.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe")
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$root = Join-Path $env:TEMP "shaiwin-reveal"
# A non-resident line has its own state machine, so this test runs with the
# always-on mode off; the resident default is covered by smoke-test.ps1.
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=0`nauto_hide_secs=2`nexcluded=`n"
Initialize-Fixtures $root
$journal = Join-Path $state "ShaiWin\parked.txt"

$shotDir = Join-Path $env:TEMP "shaiwin-reveal-shots"
New-Item -ItemType Directory -Force -Path $shotDir | Out-Null
Get-ChildItem $shotDir -Filter "hits-*.txt" -ErrorAction SilentlyContinue | Remove-Item -Force
$env:SHAIWIN_DUMP_DIR = $shotDir

Stop-ShaiwinWithEverythingDown

$screenW = [W]::GetSystemMetrics(0)
$screenH = [W]::GetSystemMetrics(1)
$centerX = [int]($screenW / 2)
$centerY = [int]($screenH / 2)
$canMoveCursor = Test-CursorControl

$proc = Start-Shaiwin $Exe $state
Start-Sleep -Milliseconds 1400

# The toggle hot key, sent the way Windows sends it.
function Toggle([IntPtr]$h) { Invoke-Wm $h $KitHotkey ([IntPtr]2) ([IntPtr]::Zero) }

# The app reports the rectangles it just drew; the pill has its own id. Asking
# for a dump is the only reliable way to tell "the line is down" from "only the
# pill is down".
function Get-Hits([IntPtr]$overlay) {
  return @(Get-CardRects $shotDir $proc.Id $overlay)
}

function Test-Peek([IntPtr]$overlay) {
  if (-not [W]::IsWindowVisible($overlay)) { return $false }
  return [bool](Get-Hits $overlay | Where-Object { $_.Peek } | Select-Object -First 1)
}

function Test-Line([IntPtr]$overlay) {
  if (-not [W]::IsWindowVisible($overlay)) { return $false }
  return -not [bool](Get-Hits $overlay | Where-Object { $_.Peek } | Select-Object -First 1)
}

function Wait-Peek([IntPtr]$overlay, [bool]$want, [int]$timeoutMs) {
  return Wait-Until ({ (Test-Peek $overlay) -eq $want }) $timeoutMs
}

function Wait-Line([IntPtr]$overlay, [bool]$want, [int]$timeoutMs) {
  return Wait-Until ({ (Test-Line $overlay) -eq $want }) $timeoutMs
}

function Click-Peek([IntPtr]$overlay) {
  $r = Get-Rect $overlay
  $p = Get-Hits $overlay | Where-Object { $_.Peek } | Select-Object -First 1
  if (-not $p) { return $false }
  $x = [int](($p.Left + $p.Right) / 2 + $r.Left)
  $y = [int](($p.Top + $p.Bottom) / 2 + $r.Top)
  return (Click-At $x $y)
}

function Test-Cards([IntPtr]$overlay) {
  return @(Get-Hits $overlay | Where-Object { -not $_.Peek }).Count
}

try {
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay window was not created" }
  Write-Host "shaiwin pid $($proc.Id), overlay $overlay, cursor control: $canMoveCursor"

  if (-not $canMoveCursor) {
    Write-Host "== 1-6. SKIPPED: this session has no pointer to move =="
  } else {
    Write-Host "== 1. a launch shows the line, then gets out of the way =="
    # "你不展开，谁知道软件打开了" — a line parked at the top edge is
    # indistinguishable from an app that never started, so a launch shows it in
    # every mode and the visit rules take it back.
    if (-not (Move-Cursor $centerX $centerY)) { Fail "lost cursor control" }
    if (-not (Wait-Line $overlay $true 2500)) {
      Fail "a launch in non-resident mode left the line tucked away"
    }
    Write-Host "  shown at launch"
    if (-not (Wait-Until ({ -not [W]::IsWindowVisible($overlay) }) 8000)) {
      Fail "the launch look never ended"
    }
    Write-Host "  tucked after the look"

    $rect = Get-Rect $overlay
    # A point on the top edge that is not on the pill: the pill hangs in the
    # middle of the line, about 44 px below the edge.
    $edgeX = [int]($rect.Left + 80)
    $edgeY = $rect.Top
    $topX = $edgeX
    $topY = $edgeY

    Write-Host "== 2. the edge brings the pill out, and only the pill opens the line =="
    # Sliding to the top of the screen is passing through on the way to a window's
    # own title bar, not a request for the rack. The pill (the card count) answers
    # at once; the line stays away until the pointer comes to rest on the button.
    if (-not (Move-Cursor $edgeX $edgeY)) { Fail "lost cursor control" }
    if (-not (Wait-Peek $overlay $true 1200)) {
      Fail "hovering the top edge did not show the show-the-line pill"
    }
    Start-Sleep -Milliseconds 900        # twice the dwell an empty line waits on
    if (Test-Line $overlay) {
      Fail "the line opened itself on a pointer that never reached the pill"
    }
    Write-Host "  the pill is out and the line is not"
    $pill = Get-Hits $overlay | Where-Object { $_.Peek } | Select-Object -First 1
    if (-not $pill) { Fail "the pill vanished while the pointer sat at the edge" }
    $pillX = [int]($rect.Left + ($pill.Left + $pill.Right) / 2)
    $pillY = [int]($rect.Top + ($pill.Top + $pill.Bottom) / 2)
    if (-not (Move-Cursor $pillX $pillY)) { Fail "lost cursor control" }
    if (-not (Wait-Line $overlay $true 2000)) {
      Fail "resting on the pill did not open the line without a click"
    }
    Write-Host "  resting on the button opens it"

    Write-Host "== 3. leaving the edge dismisses the pill =="
    Toggle $overlay
    if (-not (Wait-Until ({ -not (Test-Line $overlay) }) 2500)) {
      Fail "the shortcut did not put the line away"
    }
    if (-not (Move-Cursor $edgeX $edgeY)) { Fail "lost cursor control" }
    if (-not (Wait-Peek $overlay $true 2000)) {
      Fail "the pill did not answer the edge a second time"
    }
    if (-not (Move-Cursor $centerX $centerY)) { Fail "lost cursor control" }
    if (-not (Wait-Until ({ -not [W]::IsWindowVisible($overlay) }) 2500)) {
      Fail "the pill stayed on screen after the pointer left the edge"
    }
    Write-Host "  hidden"

    Write-Host "== 4. a click away no longer ends an empty line at once =="
    if (-not (Move-Cursor $edgeX $edgeY)) { Fail "lost cursor control" }
    if (-not (Wait-Peek $overlay $true 2000)) { Fail "the pill did not answer the edge" }
    if (-not (Move-Cursor $pillX $pillY)) { Fail "lost cursor control" }
    if (-not (Wait-Line $overlay $true 2500)) { Fail "the pill did not open the line" }
    Write-Host "  line down"
    # A click well away from an empty line is the user going to find the window
    # that goes on it, not the user leaving: the line has to survive the trip.
    # It still leaves on its own clock, which for an empty line is at least the
    # empty-line linger (4 s here, longer than auto_hide_secs=2).
    if (-not (Move-Cursor $centerX $centerY)) { Fail "lost cursor control" }
    if (-not (Click-At $centerX $centerY)) { Fail "lost cursor control" }
    Start-Sleep -Milliseconds 1500
    if (-not [W]::IsWindowVisible($overlay)) {
      Fail "an empty line vanished while the user was on the way to pick a window"
    }
    if (-not (Wait-Until ({ -not [W]::IsWindowVisible($overlay) }) 5000)) {
      Fail "an empty line never left after the linger"
    }
    Write-Host "  survived the click, then left on its own clock"

    Write-Host "== 5. the shortcut opens the empty line for a short visit only =="
    # The pointer is above the rope, not on it: the visit must end on the
    # line's own clock. Toggle before the pill's dwell, so this is the line's
    # auto-hide and not an interaction.
    if (-not (Move-Cursor $topX $topY)) { Fail "lost cursor control" }
    Toggle $overlay
    if (-not (Wait-Line $overlay $true 2000)) { Fail "the shortcut did not open the empty line" }
    Start-Sleep -Milliseconds 900
    if (-not [W]::IsWindowVisible($overlay)) { Fail "the empty line vanished immediately" }
    if (-not (Wait-Until ({ -not [W]::IsWindowVisible($overlay) }) 5000)) {
      Fail "the empty line stayed down until it became furniture"
    }
    Write-Host "  down, then tucked itself away"

    Write-Host "== 6. leaving and returning re-arms the pill =="
    if (-not (Move-Cursor $centerX $centerY)) { Fail "lost cursor control" }
    Start-Sleep -Milliseconds 600
    if (-not (Move-Cursor $topX $topY)) { Fail "lost cursor control" }
    if (-not (Wait-Peek $overlay $true 2500)) {
      Fail "the pill did not come back after the pointer left the edge"
    }
    if (-not (Move-Cursor $centerX $centerY)) { Fail "lost cursor control" }
    Write-Host "  pill re-armed"

    Write-Host "== 7. a loaded line stays while it is being used =="
    # The pointer resting on a card is using it; taking the pointer away lets
    # the auto-hide clock run out.
    $a = New-Fixture "A" 0
    Invoke-Wm $overlay $KitHang ([IntPtr]::Zero) ([IntPtr]$a)
    if (-not (Wait-Until ({ (Test-Cards $overlay) -ge 1 }) 6000)) {
      Fail "hanging a window drew no card"
    }
    $card = Get-Hits $overlay | Where-Object { -not $_.Peek } | Select-Object -First 1
    if (-not $card) { Fail "the card has no rectangle to rest the pointer on" }
    $cardX = [int]($rect.Left + ($card.Left + $card.Right) / 2)
    $cardY = [int]($rect.Top + ($card.Top + $card.Bottom) / 2)
    if (-not (Move-Cursor $cardX $cardY)) { Fail "lost cursor control" }
    Start-Sleep -Milliseconds 2000
    if (-not (Test-Line $overlay)) {
      Fail "the line tucked itself away under a pointer that was still on it"
    }
    Write-Host "  still down while the pointer is on the card"
    if (-not (Move-Cursor $centerX $centerY)) { Fail "lost cursor control" }
    if (-not (Wait-Until ({ -not [W]::IsWindowVisible($overlay) }) 4000)) {
      Fail "a loaded line kept the top of the screen after the pointer left it"
    }
    Write-Host "  then gone"
    Invoke-Wm $overlay $KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero)
    Start-Sleep -Milliseconds 400
  }

  Write-Host "== 8. another window coming forward ends a loaded visit =="
  # No pointer involved. The line carries a card here: an empty line is the
  # deliberate exception (the user is off picking a window), but a line with
  # something on it must leave when the next window is used. Which layer is
  # topmost is the pointer's business now and is covered by zorder-test.
  $probe = New-Fixture "Probe" 3
  $other = New-Fixture "Other" 4
  [void][W]::ShowWindow($probe, 5)
  [void][W]::SetForegroundWindow($probe)
  Start-Sleep -Milliseconds 250
  if ([W]::GetForegroundWindow() -ne $probe) {
    Write-Host "  SKIPPED: this session will not let the foreground move"
  } else {
    Invoke-Wm $overlay $KitHang ([IntPtr]::Zero) ([IntPtr]$probe)
    if (-not (Wait-Until ({ (Get-Record $journal $probe) -ne $null }) 8000)) {
      Fail "the probe window was not hung"
    }
    if (-not (Wait-Line $overlay $true 2500)) { Fail "hanging a window did not bring the line out" }
    Write-Host "  the line is down with a card on it"
    # The user goes back to a different window. That is the end of the visit.
    [void][W]::SetForegroundWindow($other)
    if (-not (Wait-Until ({ -not [W]::IsWindowVisible($overlay) }) 2500)) {
      Fail "a loaded line stayed over the desktop after another window came forward"
    }
    Write-Host "  tucked away"
    # Asking for the line again brings it back with its card still on it.
    Toggle $overlay
    if (-not (Wait-Line $overlay $true 2500)) { Fail "the line did not come back" }
    Write-Host "  the shortcut brings it back"
    Invoke-Wm $overlay $KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero)
    Start-Sleep -Milliseconds 500
  }

  Write-Host "PASS"
} finally {
  Stop-ShaiwinWithEverythingDown
  Remove-Fixtures
  Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "cleaned up"
}
