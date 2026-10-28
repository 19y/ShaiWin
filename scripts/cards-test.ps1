# Regression test for the cards hanging on the line (Windows).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\cards-test.ps1
#
# Each step is one of the bugs reported from real use:
#   1. several cards piled on the same spot, so clicking one took another window
#      down - the line has to lay them out in its real order
#   2. a card dragged along the line sliding back on its own afterwards
#   3. clicking a card has to bring back the window that card belongs to, and
#      only that one
#   4. a card that has just hung is clickable on the first frame it appears
#      (it used to wait for a fade that took fifteen seconds)
#   5. the line must not keep compositing at 60 Hz while nothing moves, which is
#      what felt sticky
#
# The drag and the clicks need a pointer that can actually be moved; a session
# with no input desktop skips those steps and still checks 4 and 5.
#
# Fixtures and a private LOCALAPPDATA, like the other tests: nothing the user was
# working on is moved, closed or clicked.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe")
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$root = Join-Path $env:TEMP "shaiwin-cards"
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nhover_open=0`nalways_on=1`nexcluded=`n"
Initialize-Fixtures $root
$journal = Join-Path $state "ShaiWin\parked.txt"

$shotDir = Join-Path $env:TEMP "shaiwin-cards-shots"
New-Item -ItemType Directory -Force -Path $shotDir | Out-Null
Get-ChildItem $shotDir -Filter "hits-*.txt" -ErrorAction SilentlyContinue | Remove-Item -Force
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
    }
  })
}
function Rect-For($rects, [IntPtr]$h) { $rects | Where-Object { $_.Hwnd -eq [long]$h } | Select-Object -First 1 }
function Hang([IntPtr]$hwnd) { Invoke-Wm (Get-Overlay) $KitHang ([IntPtr]::Zero) ([IntPtr]$hwnd) }

try {
  Write-Host "== 0. start and hang three windows =="
  $script:proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2000
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay window was not created" }

  $a = New-Fixture "A" 0
  $b = New-Fixture "B" 1
  $c = New-Fixture "C" 2
  Hang $a; Hang $b; Hang $c
  if (-not (Wait-Until ({ @(Order).Count -eq 3 }) 6000)) { Fail "three windows did not become three records" }
  if (-not (Wait-Until ({ @(Rects $overlay).Count -eq 3 }) 6000)) { Fail "three cards were not drawn" }

  Write-Host "== 1. three cards, three places =="
  $rects = Rects $overlay
  $ordered = @($rects | Sort-Object Left)
  foreach ($r in $ordered) {
    Write-Host "  card $($r.Hwnd) covers $($r.Left),$($r.Top) to $($r.Right),$($r.Bottom)"
  }
  for ($i = 1; $i -lt $ordered.Count; $i++) {
    $gap = $ordered[$i].Left - $ordered[$i - 1].Right
    if ($gap -lt 0) { Fail "cards overlap by $([Math]::Abs($gap))px - clicking one would hit another" }
  }
  $drawn = @($rects | Sort-Object Left | ForEach-Object { $_.Hwnd })
  if ((@(Order) -join ",") -ne ($drawn -join ",")) {
    Fail "the cards are not drawn in the order the line keeps them: journal $(@(Order) -join ',') vs drawn $($drawn -join ',')"
  }

  if ($canPointer) {
    Write-Host "== 2. drag the last card to the front of the line =="
    $first = $ordered[0]
    $last = $ordered[$ordered.Count - 1]
    $from = Get-Center $last
    $to = Get-Center $first
    if (-not (Drag-From-To $from.X $from.Y $to.X $to.Y 10)) { Fail "lost cursor control during the drag" }
    if (-not (Wait-Until ({ @(Order)[0] -eq [long]$last.Hwnd }) 4000)) {
      Fail "the dragged card did not stay at the front: order is $(@(Order) -join ',')"
    }
    Start-Sleep -Milliseconds 1200
    $after = Rects $overlay
    $moved = Rect-For $after $last.Hwnd
    if (-not $moved) { Fail "the dragged card vanished from the line" }
    $drift = [Math]::Abs($moved.Left - $first.Left)
    if ($drift -gt 24) { Fail "the card slid back $drift px after being dropped" }
    Write-Host "  it stayed at the front ($drift px from the first card's old spot)"

    Write-Host "== 3. clicking a card takes that window down, and only that one =="
    $middle = Rect-For (Rects $overlay) $first.Hwnd
    if (-not $middle) { Fail "the card that used to be first is not on the line" }
    $point = Get-Center $middle
    if (-not (Click-At $point.X $point.Y)) { Fail "lost cursor control" }
    if (-not (Wait-Until ({ (Get-Record $journal $first.Hwnd).Hooked }) 5000)) {
      Fail "clicking its card did not bring window $($first.Hwnd) back on the desktop"
    }
    $clicked = [IntPtr]$first.Hwnd
    if (-not (Wait-Until ({ Test-OnDesktop $clicked }) 5000)) { Fail "window $clicked is hooked but not on screen" }
    if (-not (Test-Parked $b)) { Fail "window B came down even though its card was not clicked" }
    if (-not (Test-Parked $c)) { Fail "window C came down even though its card was not clicked" }
    Write-Host "  only the clicked window came back; the others stayed hung"

    Write-Host "== 4. a freshly hung card can be clicked straight away =="
    $d = New-Fixture "D" 3
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    Hang $d
    $seen = $null
    while ($stopwatch.Elapsed.TotalMilliseconds -lt 1500 -and -not $seen) {
      $seen = Rect-For (Rects $overlay) $d
    }
    $stopwatch.Stop()
    if (-not $seen) { Fail "the new card had no clickable rectangle after 1.5 s (the fade that had to finish first)" }
    Write-Host "  clickable $([int]$stopwatch.Elapsed.TotalMilliseconds) ms after the hang"

    # The clothespin is drawn above the card body. Grabbing it used to pass the
    # click through to whatever was underneath, which read as "sometimes a card
    # just will not move".
    Write-Host "== 4b. the clothespin belongs to the card =="
    $pin = Rect-For (Rects $overlay) $d
    if (-not $pin) { Fail "the new card has no rectangle" }
    $pinX = [int](($pin.Left + $pin.Right) / 2)
    $pinY = [int]$pin.Top + 4
    if (-not (Click-At $pinX $pinY)) { Fail "lost cursor control" }
    if (-not (Test-OnDesktop $d)) {
      if (-not (Wait-Until ({ Test-OnDesktop $d }) 5000)) {
        Fail "clicking the clothespin at $pinX,$pinY did nothing - it is not part of the card"
      }
    }
    Write-Host "  the clothespin is part of the card"
  } else {
    Write-Host "== 2-4. SKIPPED: this session has no pointer to move =="
    $d = New-Fixture "D" 3
    Hang $d
    if (-not (Wait-Until ({ @(Rects $overlay).Count -eq 4 }) 6000)) {
      Fail "the fourth card was never drawn"
    }
    $seen = Rect-For (Rects $overlay) $d
    if (-not $seen) { Fail "the fourth card has no clickable rectangle" }
  }

  Write-Host "== 5. a line with nothing moving costs nothing =="
  if ($canPointer) {
    $v = Get-VirtualBounds
    [void](Move-Cursor ([int](($v.Left + $v.Right) / 2)) ([int]($v.Bottom - 100)))
  }
  Start-Sleep -Milliseconds 1500
  $quiet = [TimeSpan]::MaxValue
  for ($i = 0; $i -lt 4; $i++) {
    $p0 = (Get-Process -Id $script:proc.Id).TotalProcessorTime
    Start-Sleep -Milliseconds 2000
    $p1 = (Get-Process -Id $script:proc.Id).TotalProcessorTime
    $used = $p1 - $p0
    if ($used -lt $quiet) { $quiet = $used }
    Write-Host "  window $i : $([int]$used.TotalMilliseconds) ms of cpu"
  }
  if ($quiet.TotalMilliseconds -gt 400) {
    Fail "the quietest 2 s still cost $([int]$quiet.TotalMilliseconds) ms - the line is compositing without end"
  }
  Write-Host "  quietest 2 s: $([int]$quiet.TotalMilliseconds) ms"

  Write-Host "== 6. minimising them one after another gives each its own slot =="
  # What the user described as 点击缩小后都回退到衣架的同一个位置: put every
  # window back on the desktop (still hooked), then send them up again the way
  # the taskbar does. Each one has to land in a slot of its own.
  $all = @($a, $b, $c, $d)
  foreach ($h in $all) {
    Invoke-Wm (Get-Overlay) $KitTakeOne ([IntPtr]::Zero) ([IntPtr]$h)
    if (-not (Wait-Until ({ -not (Test-Parked $h) }) 6000)) { Fail "window $h never came down" }
  }
  Start-Sleep -Milliseconds 800
  foreach ($h in $all) {
    $why = Minimize-Hang $h $journal
    if ($why) { Fail "minimizing $h did not hang it up: $why" }
  }
  if (-not (Wait-Until ({ @(Rects $overlay).Count -eq 4 }) 6000)) {
    Fail "after four minimizes the line shows $(@(Rects $overlay).Count) cards, not 4"
  }
  $again = @(Rects $overlay)
  $byLeft = @($again | Sort-Object Left)
  for ($i = 1; $i -lt $byLeft.Count; $i++) {
    if ($byLeft[$i].Left - $byLeft[$i - 1].Left -lt 40) {
      Fail "two cards share a spot on the line - $($byLeft[$i - 1].Hwnd) and $($byLeft[$i].Hwnd) are both at $([int]$byLeft[$i].Left)"
    }
  }
  Write-Host "  four windows, four slots: $(($byLeft | ForEach-Object { $_.Left }) -join ', ')"

  Write-Host "== 7. take everything down =="
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
