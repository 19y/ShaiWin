# Regression test for showing and hiding the empty line (Windows).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\reveal-test.ps1
#
# It runs the app with `hover_open=1` and `always_on=0`, the mode where the line
# is tucked away until it is asked for, and covers the state machine that decides
# whether the line is down:
#   1. an empty line starts hidden (no cards, no pointer at the top)
#   2. an explicit "show line" stays down until it is dismissed on purpose
#      (it used to tuck itself away after 0.9 s, so the hot key looked dead)
#   3. an explicit "hide line" puts it away and does not pop back out from under
#      a pointer that is still resting on the top edge
#   4. hovering the top edge brings an empty line down, leaving hides it again
#   5. hiding while the pointer is on the edge sticks; leaving and coming back
#      re-arms hover
#
# The hover steps need a pointer that can actually be moved; a session with no
# input desktop skips them and still checks 1-3.
#
# It uses its own LOCALAPPDATA so it can never touch the real journal, and it
# takes any running instance's windows down before stopping it.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe")
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$root = Join-Path $env:TEMP "shaiwin-reveal"
# This machine only exists when the line is not pinned open, so the test runs
# with always_on off; the pinned default has its own check in smoke-test.
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nhover_open=1`nalways_on=0`nexcluded=`n"

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

try {
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay window was not created" }
  Write-Host "shaiwin pid $($proc.Id), overlay $overlay, cursor control: $canMoveCursor"

  Write-Host "== 1. empty line starts hidden =="
  if (-not (Wait-Visibility $overlay $false 1500)) {
    if ($canMoveCursor) {
      Fail "an empty line was visible at startup"
    }
    # The cursor could not be moved and happens to rest on the top edge, so
    # hover is doing its job. Put the line away through the explicit path and
    # start from there; the hover steps will be skipped below.
    Write-Host "  line is down because the cursor sits on the top edge; hiding it to start"
    Toggle $overlay
    if (-not (Wait-Visibility $overlay $false 2000)) { Fail "the line did not hide" }
  }
  Write-Host "  hidden"

  Write-Host "== 2. explicit show stays down =="
  Toggle $overlay
  if (-not (Wait-Visibility $overlay $true 1500)) { Fail "the line did not come down" }
  Start-Sleep -Milliseconds 3000
  if (-not [W]::IsWindowVisible($overlay)) {
    Fail "the empty line hid itself while explicitly shown (the reported bug)"
  }
  Write-Host "  still down after 3 s"

  Write-Host "== 3. explicit hide sticks =="
  Toggle $overlay
  if (-not (Wait-Visibility $overlay $false 2000)) { Fail "the line did not tuck away" }
  Start-Sleep -Milliseconds 1500
  if ([W]::IsWindowVisible($overlay)) { Fail "the line came back after being hidden" }
  Write-Host "  hidden and stayed hidden"

  $rect = Get-Rect $overlay

  if ($canMoveCursor) {
    $topX = [int](($rect.Left + $rect.Right) / 2)
    $topY = $rect.Top

    Write-Host "== 4. hover brings an empty line down =="
    if (-not (Move-Cursor $centerX $centerY)) { Fail "lost cursor control" }
    Start-Sleep -Milliseconds 600
    if (-not (Move-Cursor $topX $topY)) { Fail "lost cursor control" }
    if (-not (Wait-Visibility $overlay $true 2000)) {
      Fail "hovering the top edge did not reveal the empty line"
    }
    Write-Host "  down"

    Write-Host "== 5. leaving tucks it away =="
    if (-not (Move-Cursor $centerX $centerY)) { Fail "lost cursor control" }
    if (-not (Wait-Visibility $overlay $false 2500)) {
      Fail "the line stayed down after the pointer left"
    }
    Write-Host "  hidden"

    Write-Host "== 6. hiding with the pointer on the edge sticks =="
    if (-not (Move-Cursor $topX $topY)) { Fail "lost cursor control" }
    if (-not (Wait-Visibility $overlay $true 2000)) { Fail "hover did not reveal the line" }
    Toggle $overlay
    if (-not (Wait-Visibility $overlay $false 2000)) { Fail "the line did not hide" }
    Start-Sleep -Milliseconds 1500
    if ([W]::IsWindowVisible($overlay)) { Fail "the line popped back out under the cursor" }

    Write-Host "== 7. leaving and returning re-arms hover =="
    if (-not (Move-Cursor $centerX $centerY)) { Fail "lost cursor control" }
    Start-Sleep -Milliseconds 600
    if (-not (Move-Cursor $topX $topY)) { Fail "lost cursor control" }
    if (-not (Wait-Visibility $overlay $true 2000)) {
      Fail "hover did not work again after the pointer left the edge"
    }
    if (-not (Move-Cursor $centerX $centerY)) { Fail "lost cursor control" }
    if (-not (Wait-Visibility $overlay $false 2500)) { Fail "the line stayed down" }
    Write-Host "  hover re-armed"
  } else {
    Write-Host "== 4-7. SKIPPED: this session has no pointer to move =="
  }

  Write-Host "PASS"
} finally {
  Stop-ShaiwinWithEverythingDown
  Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "cleaned up"
}
