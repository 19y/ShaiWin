# A language pack imported the way a user imports one (scripts\lang-import-test.ps1).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\lang-import-test.ps1
#
# The Russian pack is not built into the program. It is a file somebody writes and
# drops into %LOCALAPPDATA%\ShaiWin\languages, so this test does exactly that: it
# copies languages\ru.txt into a private sandbox, picks Русский in the settings
# panel, saves, and then looks at what the app drew - the panel, the empty line and
# a card - to see whether the words arrived.
#
# It also checks the two things a user's folder can get wrong: a file named after a
# language the program already ships must not appear twice, and the program's own
# startup export must never write over the pack that was dropped in.
#
# The tag editor is walked through in Russian too, because that is the one screen
# where a translation has to fit words the program never chose: the popover has to
# grow to hold them, the field has to take a Cyrillic tag, and the button that
# clears it has to be the button that clears it.
#
# Nothing of the user's is read or written: the app runs against its own
# LOCALAPPDATA, and the windows it hangs are throwaway fixtures.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe")
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

Add-Type -AssemblyName System.Drawing
if (-not ("Win.Print" -as [type])) {
  Add-Type -Namespace Win -Name Print -MemberDefinition @'
    public delegate bool EnumProc(IntPtr h, IntPtr lp);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr lp);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)]
    public static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder s, int n);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)]
    public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int n);
    // Messages to the field have to be synchronous - PostMessage answers before
    // the control has seen anything - and they have to be the wide ones, because
    // the ANSI entry point converts Cyrillic through the machine's own codepage
    // and the word arrives with extra letters in it.
    [DllImport("user32.dll", EntryPoint="SendMessageW", CharSet=CharSet.Unicode)]
    public static extern IntPtr SendKeyW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
'@
}

$shotDir = Join-Path $env:TEMP "shaiwin-lang-shots"
New-Item -ItemType Directory -Force -Path $shotDir | Out-Null
Get-ChildItem $shotDir -ErrorAction SilentlyContinue | Remove-Item -Force
$env:SHAIWIN_DUMP_DIR = $shotDir

# Cyrillic words spelled from code points: these files have no byte-order mark, and
# PowerShell 5.1 reads a mark-less file as the system's own codepage, which turns
# "Русский" into something the parser cannot even hold.
function Word([int[]]$codes) { return -join ($codes | ForEach-Object { [char]$_ }) }
$ruRussian = Word @(0x0420, 0x0443, 0x0441, 0x0441, 0x043A, 0x0438, 0x0439)
$ruSave = Word @(0x0421, 0x043E, 0x0445, 0x0440, 0x0430, 0x043D, 0x0438, 0x0442, 0x044C)
$ruSystem = Word @(0x0421, 0x0438, 0x0441, 0x0442, 0x0435, 0x043C, 0x043D, 0x044B, 0x0439)
$ruTag = Word @(0x041F, 0x0440, 0x043E, 0x0432, 0x0435, 0x0440, 0x043A, 0x0430)
$ruClear = Word @(0x0423, 0x0431, 0x0440, 0x0430, 0x0442, 0x044C, 0x0020,
                  0x043C, 0x0435, 0x0442, 0x043A, 0x0443)
$zhSave = Word @(0x4FDD, 0x5B58)

function Get-Children([IntPtr]$parent, [string]$want) {
  $script:kids = New-Object System.Collections.ArrayList
  $cb = [Win.Print+EnumProc]{
    param($h, $lp)
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win.Print]::GetClassNameW($h, $sb, 256)
    if ($sb.ToString() -eq $want) { [void]$script:kids.Add($h) }
    return $true
  }
  [void][Win.Print]::EnumChildWindows($parent, $cb, [IntPtr]::Zero)
  return $script:kids.ToArray()
}

function Get-Text([IntPtr]$h) {
  $sb = New-Object System.Text.StringBuilder 512
  [void][Win.Print]::GetWindowTextW($h, $sb, 512)
  return $sb.ToString()
}

# Reading the field from outside the app's process: WM_GETTEXTLENGTH asks the
# control how many characters it holds, and the answer needs no buffer to carry
# across the border between the two processes - so it is the one that can be
# trusted. (GetWindowText and EM_GETLINE are the casualties of that border:
# the first reports what the control was created with, the second reports the
# size word this caller had to hand it.)
function Get-FieldLength([IntPtr]$edit) {
  return [int][Win.Print]::SendKeyW($edit, 0x000E, [IntPtr]::Zero, [IntPtr]::Zero).ToInt64()
}

