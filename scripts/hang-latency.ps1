# Where does the time go between letting go of a window and seeing its card?
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\hang-latency.ps1
#
# Hangs three throwaway windows one after another through the automation hook and
# then reads the app's own log back. Every stage of the chain writes one line, so
# the table below is arithmetic on log stamps, not a guess:
#
#   waited  the ask sat behind whatever else the worker was doing
#   shot    PrintWindow, and the settle wait inside it
#   plan    reading the placement and working out where to park
#   park    the reply reaching the UI thread and the window actually moving
#   frame   the park until the card was really painted
#   total   the first line until the last - what the user waits for
#
# The windows here are blank forms, so `shot` is the cost of the plumbing and not
# the cost of a browser painting a frame. A real Chrome or Electron window can
# take much longer inside PrintWindow, and that is exactly what this table is for
# catching when it happens on the user's desktop.
#
# Like the other tests this stops any running ShaiWin first and never touches the
# real %LOCALAPPDATA%\ShaiWin.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe"),
  [ValidateRange(1, 8)]
  [int]$Count = 3
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$script:Hang = [uint32](0x8000 + 10)

# Every log line starts with its millisecond stamp.
function Get-Stamp([string]$line) { return [long](($line -split " ")[0]) }
function Num([string]$line, [string]$pattern) {
  $m = [regex]::Match($line, $pattern)
  if (-not $m.Success) { Fail "the log line does not say what the parser expects: $line" }
  return [int]$m.Groups[1].Value
}
function Find-Line($log, [string]$pattern) {
  @($log | Where-Object { $_ -match $pattern }) | Select-Object -First 1
}

$root = Join-Path $env:TEMP "shaiwin-latency"
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=1`ndrag_hang=1`nexcluded=`n"
$fixtureDir = Join-Path $root "fixtures"
New-Item -ItemType Directory -Force -Path $fixtureDir | Out-Null
Initialize-Fixtures $fixtureDir

$overlay = [IntPtr]::Zero
$proc = $null
try {
  Stop-ShaiwinWithEverythingDown
  $proc = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2200
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the app window is not there" }

  $logPath = Join-Path $state "ShaiWin\shaiwin.log"
  $handles = @()
  for ($i = 0; $i -lt $Count; $i++) {
    # A distinct title per fixture: the helper finds a window by its title, so
    # three forms called the same thing are one form to it.
    $name = "lat$($i + 1)"
    $handles += [pscustomobject]@{ Name = "form $($i + 1)"; Hwnd = (New-Fixture $name $i) }
  }

  Write-Host "== hanging $Count windows, one at a time =="
  foreach ($one in $handles) {
    Invoke-Wm $overlay $script:Hang ([IntPtr]::Zero) ([IntPtr]$one.Hwnd)
    $hwnd = $one.Hwnd
    if (-not (Wait-Until ({ Test-Parked $hwnd }) 8000)) {
      Fail "$($one.Name) was never hung up on the line"
    }
    # The frame line is written by the paint that follows the park, so let it land.
    Start-Sleep -Milliseconds 500
  }

  $log = @(Get-Content $logPath -Encoding UTF8 -ErrorAction SilentlyContinue)
  $rows = @()
  foreach ($one in $handles) {
    $dec = [string][long]$one.Hwnd
    $hex = "0x{0:x}" -f [long]$one.Hwnd
    $ask = Find-Line $log "hang ${dec}: preparing"
    $shot = Find-Line $log "shot ${hex}:"
    $parked = Find-Line $log "parked hwnd=${dec} minimized="
    $drawn = Find-Line $log "card ${dec} drawn"
    if (-not $ask -or -not $shot -or -not $parked -or -not $drawn) {
      Fail ("$($one.Name) has an incomplete chain in the log (ask=$([bool]$ask) " +
            "shot=$([bool]$shot) park=$([bool]$parked) frame=$([bool]$drawn))")
    }
    $waited = Num $shot "waited (\d+) ms"
    $shotMs = Num $shot "shot (\d+) ms"
    $settled = Num $shot "settled (\d+)"
    $planMs = Num $shot "plan (\d+) ms"
    $rows += [pscustomobject]@{
      Window  = $one.Name
      Waited  = $waited
      Shot    = $shotMs
      OfWhich = $settled
      Plan    = $planMs
      Park    = (Get-Stamp $parked) - (Get-Stamp $shot)
      Frame   = Num $drawn "drawn (\d+) ms"
      Total   = (Get-Stamp $drawn) - (Get-Stamp $ask)
    }
  }

  $rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Host

  $worst = $rows | Sort-Object Total -Descending | Select-Object -First 1
  Write-Host "slowest: $($worst.Window) at $($worst.Total) ms"
  # A hang the user cannot feel is under about a fifth of a second; anything past
  # half a second is what he called 卡. Report it, do not decide for him.
  if ($worst.Total -gt 500) { Write-Host "NOTE: that is the lag the user reported - see which column owns it." }
  Write-Host "log: $logPath"
} finally {
  if ($overlay -ne [IntPtr]::Zero) { Invoke-Wm $overlay $script:KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero) }
  Start-Sleep -Milliseconds 1500
  Remove-Fixtures
  if ($proc) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
}
