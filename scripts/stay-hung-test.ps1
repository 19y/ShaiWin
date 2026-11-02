# Regression test for the line that keeps its cards - the shipped default.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\stay-hung-test.ps1
#
# Why the setting exists: opening a second window covered the first, that window's
# card came off the rope with it, and the way back was the taskbar. The line is
# meant to be a row of pins you set up once, so a click must never be an unpin.
#
#   1. clicking a card shows its window and leaves the card on the rope
#   2. clicking it again while it is already open only puts it in front: the
#      window does not move, the card does not leave
#   3. a window that comes down in the moment after a show of our own - Windows
#      restores a minimized window together with the windows it owns - goes back
#      up on the line, and its card never leaves; the same window restored with
#      nothing just shown is the user's own restore and the card follows it
#   4. minimizing and maximizing change nothing about the line
#   5. a window moved while it is open comes back where the user left it, not
#      where it was when it first went up
#   6. taking a card down is the only way off the rope, and the window is left
#      exactly where it stands
#   7. letting go of a card removes it and touches no window at all
#   8. kill ShaiWin with a card hanging over an open window, start it again
#      -> the pin is still on the rope and the window was never moved
#
# `stay_hung` is left out of the settings file on purpose: all eight steps are
# about what the app does when nobody configured it, which is the default the
# setting ships on.
#
# Fixtures and a private LOCALAPPDATA, like the other tests: nothing the user was
# working on is moved, closed or clicked. The clicks go to the app as the message
# a card sends (WM_APP+19), so no pointer has to be parked on a card.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe")
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$root = Join-Path $env:TEMP "shaiwin-stay"
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=1`nexcluded=`n"
Initialize-Fixtures $root
$journal = Join-Path $state "ShaiWin\parked.txt"

$shotDir = Join-Path $env:TEMP "shaiwin-stay-shots"
New-Item -ItemType Directory -Force -Path $shotDir | Out-Null
Get-ChildItem $shotDir -Filter "hits-*.txt" -ErrorAction SilentlyContinue | Remove-Item -Force
$env:SHAIWIN_DUMP_DIR = $shotDir

Stop-ShaiwinWithEverythingDown

$script:proc = $null
$script:passed = $false