function Type-Into([IntPtr]$edit, [string]$word) {
  # One character at a time, the way a user types it.
  foreach ($ch in $word.ToCharArray()) {
    [void][Win.Print]::SendKeyW($edit, 0x0102, [IntPtr][int]$ch, [IntPtr]::Zero)
  }
}

function Press-Enter([IntPtr]$h) {
  [void][Win.Print]::SendKeyW($h, 0x0100, [IntPtr]0x0D, [IntPtr]::Zero)
}

function Get-Button-ByText([IntPtr]$parent, [string]$want) {
  @(Get-Children $parent "ShaiWin.Settings.Button" | Where-Object { (Get-Text $_) -eq $want })
}

function Get-Box([IntPtr]$h) {
  $r = Get-Rect $h
  if (-not $r) { Fail "no rect for a child window" }
  $w = $r.Right - $r.Left
  $t = $r.Bottom - $r.Top
  if ($w -lt 2 -or $t -lt 2) { Fail "the control has no size ($w x $t)" }
  return @{ w = $w; t = $t }
}

function Click-At([IntPtr]$h, [int]$x, [int]$y) {
  $lp = [IntPtr](($y -shl 16) -bor ($x -band 0xFFFF))
  Invoke-Wm $h ([uint32]0x0201) ([IntPtr]::Zero) $lp
  Invoke-Wm $h ([uint32]0x0202) ([IntPtr]::Zero) $lp
  Start-Sleep -Milliseconds 250
}

function Click-Middle([IntPtr]$h) {
  $box = Get-Box $h
  Click-At $h ([int]($box.w / 2)) ([int]($box.t / 2))
}

function Save-Shot([IntPtr]$hwnd, [string]$path) {
  $r = New-Object W+RECT
  if (-not [W]::GetWindowRect($hwnd, [ref]$r)) { Fail "no rect for $path" }
  $w = $r.Right - $r.Left
  $h = $r.Bottom - $r.Top
  if ($w -lt 20 -or $h -lt 20) { Fail "the window is not really there ($w x $h)" }
  $bmp = New-Object System.Drawing.Bitmap $w, $h, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $gfx = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $gfx.GetHdc()
  $ok = [Win.Print]::PrintWindow($hwnd, $hdc, 2)
  $gfx.ReleaseHdc($hdc)
  $gfx.Dispose()
  if (-not $ok) { $bmp.Dispose(); Fail "PrintWindow failed for $path" }
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  Write-Host "shot: $path ($w x $h)"
}

function Request-Dump([string]$name) {
  Get-ChildItem $shotDir -Filter "frame-*.png" -ErrorAction SilentlyContinue | Remove-Item -Force
  Invoke-Wm (Get-Overlay) $KitDumpFrame ([IntPtr]::Zero) ([IntPtr]::Zero)
  if (-not (Wait-Until ({ (Get-ChildItem $shotDir -Filter "frame-*.png" -ErrorAction SilentlyContinue).Count -gt 0 }) 6000)) {
    Fail "no frame was dumped for $name (the renderer never ran)"
  }
  $new = Get-ChildItem $shotDir -Filter "frame-*.png" | Select-Object -First 1
  $to = $name
  Move-Item $new.FullName $to -Force
  $bytes = (Get-Item $to).Length
  Write-Host "frame: $to ($bytes bytes)"
  if ($bytes -lt 2000) { Fail "the dumped frame looks empty ($bytes bytes)" }
  return $to
}

$root = Join-Path $env:TEMP "shaiwin-lang"
$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=1`ndrag_hang=1`nexcluded=`nstay_hung=1`nsag=-1"
Initialize-Fixtures $root

$langDir = Join-Path $state "ShaiWin\languages"
New-Item -ItemType Directory -Force $langDir | Out-Null
# The user's own file, copied out of the repository and into the folder the
# settings panel opens. Nothing else about Russian is known to the program.
Copy-Item (Join-Path $PSScriptRoot "..\languages\ru.txt") (Join-Path $langDir "ru.txt") -Force
# A file named after the setting that means "follow the system". It is not a
# language, and the row must not offer it as a second one.
Set-Content -Path (Join-Path $langDir "auto.txt") -Value "_name = Nope`nquit = No`n" -Encoding ascii
$ruBytes = (Get-Item (Join-Path $langDir "ru.txt")).Length

