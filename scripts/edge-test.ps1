# Edge-case tests for ShaiWin (Windows).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\edge-test.ps1
#
# Covers the cases the happy-path smoke test does not:
#   1. the same window is asked to hang twice          -> one card, one record
#   2. the user puts a parked window back by hand      -> the card hides, the
#      hook stays (minimizing it again hangs it up)
#   3. a maximized window hangs and comes back maximized
#   4. the application closes while its card is hanging -> record is dropped
#   5. quitting while something is hanging             -> window minimized in
#      place (not thrown onto the desktop), journal gone
#   6. minimizing a restored window                    -> only that window comes
#      back on the line; the shell's hand-over of the foreground to a parked
#      window is not a request to see it
#
# The windows are throwaway fixtures started by this script, so nothing the user
# was working on is ever moved, closed or killed; and the app runs against its
# own LOCALAPPDATA, so the real journal and settings stay untouched.
param([string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe"))

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$root = Join-Path $env:TEMP "shaiwin-edge"
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nexcluded=`n"
Initialize-Fixtures $root
$journal = Join-Path $state "ShaiWin\parked.txt"

Stop-ShaiwinWithEverythingDown

function Count-Records { @(Read-Journal $journal).Count }
function Hang([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitHang ([IntPtr]::Zero) ([IntPtr]$hwnd) }
function Take-All { Invoke-Wm (Get-Overlay) $KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero) }
function Take-One([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitTakeOne ([IntPtr]::Zero) ([IntPtr]$hwnd) }

try {
  $proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2000
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay window was not created" }
  Write-Host "shaiwin pid $($proc.Id), overlay $overlay"

  Write-Host "== 1. hang the same window twice =="
  $a = New-Fixture "A" 0
  $origin = Get-Rect $a
  Hang $a
  Start-Sleep -Milliseconds 500
  Hang $a
  if (-not (Wait-Until ({ (Count-Records) -eq 1 }) 5000)) { Fail "expected one journal record, got $(Count-Records)" }
  if (-not (Wait-Until ({ Test-Parked $a }) 4000)) { Fail "A was not parked" }
  Write-Host "  one record, A parked"

  Write-Host "== 2. put the parked window back by hand =="
  # A parked window is minimized, so putting it back by hand is what clicking its
  # taskbar button does: restore it. (Under the old off-screen parking this was a
  # SetWindowPos back onto the desktop - there was a visible window to drag. A
  # minimized one has to be restored instead.)
  [void][W]::ShowWindow($a, 9)   # SW_RESTORE
  if (-not (Wait-Until ({ Test-OnDesktop $a }) 5000)) { Fail "A did not come back on screen" }
  $backA = Get-Rect $a
  if ($backA) {
    $dx = [Math]::Abs($backA.Left - $origin.Left)
    $dy = [Math]::Abs($backA.Top - $origin.Top)
    if ($dx -gt 8 -or $dy -gt 8) { Fail "A came back at $($backA.Left),$($backA.Top) instead of $($origin.Left),$($origin.Top)" }
  }
  if (-not (Wait-Until ({ (Get-Record $journal $a).Hooked }) 5000)) { Fail "the hook was lost when the window was put back by hand" }
  if ((Count-Records) -ne 1) { Fail "the record was duplicated or dropped" }
  # Putting it back by hand is not the same as cancelling the hook, so a
  # minimize afterwards still belongs on the line - and it goes up from where it
  # stands, without flashing back onto the desktop first.
  $why = Minimize-Hang $a $journal
  if ($why) { Fail "A: $why" }
  Write-Host "  card hidden, hook kept, and a later minimize hung it up again without a flash"

  Write-Host "== 3. maximized window =="
  $b = New-Fixture "B" 1
  [void][W]::ShowWindow($b, 3)   # SW_MAXIMIZE
  Start-Sleep -Milliseconds 900
  if (-not [W]::IsZoomed($b)) { Fail "could not maximize the fixture" }
  Hang $b
  # Maximized windows are taken off screen by minimizing them, not by parking
  # them past the edge: moving one drops it out of the maximized state.
  if (-not (Wait-Until ({ [W]::IsIconic($b) }) 5000)) { Fail "the maximized window was not taken off screen" }
  Take-All
  if (-not (Wait-Until ({ Test-OnDesktop $b -or [W]::IsZoomed($b) }) 6000)) { Fail "B did not come back on screen" }
  if (-not [W]::IsZoomed($b)) { Fail "the window did not come back maximized" }
  Write-Host "  taken off screen and restored as maximized"

  Write-Host "== 4. the application closes while its card is hanging =="
  $c = New-Fixture "C" 2
  Hang $c
  if (-not (Wait-Until ({ Test-Parked $c }) 5000)) { Fail "C was not parked" }
  $before = Count-Records
  Stop-Process -Id (Get-FixturePid "C") -Force
  if (-not (Wait-Until ({ (Count-Records) -eq ($before - 1) }) 6000)) {
    Fail "the record of the closed window was not dropped ($before before, $(Count-Records) after)"
  }
  if (Get-Record $journal $c) { Fail "the record of a closed window is still there" }
  Write-Host "  its record went away with it"

  Write-Host "== 5. quit while something is hanging =="
  $d = New-Fixture "D" 3
  $originD = Get-Rect $d
  Hang $d
  if (-not (Wait-Until ({ Test-Parked $d }) 5000)) { Fail "D was not parked" }
  Invoke-Wm (Get-Overlay) $KitClose ([IntPtr]::Zero) ([IntPtr]::Zero)
  # Watched while the app goes away: minimizing in one step is the point, and a
  # window drawn on the desktop on the way out is the flash to avoid.
  $flashed = $false
  $end = [Environment]::TickCount + 6000
  while ([Environment]::TickCount -lt $end) {
    if (Test-OnDesktop $d) { $flashed = $true }
    if (-not (Get-Process shaiwin -ErrorAction SilentlyContinue)) { break }
    Start-Sleep -Milliseconds 15
  }
  if ($flashed) { Fail "D was shown on the desktop while the app was quitting" }
  if (-not (Wait-Until ({ -not (Get-Process shaiwin -ErrorAction SilentlyContinue) }) 6000)) { Fail "shaiwin did not exit on WM_CLOSE" }
  if (-not (Wait-Until ({ [W]::IsIconic($d) }) 4000)) {
    Fail "D was not minimized on quit - quitting must not throw every card's window back onto the desktop"
  }
  # Minimized where the user can find it: the remembered place is still D's own.
  $pl = Get-Placement $d
  if (-not $pl) { Fail "D has no placement after the quit" }
  $dx = [Math]::Abs($pl.rcNormalPosition.Left - $originD.Left)
  $dy = [Math]::Abs($pl.rcNormalPosition.Top - $originD.Top)
  if ($dx -gt 8 -or $dy -gt 8) { Fail "D would come back somewhere else (off by $dx,$dy)" }
  # And a click on its taskbar button really does put it back there.
  [void][W]::ShowWindow($d, 9)   # SW_RESTORE
  Start-Sleep -Milliseconds 400
  $back = Get-Rect $d
  if (-not $back) { Fail "D's rectangle is gone after restoring it" }
  $dx = [Math]::Abs($back.Left - $originD.Left)
  $dy = [Math]::Abs($back.Top - $originD.Top)
  if ($dx -gt 8 -or $dy -gt 8) { Fail "restoring D put it somewhere else (off by $dx,$dy)" }
  if (Test-Path $journal) { Fail "the journal was not cleared on a normal quit" }
  Write-Host "  everything minimized in place, journal cleared"

  Write-Host "== 6. minimizing a window must not open another card =="
  $proc2 = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 1500
  $e = New-Fixture "E" 4
  $f = New-Fixture "F" 5
  Hang $e
  Hang $f
  if (-not (Wait-Until ({ (Test-Parked $e) -and (Test-Parked $f) }) 6000)) { Fail "E and F were not parked" }
  Take-One $e
  if (-not (Wait-Until ({ Test-OnDesktop $e }) 6000)) { Fail "E did not come back on screen" }
  if (Test-LockScreen) {
    Write-Host "  SKIPPED: the lock screen cannot give a window the foreground"
  } else {
    $r = Get-Rect $e
    if (-not (Click-At ([int](($r.Left + $r.Right) / 2)) ($r.Top + 12))) { Write-Host "  SKIPPED: the pointer could not click E" }
    elseif ([W]::GetForegroundWindow() -ne $e) { Write-Host "  SKIPPED: E could not be made the foreground window" }
    else {
      # The reported sequence: restored and maximized, then minimized. Windows
      # hands the foreground to the next window in the z-order, and a parked
      # window is visible enough to be that next window.
      [void][W]::ShowWindow($e, 3)   # SW_MAXIMIZE
      Start-Sleep -Milliseconds 500
      [void][W]::ShowWindow($e, 2)   # SW_MINIMIZE
      $shown = $false
      $end = [Environment]::TickCount + 2500
      while ([Environment]::TickCount -lt $end) {
        if (Test-OnDesktop $f) { $shown = $true; break }
        Start-Sleep -Milliseconds 25
      }
      if ($shown) { Fail "minimizing E opened F, a window that was on the line" }
      if (-not (Test-Parked $f)) { Fail "F was pulled off the line when E was minimized" }
      if (-not (Wait-Until ({ Test-Hung-Up $e $journal }) 3000)) { Fail "E did not go back on the line" }
      Write-Host "  only E went back on the line"
    }
  }
  Take-All
  Stop-Process -Id $proc2.Id -Force -ErrorAction SilentlyContinue

  Write-Host "PASS"
} finally {
  Stop-ShaiwinWithEverythingDown
  Remove-Fixtures
  Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "cleaned up"
}
