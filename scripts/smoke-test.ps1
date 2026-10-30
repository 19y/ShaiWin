# End-to-end smoke test for ShaiWin (Windows).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-test.ps1
#
# It drives the real app through four states:
#   1. hang a throwaway window                (capture -> journal -> park)
#   2. dump the composed frame                (does the renderer draw?)
#   3. kill ShaiWin, start it again           (crash recovery)
#   4. take everything down through the app   (restore + journal cleared)
#
# The windows are fixtures from scripts\testkit.ps1, so no document of the user
# is ever moved or closed, and the app runs against its own LOCALAPPDATA.
#
# The hooks it uses are part of the app, not test-only back doors:
#   WM_APP+10 with lParam = HWND  -> hang that window
#   WM_APP+11                     -> take everything down
#   WM_APP+12                     -> write the next frame to $env:SHAIWIN_DUMP_DIR
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe"),
  [switch]$KeepRunning
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$root = Join-Path $env:TEMP "shaiwin-smoke"
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=1`nexcluded=`n"
Initialize-Fixtures $root
$journal = Join-Path $state "ShaiWin\parked.txt"

$shotDir = Join-Path $env:TEMP "shaiwin-shots"
New-Item -ItemType Directory -Force -Path $shotDir | Out-Null
Get-ChildItem $shotDir -Filter "frame-*.png" -ErrorAction SilentlyContinue | Remove-Item -Force
$env:SHAIWIN_DUMP_DIR = $shotDir
$spriteDir = Join-Path $env:TEMP "shaiwin-sprites"
New-Item -ItemType Directory -Force -Path $spriteDir | Out-Null
Get-ChildItem $spriteDir -Filter "*.png" -ErrorAction SilentlyContinue | Remove-Item -Force
$env:SHAIWIN_DUMP_SPRITES = $spriteDir

Write-Host "== 0. clean slate =="
Stop-ShaiwinWithEverythingDown

function Request-Dump([string]$path) {
  Get-ChildItem $shotDir -Filter "frame-*.png" -ErrorAction SilentlyContinue | Remove-Item -Force
  Invoke-Wm (Get-Overlay) $KitDumpFrame ([IntPtr]::Zero) ([IntPtr]::Zero)
  Start-Sleep -Milliseconds 600
  $new = Get-ChildItem $shotDir -Filter "frame-*.png" -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $new) { Fail "no frame was dumped (the renderer never ran)" }
  Move-Item $new.FullName $path -Force
  $bytes = (Get-Item $path).Length
  Write-Host "frame: $path ($bytes bytes)"
  if ($bytes -lt 2000) { Fail "dumped frame looks empty ($bytes bytes)" }
}

try {
  Write-Host "== 1. start =="
  $canPointer = Test-CursorControl
  $proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2200
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay window was not created" }
  $rect = Get-Rect $overlay
  Write-Host "overlay ${overlay}: visible=$([W]::IsWindowVisible($overlay)) $($rect.Left),$($rect.Top) $($rect.Right - $rect.Left)x$($rect.Bottom - $rect.Top)"
  if ($canPointer) {
    # always_on=1: a running ShaiWin shows its line. The hover/reveal machine is
    # tested with always_on=0 in reveal-test.ps1.
    if (-not [W]::IsWindowVisible($overlay)) { Fail "the line was not down at startup with always_on=1" }
  } else {
    Write-Host "  startup visibility not checked: this session has no pointer to read"
  }

  Write-Host "== 2. hang a fixture =="
  $a = New-Fixture "A" 0
  $origin = Get-Rect $a
  Write-Host "target hwnd $a at $($origin.Left),$($origin.Top) $($origin.Right - $origin.Left)x$($origin.Bottom - $origin.Top)"
  Invoke-Wm $overlay $KitHang ([IntPtr]::Zero) ([IntPtr]$a)
  if (-not (Wait-Until ({ Test-Parked $a }) 5000)) { Fail "the window was not parked off screen" }
  if (-not (Test-Path $journal)) { Fail "parked.txt was not written" }
  Copy-Item $journal (Join-Path $env:TEMP "shaiwin-journal-copy.txt") -Force
  if (-not (Wait-Visibility $overlay $true 2500)) { Fail "the line did not appear after hanging a window" }
  Request-Dump (Join-Path $shotDir "shot-01.png")

  Write-Host "== 3. kill and recover =="
  Get-Process shaiwin | Stop-Process -Force
  Start-Sleep -Milliseconds 600
  if (-not (Test-Path $journal)) { Fail "journal vanished after a forced kill" }
  $proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2200
  $overlay = Get-Overlay
  if (-not (Wait-Visibility $overlay $true 2500)) { Fail "the recovered line is not visible" }
  if (-not (Test-Parked $a)) { Fail "the recovered window is not parked any more" }
  Request-Dump (Join-Path $shotDir "shot-02.png")
  Write-Host "  recovered, window still parked at $((Get-Rect $a).Left)"

  Write-Host "== 4. take everything down =="
  Invoke-Wm $overlay $KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero)
  if (-not (Wait-Until ({ -not (Test-Parked $a) }) 6000)) { Fail "the window stayed parked after taking everything down" }
  $back = Get-Rect $a
  $dx = [Math]::Abs($back.Left - $origin.Left)
  $dy = [Math]::Abs($back.Top - $origin.Top)
  if ($dx -gt 8 -or $dy -gt 8) { Fail "window did not return to its original position (off by $dx,$dy)" }
  # The window is back the moment `restore` moves it; the journal is rewritten a
  # few hundred microseconds later, in the same call. Waiting on the file is the
  # promise ("nothing left hanging means nothing left in the journal"); reading
  # it the instant the window lands is racing the app's own order of operations.
  if (-not (Wait-Until ({ -not (Test-Path $journal) }) 3000)) {
    # Say what is actually in there. "The file exists" is not a bug report.
    Write-Host "  parked.txt holds:"
    Get-Content $journal -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "    $_" }
    Write-Host "  last lines of the log:"
    Get-Content (Join-Path (Split-Path $journal) "shaiwin.log") -Tail 12 |
      ForEach-Object { Write-Host "    $_" }
    Fail "parked.txt still exists after taking everything down"
  }
  if ($canPointer) {
    # The line is resident, so emptying it is not a reason for it to slip away;
    # that is what the toggle shortcut is for.
    if (-not [W]::IsWindowVisible($overlay)) {
      Fail "the pinned line hid itself when the last card came down"
    }
  } else {
    Write-Host "  final visibility not checked: this session has no pointer to read"
  }
  Write-Host "  back at $($back.Left),$($back.Top), journal cleared"

  Write-Host "PASS"
} finally {
  if (-not $KeepRunning) {
    Stop-ShaiwinWithEverythingDown
    Remove-Fixtures
    Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "cleaned up"
  }
}