Stop-ShaiwinWithEverythingDown
$proc = Start-Shaiwin $Exe $state
Start-Sleep -Milliseconds 2200
$app = Get-Overlay
if (-not (Test-Window $app)) { Fail "the app window is not there" }

function Open-Panel {
  Invoke-Wm (Get-Overlay) ([uint32](0x8000 + 15)) ([IntPtr]::Zero) ([IntPtr]::Zero)
  if (-not (Wait-Until { Test-Window (Find-By-Class "ShaiWin.Settings") } 5000)) {
    Fail "the settings window never opened"
  }
  Start-Sleep -Milliseconds 900
  return Find-By-Class "ShaiWin.Settings"
}

function Close-Panel([IntPtr]$wnd) {
  Invoke-Wm $wnd ([uint32]0x0010) ([IntPtr]::Zero) ([IntPtr]::Zero)
  Start-Sleep -Milliseconds 400
}

function Save-Button([IntPtr]$wnd) {
  # The right-hand one of the two buttons at the bottom right.
  return @(Get-Children $wnd "ShaiWin.Settings.Button" |
    Sort-Object { (Get-Rect $_).Left } | Select-Object -Last 1)[0]}

function Open-Tag([IntPtr]$card) {
  Invoke-Wm (Get-Overlay) ([uint32](0x8000 + 16)) ([IntPtr]::Zero) ([IntPtr]$card)
  if (-not (Wait-Until { Test-Window (Find-By-Class "ShaiWin.Tag") } 5000)) {
    Fail "the tag editor never opened"
  }
  Start-Sleep -Milliseconds 700
  return Find-By-Class "ShaiWin.Tag"
}

function Close-Tag([IntPtr]$tag) {
  Invoke-Wm $tag ([uint32]0x0010) ([IntPtr]::Zero) ([IntPtr]::Zero)
  if (-not (Wait-Until ({ -not (Test-Window $tag) }) 5000)) {
    Fail "the tag editor would not close"
  }
}

function Width-Of($rect) { return $rect.Right - $rect.Left }
function Tall-Of($rect) { return $rect.Bottom - $rect.Top }

