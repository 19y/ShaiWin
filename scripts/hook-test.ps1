# Regression tests for the hook: the taskbar, minimize, unhook and restart paths.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\hook-test.ps1
#
# Covers the reported bugs:
#   1. hang a window, then minimize it the way the taskbar does
#      -> the window comes back on screen instead of staying invisible
#   2. minimize a window that is back on the desktop
#      -> it is hung up again by itself (the hook), without ever flashing back
#        onto the desktop on its way up, and clicking it in the taskbar brings
#        it down again with the hook still on
#   3. unhook a window
#      -> minimizing it afterwards does nothing to the line
#   4. kill ShaiWin while a hooked window is on the desktop, start it again
#      -> the hook survives the restart, and still works (steps 4 and 5)
#   6. ask for a window that is hooked but sitting on the desktop
#      -> it goes back up; it used to be refused with "already on the line"
#         while the line showed nothing of it
#   7. drag a window to the top and let go
#      -> the "let go" prompt is gone once the drag is over and the pointer has
#        left the top edge (the stuck pill); skipped when the cursor cannot move
#   7b. put the line away on purpose, then drag a window over its old spot
#      -> the line stays hidden and the window does not hang on it
#
# The windows are throwaway fixtures from scripts\testkit.ps1, so no editor,
# browser or document of the user is ever moved or closed, and the app runs
# against its own LOCALAPPDATA: the real journal and settings stay untouched.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe")
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$root = Join-Path $env:TEMP "shaiwin-hook"
# Direct hover has its own test; here the line is driven through the hooks.
# always_on=0 because steps 7 and 7b assert the line tucks itself away again.
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nhover_open=0`nalways_on=0`nexcluded=`n"
Initialize-Fixtures $root
$journal = Join-Path $state "ShaiWin\parked.txt"

Stop-ShaiwinWithEverythingDown

# The pointer cannot be moved or even read in a session with no input desktop,
# so the drag step is reported as skipped rather than failed.
$canMoveCursor = Test-CursorControl

function Hang([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitHang ([IntPtr]::Zero) ([IntPtr]$hwnd) }
function Take-One([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitTakeOne ([IntPtr]::Zero) ([IntPtr]$hwnd) }
function Unhook([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitUnhook ([IntPtr]::Zero) ([IntPtr]$hwnd) }
function Toggle([IntPtr]$hwnd) { Invoke-Wm $hwnd $KitHotkey ([IntPtr]2) ([IntPtr]::Zero) }
function Record([IntPtr]$hwnd) { Get-Record $journal $hwnd }

try {
  $proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 1800
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay window was not created" }
  Write-Host "shaiwin pid $($proc.Id), overlay $overlay, cursor control: $canMoveCursor"

  $a = New-Fixture "A" 0
  $b = New-Fixture "B" 1
  Write-Host "fixtures A=$a B=$b"

  # --- 1 and 2: the taskbar path ----------------------------------------------
  Write-Host "== 1. hang A, then minimize it the way the taskbar does =="
  Hang $a
  if (-not (Wait-Until ({ Test-Parked $a }) 5000)) { Fail "A was not parked after hanging it" }
  $rec = Record $a
  if (-not $rec) { Fail "no journal record for A while it is parked" }
  if ($rec.Hooked) { Fail "a parked window should not be recorded as hooked away" }
  Write-Host "  A parked and recorded"

  [void][W]::ShowWindow($a, 2)   # SW_MINIMIZE, the same thing the taskbar asks for
  if (-not (Wait-Until ({ Test-OnDesktop $a }) 5000)) {
    Fail "minimizing a hung window left it invisible - the reported taskbar bug"
  }
  # The window is on the desktop as soon as Windows restores it; the hook flag
  # only follows when the app has written its journal.
  if (-not (Wait-Until ({ (Record $a) -and (Record $a).Hooked }) 5000)) {
    Fail "A came back but is no longer hooked, so a later minimize would not hang it again"
  }
  Write-Host "  A is back on screen and still hooked"

  Write-Host "== 2. minimizing it on the desktop hangs it up again, quietly =="
  $why = Minimize-Hang $a $journal
  if ($why) { Fail "A: $why" }
  Write-Host "  A hung up where it stands, without coming back on screen"

  Write-Host "== 2b. clicking it in the taskbar brings it down again =="
  [void][W]::ShowWindow($a, 9)   # SW_RESTORE, what the taskbar button does
  # The window is on the desktop the moment Windows restores it; the hook flag
  # only follows when the app has had its look, so wait for the record.
  if (-not (Wait-Until ({ (Record $a) -and (Record $a).Hooked }) 5000)) {
    Fail "A came down from the line but lost its hook"
  }
  if (-not (Test-OnDesktop $a)) { Fail "A is hooked but not on the desktop: $(Get-Rect $a)" }
  Write-Host "  A is back on the desktop and still hooked"

  # --- 3: unhook --------------------------------------------------------------
  Write-Host "== 3. unhook: minimizing afterwards must not touch the line =="
  if (-not (Wait-Until ({ Test-OnDesktop $a }) 5000)) { Fail "A should be on the desktop before it is unhooked" }
  if (-not (Record $a)) { Fail "A should still be hooked before the unhook" }
  Unhook $a
  if (-not (Wait-Until ({ $null -eq (Record $a) }) 3000)) {
    Fail "unhooking A left a journal record behind"
  }
  [void][W]::ShowWindow($a, 2)
  Start-Sleep -Milliseconds 1500
  if (-not [W]::IsIconic($a)) { Fail "an unhooked window should just stay minimized" }
  if (Record $a) { Fail "minimizing an unhooked window wrote a journal record" }
  [void][W]::ShowWindow($a, 9)   # SW_RESTORE, so the rest of the test has a normal window
  Start-Sleep -Milliseconds 600
  Write-Host "  unhooked and left alone"

  # --- 4: the hook survives a restart ----------------------------------------
  Write-Host "== 4. kill ShaiWin with a hooked window on the desktop, start it again =="
  Hang $b
  if (-not (Wait-Until ({ Test-Parked $b }) 5000)) { Fail "B was not parked" }
  Take-One $b
  if (-not (Wait-Until ({ Test-OnDesktop $b }) 5000)) { Fail "B was not taken down" }
  if (-not (Wait-Until ({ (Record $b) -and (Record $b).Hooked }) 5000)) { Fail "B should be recorded as hooked before the restart" }
  Get-Process shaiwin -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Milliseconds 600
  $proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 1800
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay is gone after the restart" }
  if (-not (Record $b)) { Fail "B's hook was lost with the restart" }
  if (Test-Parked $b) { Fail "a hooked window should not be parked before it is minimized" }
  Write-Host "  hook recovered"

  Write-Host "== 5. the recovered hook still works =="
  $why = Minimize-Hang $b $journal
  if ($why) { Fail "B after the restart: $why" }
  Write-Host "  B hung up again after the restart, quietly"

  Write-Host "== 6. hang a hooked window that is sitting on the desktop =="
  # The reported dead end: the card is hidden, so the line looks empty, and
  # asking for it again answered "that window is already on the line".
  Take-One $b
  if (-not (Wait-Until ({ (Record $b) -and (Record $b).Hooked }) 5000)) { Fail "B was not taken down and left hooked" }
  if (-not (Test-OnDesktop $b)) { Fail "B is hooked but not on the desktop: $(Get-Rect $b)" }
  Hang $b
  if (-not (Wait-Until ({ Test-Parked $b }) 5000)) {
    Fail "hanging a hooked window on the desktop was refused - the reported dead end"
  }
  if (@(Read-Journal $journal).Count -ne 1) { Fail "re-hanging B should reuse its record, got $(@(Read-Journal $journal).Count)" }
  Write-Host "  B went back up on the same record"

  # --- 7: the drag prompt -----------------------------------------------------
  if ($canMoveCursor) {
    Write-Host "== 7. drag to the top: the prompt must not outlive the drag =="
    $cursorHome = New-Object W+POINT
    [void][W]::GetCursorPos([ref]$cursorHome)
    $c = New-Fixture "C" 2
    $r = Get-Rect $c
    $grabX = [int](($r.Left + $r.Right) / 2)
    $grabY = [int]($r.Top + 18)
    if (-not (Move-Cursor $grabX $grabY)) { Fail "lost cursor control" }
    [W]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
    Start-Sleep -Milliseconds 200
    $v = Get-VirtualBounds
    # Into the hang band, but not flush with the screen edge: the top
    # MAXIMIZE_YIELD (2 logical px) belongs to Windows' own maximize gesture, so
    # a release there is deliberately left to the system. The band now stops at
    # the rope (default curve on a 2560 px screen: 3-74 physical px), so this
    # grab - 18 px above the window's own top edge - parks that edge at +42,
    # inside the band where a real release hangs.
    $bandY = [int]($v.Top + 60)
    for ($i = 1; $i -le 8; $i++) {
      [void](Move-Cursor $grabX ($grabY + [int](($bandY - $grabY) * $i / 8)))
    }
    Start-Sleep -Milliseconds 500
    if (-not (Wait-Until ({ [W]::IsWindowVisible((Get-Overlay)) }) 2000)) {
      [W]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
      Fail "the line did not appear while the window was in the hang band"
    }
    [W]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
    if (-not (Wait-Until ({ Test-Parked $c }) 5000)) {
      Fail "letting go in the band did not hang the window up"
    }
    Write-Host "  C hung up by the drag"

    # With the pointer away from the top edge and nothing pinned, the strip and
    # whatever it was saying must both leave the screen.
    if (-not (Move-Cursor ([int]($v.Right / 2)) ([int]($v.Bottom - 120)))) { Fail "lost cursor control" }
    if (-not (Wait-Until ({ -not [W]::IsWindowVisible((Get-Overlay)) }) 4000)) {
      Fail "the line stayed on screen after the drag ended and the pointer left: $(Get-Rect (Get-Overlay))"
    }
    Write-Host "  the strip went away with the drag"

    # --- 7b: a line put away on purpose is inert ------------------------------
    Write-Host "== 7b. a line put away on purpose ignores a drag over its old spot =="
    $overlay = Get-Overlay
    # Get it down first if it has already tucked itself away, so the hide below
    # goes through the explicit toggle (an automatic tuck is not a dismissal).
    if (-not [W]::IsWindowVisible($overlay)) {
      Toggle $overlay
      if (-not (Wait-Visibility $overlay $true 2000)) { Fail "the line did not come down for the hide step" }
    }
    Toggle $overlay
    if (-not (Wait-Visibility $overlay $false 2000)) { Fail "the line did not tuck away" }
    # The reported bug: with the line collapsed, a window dragged to where the
    # line used to be lit it up and hung itself on it. It must stay inert until
    # it is asked for again (hotkey, pill or hover).
    $d = New-Fixture "D" 3
    $rd = Get-Rect $d
    $dx = [int](($rd.Left + $rd.Right) / 2)
    $dy = [int]($rd.Top + 18)
    if (-not (Move-Cursor $dx $dy)) { Fail "lost cursor control" }
    [W]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
    Start-Sleep -Milliseconds 200
    for ($i = 1; $i -le 8; $i++) {
      [void](Move-Cursor $dx ($dy + [int](($bandY - $dy) * $i / 8)))
    }
    Start-Sleep -Milliseconds 900
    if ([W]::IsWindowVisible($overlay)) {
      [W]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
      Fail "a line that was put away lit up for a dragged window"
    }
    [W]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
    Start-Sleep -Milliseconds 900
    if (Test-Parked $d) { Fail "a window hung itself on a line that was put away" }
    if ([W]::IsWindowVisible($overlay)) { Fail "the hidden line came back after the drop" }
    Write-Host "  left alone"
    [void][W]::SetCursorPos($cursorHome.X, $cursorHome.Y)
  } else {
    Write-Host "== 7. SKIPPED: this session has no pointer to move =="
  }

  Write-Host "== 8. take everything down through the app =="
  $overlay = Get-Overlay
  if (Test-Window $overlay) { Invoke-Wm $overlay $KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero) }
  Start-Sleep -Milliseconds 2600
  foreach ($h in $script:fixtureHandles) {
    if ($h -ne [IntPtr]::Zero -and (Test-Parked $h)) { Fail "window $h was still parked after taking everything down" }
  }
  if (Test-Path $journal) { Fail "parked.txt still exists after taking everything down" }
  Write-Host "  nothing left hanging"

  Write-Host "PASS"
} finally {
  Stop-ShaiwinWithEverythingDown
  Remove-Fixtures
  Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "cleaned up"
}