function Hang([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitHang ([IntPtr]::Zero) ([IntPtr]$hwnd) }
function Open([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitOpen ([IntPtr]::Zero) ([IntPtr]$hwnd) }
function Take([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitTakeOne ([IntPtr]::Zero) ([IntPtr]$hwnd) }
function LetGo([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitUnhook ([IntPtr]::Zero) ([IntPtr]$hwnd) }
function Record([IntPtr]$hwnd) { Get-Record $journal $hwnd }
function Cards { @(Get-CardRects $shotDir $script:proc.Id (Get-Overlay)) }

function Assert-OnLine([IntPtr]$hwnd, [string]$what) {
  $drawn = Cards
  if (-not ($drawn | Where-Object { $_.Hwnd -eq [long]$hwnd })) {
    Fail "$what - and the card of $hwnd is not on the rope. hanging: $((($drawn | ForEach-Object { $_.Hwnd }) -join ', '))"
  }
}

function Assert-OffLine([IntPtr]$hwnd, [string]$what) {
  $drawn = Cards
  if ($drawn | Where-Object { $_.Hwnd -eq [long]$hwnd }) {
    Fail "$what - and the card of $hwnd is still on the rope. hanging: $((($drawn | ForEach-Object { $_.Hwnd }) -join ', '))"
  }
}

function Card-Rect([IntPtr]$hwnd) { Cards | Where-Object { $_.Hwnd -eq [long]$hwnd } | Select-Object -First 1 }

# Where two windows sit relative to each other in the stack: the one nearer the
# front has the smaller rank. There is no desktop to look at here, so when the app
# says it put a window on top, the stack is the first thing asked.
#
# Both ranks come out of a single walk. Two walks are not comparable: the
# invisible topmost windows that sit in front of everything come and go, so a rank
# read a moment later counts a different number of them, and a fixture that is
# really first can read as third. Order within one traversal is the fact; a rank
# against the whole desktop is not.
function Get-ZRanks([IntPtr]$x, [IntPtr]$y) {
  $rx = -1
  $ry = -1
  $rank = 0
  $cur = [W]::GetTopWindow([IntPtr]::Zero)   # the window at the top of the stack
  while ($cur -ne [IntPtr]::Zero -and $rank -lt 4096) {
    if ($cur -eq $x) { $rx = $rank }
    if ($cur -eq $y) { $ry = $rank }
    $cur = [W]::GetWindow($cur, 2)           # GW_HWNDNEXT
    $rank++
  }
  return @{ x = $rx; y = $ry }
}

# True when the first window is in front of the second.
function Test-InFront([IntPtr]$front, [IntPtr]$behind) {
  $r = Get-ZRanks $front $behind
  return ($r.x -ge 0 -and $r.y -ge 0 -and $r.x -lt $r.y)
}

function Test-Foreground([IntPtr]$hwnd) { [W]::GetForegroundWindow() -eq $hwnd }

try {
  $script:proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2000
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay window was not created" }
  Write-Host "shaiwin pid $($script:proc.Id), overlay $overlay"

  $a = New-Fixture "A" 0
  $b = New-Fixture "B" 1
  Hang $a
  Hang $b
  if (-not (Wait-Until ({ (Test-Parked $a) -and (Test-Parked $b) }) 6000)) {
    Fail "A and B did not both hang up"
  }
  if (-not (Wait-Until ({ @(Cards).Count -eq 2 }) 6000)) {
    Fail "two hung windows did not become two cards: $(@(Cards).Count)"
  }

  # --- 1: the click that started all of this ----------------------------------
  Write-Host "== 1. clicking a card shows the window and keeps the card =="
  Open $a
  if (-not (Wait-Until ({ Test-OnDesktop $a }) 5000)) { Fail "clicking A's card did not put A on the desktop" }
  if (-not (Wait-Until ({ (Record $a) -and (Record $a).Shown }) 5000)) {
    Fail "A is open on the desktop and its record does not say the card is keeping it"
  }
  Assert-OnLine $a "A is open on the desktop"
  if ((Record $a).Hooked) {
    Fail "A's record says the hook is behind a hidden card - a card that hangs has nothing waiting to come back"
  }
  if (-not (Test-Parked $b)) { Fail "B came down with A - only the clicked card was asked about" }
  Assert-OnLine $b "B was never clicked"
  Write-Host "  A is on the desktop, its card is still on the rope, B is untouched"

  # --- 2: the second click, on a window that is already there -----------------
  Write-Host "== 2. clicking it again only puts it in front =="
  # B came out second, so A is the covered one - the situation the user is in.
  # Nothing is pushed to the back for the test's convenience.
  Open $b
  if (-not (Wait-Until ({ Test-OnDesktop $b }) 5000)) { Fail "clicking B's card did not put B on the desktop" }
  $before = Get-Rect $a
  $cardBefore = Card-Rect $a
  Open $a
  if (-not (Wait-Until ({ Test-InFront $a $b }) 5000)) {
    $r = Get-ZRanks $a $b
    Fail "clicking the card of a window that is already open did not bring it to the top of the stack: A rank $($r.x), B rank $($r.y)"
  }
  # Then the same from the other side, so what is being read is the raise and not
  # the order the two windows happened to be created in.
  Open $b
  if (-not (Wait-Until ({ Test-InFront $b $a }) 5000)) {
    $r = Get-ZRanks $b $a
    Fail "clicking A's card put A on top and B's card will not put B back on top: B rank $($r.x), A rank $($r.y)"
  }
  $after = Get-Rect $a
  if ($before.Left -ne $after.Left -or $before.Top -ne $after.Top) {
    Fail "raising A moved it, from $($before.Left),$($before.Top) to $($after.Left),$($after.Top)"
  }
  Assert-OnLine $a "A was raised from behind B"
  $cardAfter = Card-Rect $a
  if (-not $cardAfter -or [Math]::Abs($cardAfter.Left - $cardBefore.Left) -gt 4) {
    Fail "the raise moved A's card: $($cardBefore.Left) -> $($cardAfter.Left)"
  }
  # The focus is the other half of the same promise. It is read, not required:
  # Windows hands the foreground to a process that has the rights, and a click
  # sent as a message here is not the input that grants them.
  $focus = if (Test-Foreground $b) { "B holds the focus" } else { "the focus stayed on $([W]::GetForegroundWindow()) - this session grants it to nobody" }
  Write-Host "  each card raises its own window; A is still at $($after.Left),$($after.Top) with its card in the same slot; $focus"

  # --- 3: the window that came down with the one we showed --------------------
  Write-Host "== 3. a click on one card does not bring down the card next to it =="
  # The reported bug: A and B both on the rope, B covering A, click A's card and
  # B lands on the desktop as well.
  #
  # It is not the line opening anything. A minimized window comes back together
  # with the windows it owns, so restoring one window restores its whole group,
  # and two cards can be two windows of one application. The app is never told
  # about the second window - it simply is not parked the next time the windows
  # are read. The tell is that it arrived in the moment after a show of our own;
  # a restore the user did himself (taskbar, Alt+Tab, the window's own button) is
  # his business and the card follows it.
  #
  # One half of that tell cannot be read here: a restore the user did also brings
  # that window the focus, and this session reports no foreground window at all,
  # so the arrival always looks unfocused. What is tested is the arrival itself.
  Take $a
  Take $b
  if (-not (Wait-Until ({ (-not (Record $a)) -and (-not (Record $b)) }) 6000)) {
    $left = @(Read-SharedText $journal | Where-Object { $_.Trim().Length -gt 0 } | ForEach-Object { ($_ -split "`t")[0] })
    $alive = [bool](Get-Process -Id $script:proc.Id -ErrorAction SilentlyContinue)
    Fail "taking A's and B's cards down left a record behind (file=$([bool](Test-Path $journal)) app alive=$alive records=$(($left) -join ', '))"
  }
  Hang $a
  Hang $b
  if (-not (Wait-Until ({ (Test-Parked $a) -and (Test-Parked $b) }) 6000)) {
    Fail "A and B did not both go back up on the rope"
  }

  Open $a
  if (-not (Wait-Until ({ Test-OnDesktop $a }) 6000)) { Fail "clicking A's card did not put A on the desktop" }
  $bWasParked = Test-Parked $b
  [void][W]::ShowWindow($b, 9)   # SW_RESTORE, straight to Windows: the group arrival
  if (-not (Wait-Until ({ Test-Parked $b }) 6000)) {
    $why = if (-not $bWasParked) { "B was already on the desktop before the click, so the arrival proves nothing" } else { "it was left out where Windows put it" }
    Fail "B came down with the window A's card asked for and the line let it stay - ${why}: $(Get-Rect $b)"
  }
  Assert-OnLine $b "B was put back on the line after it arrived unasked"
  if ((Record $b).Shown) { Fail "B's record says the line is keeping a window that is parked" }
  if (-not (Test-OnDesktop $a)) { Fail "putting B back took A off the desktop too" }
  # And the app has to say it did it. Without this line the test cannot tell the
  # repair from a window that never came down at all, and it would pass on a bug.
  $log = Join-Path $state "ShaiWin\shaiwin.log"
  $said = @(Read-SharedText $log | Where-Object { $_ -match "came down with the window we just showed" })
  if (-not $said) {
    Fail "B is parked again and the line never said it put it back - either the window never arrived, or something else moved it"
  }
  Write-Host "  A stayed open, B went back up to the line, its card never left ($($said[0]))"

  # The same arrival when nothing was just shown is the user's own restore, and
  # the card has to follow it rather than fight it.
  Start-Sleep -Milliseconds 1300   # past the moment a show of ours can still explain
  [void][W]::ShowWindow($b, 9)
  if (-not (Wait-Until ({ Test-OnDesktop $b }) 6000)) {
    Fail "restoring B with nothing just shown left it parked - the line took back a window the user brought down himself"
  }
  if (-not (Wait-Until ({ (Record $b) -and (Record $b).Shown }) 6000)) {
    Fail "B is on the desktop and its record does not say the card is keeping it"
  }
  Assert-OnLine $b "B was restored on its own"
  Write-Host "  restored again with nobody just shown, B was left on the desktop with its card"

  # --- 4: the window can do what it likes -----------------------------------
  Write-Host "== 4. minimizing and maximizing are none of the line's business =="
  $place = Get-Rect $a
  $cardBefore = Card-Rect $a
  [void][W]::ShowWindow($a, 6)   # SW_MINIMIZE, the title bar button
  if (-not (Wait-Until ({ (Record $a) -and -not (Record $a).Shown }) 5000)) {
    Fail "A was minimized and its record still claims the window is out on the desktop"
  }
  Assert-OnLine $a "A was minimized"
  if (-not [W]::IsIconic($a)) { Fail "the line put A back on the desktop when it was minimized" }
  $cardAfter = Card-Rect $a
  if (-not $cardAfter) { Fail "A's card vanished when the window was minimized" }
  if ([Math]::Abs($cardAfter.Left - $cardBefore.Left) -gt 4) {
    Fail "minimizing A sent its card somewhere else: $($cardBefore.Left) -> $($cardAfter.Left)"
  }
  Write-Host "  minimized, still hanging, still in the same slot"

  [void][W]::ShowWindow($a, 3)   # SW_MAXIMIZE
  if (-not (Wait-Until ({ [W]::IsZoomed($a) }) 3000)) { Fail "A never maximized" }
  if (-not (Wait-Until ({ (Record $a) -and (Record $a).Shown }) 5000)) {
    Fail "A was maximized and its record does not say the window is on the desktop"
  }
  Assert-OnLine $a "A was maximized"
  Write-Host "  maximized, still hanging"

  # --- 5: the place the user left it ------------------------------------------
  Write-Host "== 5. a window moved while it is open comes back where it was left =="
  [void][W]::ShowWindow($a, 9)   # SW_RESTORE, so the move below is a real move
  if (-not (Wait-Until ({ -not [W]::IsZoomed($a) -and -not [W]::IsIconic($a) }) 3000)) {
    Fail "A would not come out of the maximized state"
  }
  $v = Get-VirtualBounds
  $nx = $v.Left + 70
  $ny = $v.Top + 70
  [void][W]::SetWindowPos($a, [IntPtr]::Zero, $nx, $ny, 0, 0, 0x0001 -bor 0x0004)   # NOSIZE|NOZORDER
  Start-Sleep -Milliseconds 900   # the poll is what notices the window moved
  [void][W]::ShowWindow($a, 6)
  if (-not (Wait-Until ({ (Record $a) -and -not (Record $a).Shown }) 5000)) { Fail "A never went out of sight" }
  Open $a
  if (-not (Wait-Until ({ Test-OnDesktop $a }) 5000)) { Fail "clicking the card of the minimized A did not bring it back" }
  $back = Get-Rect $a
  if ([Math]::Abs($back.Left - $nx) -gt 12 -or [Math]::Abs($back.Top - $ny) -gt 12) {
    Fail "A came back at $($back.Left),$($back.Top) instead of the $nx,$ny it was standing in - the place it was left in was not remembered (it was $($place.Left),$($place.Top) when first shown)"
  }
  Assert-OnLine $a "A came back from a minimize"
  Write-Host "  back at $($back.Left),$($back.Top), where the user had put it"

  # --- 6: the only way off ----------------------------------------------------
  Write-Host "== 6. taking a card down is final, and leaves the window alone =="
  $where = Get-Rect $a
  Take $a
  if (-not (Wait-Until ({ $null -eq (Record $a) }) 6000)) { Fail "taking A's card down left a record behind" }
  Assert-OffLine $a "A's card was taken down"
  $still = Get-Rect $a
  if ($still.Left -ne $where.Left -or $still.Top -ne $where.Top) {
    Fail "taking A's card down moved its window, from $($where.Left),$($where.Top) to $($still.Left),$($still.Top)"
  }
  if (-not (Test-OnDesktop $a)) { Fail "A is not on the desktop after its card was taken down: $still" }
  Write-Host "  card and record gone; A stays at $($still.Left),$($still.Top)"

  # --- 7: letting go ----------------------------------------------------------
  Write-Host "== 7. letting go of a card touches no window =="
  $where = Get-Rect $b
  LetGo $b
  if (-not (Wait-Until ({ $null -eq (Record $b) }) 5000)) { Fail "letting go of B's card left a record behind" }
  Assert-OffLine $b "B's card was let go"
  $still = Get-Rect $b
  if ($still.Left -ne $where.Left -or $still.Top -ne $where.Top) {
    Fail "letting go of B's card moved B, from $($where.Left),$($where.Top) to $($still.Left),$($still.Top)"
  }
  Write-Host "  gone from the rope, still where it was"

  # --- 8: across a crash ------------------------------------------------------
  Write-Host "== 8. kill ShaiWin with a pin over an open window, start it again =="
  $c = New-Fixture "C" 2
  Hang $c
  if (-not (Wait-Until ({ Test-Parked $c }) 6000)) { Fail "C was not hung up" }
  Open $c
  if (-not (Wait-Until ({ (Record $c) -and (Record $c).Shown }) 5000)) { Fail "C is open and its card is not recorded as keeping it" }
  $where = Get-Rect $c
  Stop-OurShaiwin
  $script:proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2200
  if (-not (Test-Window (Get-Overlay))) { Fail "the overlay is gone after the restart" }
  if (-not (Wait-Until ({ (Record $c) -and (Record $c).Shown }) 5000)) { Fail "C's pin was lost with the restart" }
  Assert-OnLine $c "C was open on the desktop when ShaiWin was killed"
  if (-not (Test-OnDesktop $c)) { Fail "the restart did not leave C on the desktop: $(Get-Rect $c)" }
  $still = Get-Rect $c
  if ($still.Left -ne $where.Left -or $still.Top -ne $where.Top) {
    Fail "the restart moved C, from $($where.Left),$($where.Top) to $($still.Left),$($still.Top)"
  }
  Write-Host "  the pin came back and C did not move"

  Write-Host "== 9. take everything down =="
  Invoke-Wm (Get-Overlay) $KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero)
  if (-not (Wait-Until ({ -not (Test-Path $journal) }) 8000)) { Fail "the journal survived taking everything down" }
  $script:passed = $true
  Write-Host "PASS"
} finally {
  # The sandbox is the only place the app's own account of the run exists, and it
  # is deleted here: without this, a failure says what the test saw and never what
  # the app decided.
  if (-not $script:passed) {
    $log = Join-Path $state "ShaiWin\shaiwin.log"
    if (Test-Path $log) {
      Write-Host "--- the app's last 40 lines ---"
      Get-Content $log -Tail 40 | ForEach-Object { Write-Host "  $_" }
    }
  }
  Stop-ShaiwinWithEverythingDown
  Remove-Fixtures
  Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item $shotDir -Recurse -Force -ErrorAction SilentlyContinue
  Write-Host "cleaned up"
}