try {
  Write-Host "== the line before anybody speaks Russian =="
  Request-Dump (Join-Path $shotDir "line-before.png") | Out-Null

  Write-Host "== the panel a user sees with a pack in the folder =="
  $wnd = Open-Panel
  Save-Shot $wnd (Join-Path $shotDir "panel-before.png")

  $buttons = @(Get-Children $wnd "ShaiWin.Settings.Button")
  $texts = @($buttons | ForEach-Object { Get-Text $_ })
  Write-Host "  buttons: $($texts -join ' | ')"
  if ((Get-Button-ByText $wnd $ruRussian).Count -ne 1) {
    Fail "the pack in the folder was not offered as $($ruRussian)"
  }
  if (@($texts | Where-Object { $_ -eq "auto" }).Count -gt 0) {
    Fail "auto.txt was offered as a language of its own"
  }
  foreach ($want in @("English", $ruRussian)) {
    if ((Get-Button-ByText $wnd $want).Count -ne 1) { Fail "no single button for $want" }
  }
  Assert-NoCut $state "as it opened"

  Write-Host "== picking the pack and saving =="
  Click-Middle (Get-Button-ByText $wnd $ruRussian)[0]
  Click-Middle (Save-Button $wnd)
  Start-Sleep -Milliseconds 900
  $saved = Get-Content (Join-Path $state "ShaiWin\settings.txt") -Raw
  if ($saved -notmatch "lang=ru") { Fail "the language pick was not saved:`n$saved" }
  $log = Get-Content (Join-Path $state "ShaiWin\shaiwin.log") -Raw -ErrorAction SilentlyContinue
  if ($log -notmatch "language is now ru") { Fail "the switch was not logged:`n$log" }
  Write-Host "  saved lang=ru"

  Write-Host "== the line, with the pack in force =="
  Request-Dump (Join-Path $shotDir "line-empty-ru.png") | Out-Null

  Write-Host "== the panel comes back speaking Russian =="
  Close-Panel $wnd
  $wnd = Open-Panel
  Save-Shot $wnd (Join-Path $shotDir "panel-ru.png")
  if ((Get-Button-ByText $wnd $ruSave).Count -ne 1) { Fail "the panel is not speaking Russian" }
  if ((Get-Button-ByText $wnd "Save").Count -ne 0) { Fail "an English button survived the switch" }
  if ((Get-Button-ByText $wnd $zhSave).Count -ne 0) { Fail "a Chinese button survived the switch" }
  $texts = @(Get-Children $wnd "ShaiWin.Settings.Button" | ForEach-Object { Get-Text $_ })
  Write-Host "  buttons: $($texts -join ' | ')"
  if ((Get-Button-ByText $wnd $ruSystem).Count -ne 1) { Fail "the system row lost its Russian name" }
  Assert-NoCut $state "in Russian"

  Write-Host "== a card hung while the pack is in force =="
  $journal = Join-Path $state "ShaiWin\parked.txt"
  $fixture = New-Fixture "Ru" 0
  Invoke-Wm (Get-Overlay) $KitHang ([IntPtr]::Zero) ([IntPtr]$fixture)
  # A minimized window is still a visible one to Windows, so the journal is the
  # proof the hang went through - not the window being gone.
  if (-not (Wait-Until ({ @(Read-Journal $journal | Where-Object { $_.Hwnd -eq [long]$fixture }).Count -eq 1 }) 8000)) {
    Fail "the fixture window never got on the line"
  }
  Start-Sleep -Milliseconds 1200
  Request-Dump (Join-Path $shotDir "line-card-ru.png") | Out-Null

  Write-Host "== the tag popover, in the pack's own words =="
  $tag = Open-Tag $fixture
  Save-Shot $tag (Join-Path $shotDir "tag-ru-empty.png")
  $first = Get-Rect $tag
  Write-Host "  the popover is $(Width-Of $first) x $(Tall-Of $first)"
  # Card words are painted by the app, not by a control, so the only proof the
  # third button exists is that the popover grew to hold it and said so.
  $grown = (Width-Of $first) -gt [int](286.0 * ((Tall-Of $first) / 136.0))
  $said = @(@(Read-SharedText (Join-Path $state "ShaiWin\shaiwin.log")) |
    Where-Object { $_ -match "the tag popover grew" })
  if ($grown -and $said.Count -eq 0) {
    Fail "the popover grew to $(Width-Of $first)px for this language's words and never logged it"
  }
  if ($grown) { Write-Host "  $($said[0])" }
  Assert-NoCut $state "in the popover's own words"

  Write-Host "== a tag written in Russian, and the third button that comes with it =="
  $edit = @(Get-Children $tag "Edit")
  if ($edit.Count -ne 1) {
    $script:seen = @()
    $cb = [Win.Print+EnumProc]{
      param($h, $lp)
      $sb = New-Object System.Text.StringBuilder 256
      [void][Win.Print]::GetClassNameW($h, $sb, 256)
      $script:seen += $sb.ToString()
      return $true
    }
    [void][Win.Print]::EnumChildWindows($tag, $cb, [IntPtr]::Zero)
    Fail "the popover has $($edit.Count) fields, not one (children: $($script:seen -join ', '))"
  }
  if ((Get-FieldLength $edit[0]) -ne 0) { Fail "a fresh card opens with a word already in its field" }
  Type-Into $edit[0] $ruTag
  if ((Get-FieldLength $edit[0]) -ne $ruTag.Length) {
    Fail ("the field kept {0} of the {1} characters typed" -f (Get-FieldLength $edit[0]), $ruTag.Length)
  }
  Write-Host "  the field holds the word as it was typed"
  # Enter belongs to the field: it commits the tag and takes the popover away.
  Press-Enter $edit[0]
  if (-not (Wait-Until ({ -not (Test-Window $tag) }) 5000)) {
    Fail "the popover stayed after Enter"
  }
  # The log is no evidence here: the editor previews the tag on every keystroke,
  # so by the time Enter commits it, nothing has changed and the app has nothing
  # to say. The journal is the thing that has to be right.
  if (-not (Wait-Until ({
      @(Read-Journal $journal | Where-Object { $_.Hwnd -eq [long]$fixture -and $_.Tag -eq $ruTag }).Count -eq 1
    }) 6000)) {
    $now = @(Read-Journal $journal | Where-Object { $_.Hwnd -eq [long]$fixture })
    Fail "the journal keeps the card without its Russian tag (tag: $($($now[0]).Tag))"
  }
  Write-Host "  the tag is on the journal in Russian"

  Start-Sleep -Milliseconds 600
  Request-Dump (Join-Path $shotDir "line-card-tagged-ru.png") | Out-Null
  $tag = Open-Tag $fixture
  Save-Shot $tag (Join-Path $shotDir "tag-ru-filled.png")
  $wide = Get-Rect $tag
  Write-Host "  the popover is $(Width-Of $wide) x $(Tall-Of $wide) with all three words"
  # The third button is there before there is anything to clear, so writing a tag
  # must not make the popover jump a size on its way back open.
  if ((Width-Of $wide) -ne (Width-Of $first)) {
    Fail "the popover changed width: $(Width-Of $first) to $(Width-Of $wide)"
  }
  $again = @(Get-Children $tag "Edit")[0]
  if ((Get-FieldLength $again) -ne $ruTag.Length) {
    Fail ("the reopened field holds {0} of the {1} characters it was given" -f `
      (Get-FieldLength $again), $ruTag.Length)
  }
  Write-Host "  reopened, the field holds the tag it was given"
  Assert-NoCut $state "in the popover with all three words"
  Close-Tag $tag

  Write-Host "== the third button does what its Russian word says =="
  # The buttons are painted, not controls, so the click is aimed by the popover's
  # own geometry: 136 logical points tall, 14 of padding, a 30-point row of
  # buttons at the bottom, the clear button the left-hand one of that row.
  $tag = Open-Tag $fixture
  $box = Get-Rect $tag
  $scale = (Tall-Of $box) / 136.0
  Click-At $tag ([int]((14.0 + 20.0) * $scale)) ([int]((136.0 - 14.0 - 15.0) * $scale))
  if (-not (Wait-Until ({ -not (Test-Window $tag) }) 5000)) {
    Fail "the left-hand button did not take the popover away"
  }
  if (-not (Wait-Until ({
      @(Read-Journal $journal | Where-Object { $_.Hwnd -eq [long]$fixture -and $_.Tag -eq "" }).Count -eq 1
    }) 6000)) {
    $now = @(Read-Journal $journal | Where-Object { $_.Hwnd -eq [long]$fixture })
    Fail "the tag survived its own clear button (tag: $($($now[0]).Tag))"
  }
  Write-Host "  $($ruClear) cleared it"

  Write-Host "== the user's file is still the user's file =="
  if ((Get-Item (Join-Path $langDir "ru.txt")).Length -ne $ruBytes) {
    Fail "the program rewrote the pack it was given"
  }
  foreach ($shipped in "en.txt", "zh-CN.txt") {
    if (-not (Test-Path (Join-Path $langDir $shipped))) { Fail "startup left no $shipped in the folder" }
  }
  Write-Host "  ru.txt untouched, both shipped languages exported"

  Write-Host "== back to following the system =="
  Click-Middle (Get-Button-ByText $wnd $ruSystem)[0]
  Click-Middle (Save-Button $wnd)
  Start-Sleep -Milliseconds 900
  $saved = Get-Content (Join-Path $state "ShaiWin\settings.txt") -Raw
  if ($saved -notmatch "lang=auto") { Fail "the way back was not saved:`n$saved" }
  Close-Panel $wnd
  $wnd = Open-Panel
  Save-Shot $wnd (Join-Path $shotDir "panel-back-to-system.png")
  if ((Get-Button-ByText $wnd $zhSave).Count -ne 1) { Fail "the panel did not return to the system language" }
  Write-Host "  the panel is Chinese again"

  Write-Host "== the folder door does not take the app with it =="
  # The door is the leftmost button in the same row as Save; which word is on it is
  # exactly what the pack decides, so it is found by place and not by name.
  $all = @(Get-Children $wnd "ShaiWin.Settings.Button")
  $saveTop = (Get-Rect (Save-Button $wnd)).Top
  $door = @($all | Where-Object { (Get-Rect $_).Top -ge ($saveTop - 5) } |
    Sort-Object { (Get-Rect $_).Left })[0]
  Click-Middle $door
  Start-Sleep -Milliseconds 800
  if (-not (Test-Window (Get-Overlay))) { Fail "the app died when the language folder was opened" }
  Write-Host "  still alive after the door"
} finally {
  Remove-Fixtures
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  Stop-OurShaiwin
}

Write-Host "done: $shotDir"
