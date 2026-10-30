# Produces the README picture (scripts\usage-shot.ps1).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\usage-shot.ps1
#   node scripts\trim-frame.mjs %TEMP%\shaiwin-usage\shots\frame.png assets\usage.png --pad 200
#
# Nothing of the user's is in the frame. The windows that get hung are four
# throwaway WinForms programs compiled into %TEMP% by this script, and the app
# runs against its own LOCALAPPDATA. The picture itself is the app's real
# composed frame: WM_APP+12 asks the renderer to write the next frame it draws
# to disk, so the clothesline, the pins, the colours and the shadows are the
# ones on screen, not a redraw.
param(
  [string]$Exe = (Join-Path $PSScriptRoot "..\target\release\shaiwin.exe"),
  [string]$Out = ""
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\testkit.ps1"

$Exe = (Resolve-Path $Exe).Path
$root = Join-Path $env:TEMP "shaiwin-usage"
$shotDir = Join-Path $root "shots"
$demoDir = Join-Path $root "demo"
if (-not $Out) { $Out = Join-Path $shotDir "frame.png" }
Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue

$state = New-Sandbox $root "welcomed=1`nreduced_motion=0`nall_monitors=0`nalways_on=1`nexcluded=`n"
New-Item -ItemType Directory -Force -Path $shotDir, $demoDir | Out-Null
$env:SHAIWIN_DUMP_DIR = $shotDir

# One program, copied to four names, because the card colour comes from the
# executable - four names is what makes the picture show the same-app rule and
# the different-app rule at the same time.
$DemoSource = @'
using System;
using System.Drawing;
using System.Windows.Forms;

static class Demo {
  static Label Line(string text, int x, int y, int w, Color c, bool bold, string face) {
    return new Label {
      Text = text,
      Location = new Point(x, y),
      Size = new Size(w, face == null ? 22 : 20),
      ForeColor = c,
      Font = new Font(face ?? "Segoe UI", face == null ? (bold ? 12f : 10f) : 9.5f, bold ? FontStyle.Bold : FontStyle.Regular),
    };
  }

  static Panel Tile(int x, int y, int w, int h, Color c) {
    return new Panel { Location = new Point(x, y), Size = new Size(w, h), BackColor = c };
  }

  [STAThread]
  static void Main(string[] args) {
    Application.EnableVisualStyles();
    string title = args.Length > 0 ? args[0] : "ShaiWin demo";
    string kind = args.Length > 1 ? args[1] : "board";
    int x = args.Length > 2 ? int.Parse(args[2]) : 120;
    int y = args.Length > 3 ? int.Parse(args[3]) : 260;

    var f = new Form {
      Text = title,
      StartPosition = FormStartPosition.Manual,
      Location = new Point(x, y),
      Size = new Size(560, 380),
      Font = new Font("Segoe UI", 10f),
    };

    if (kind == "board") {
      f.BackColor = Color.FromArgb(37, 45, 66);
      f.Controls.Add(Line("Sprint 14", 18, 14, 300, Color.White, true, null));
      Color[] tiles = {
        Color.FromArgb(64, 178, 255), Color.FromArgb(255, 138, 101), Color.FromArgb(102, 187, 106),
        Color.FromArgb(255, 193, 7), Color.FromArgb(229, 115, 115), Color.FromArgb(38, 198, 218),
      };
      for (int i = 0; i < tiles.Length; i++) {
        f.Controls.Add(Tile(18 + (i % 3) * 172, 50 + (i / 3) * 116, 160, 104, tiles[i]));
      }
      f.Controls.Add(Line("12 open     5 today     3 blocked", 18, 292, 420, Color.FromArgb(168, 182, 208), false, null));
    } else if (kind == "notes") {
      string[] lines = {
        "the line follows the pointer across monitors",
        "cards keep their order after a restart",
        "snapshot: PrintWindow first, then the screen",
        "a taken-down card is still hooked",
        "minimise it and it comes back on its own",
        "take down is not the same as unhook",
      };
      f.Controls.Add(Line("Weekly sync", 18, 14, 300, Color.Black, true, null));
      for (int i = 0; i < lines.Length; i++) {
        f.Controls.Add(Line("-  " + lines[i], 18, 52 + i * 28, 520, Color.FromArgb(40, 44, 52), false, null));
      }
      f.Controls.Add(Line("next: scroll the line with the wheel", 18, 236, 520, Color.FromArgb(120, 96, 200), false, null));
    } else if (kind == "queue") {
      f.Controls.Add(Line("Build queue", 18, 14, 300, Color.Black, true, null));
      var lv = new ListView {
        Location = new Point(18, 50),
        Size = new Size(520, 260),
        View = View.Details,
        FullRowSelect = true,
        GridLines = true,
        HeaderStyle = ColumnHeaderStyle.Nonclickable,
      };
      lv.Columns.Add("Job", 220);
      lv.Columns.Add("Stage", 130);
      lv.Columns.Add("Age", 90);
      string[][] rows = {
        new[] { "core-check", "passed", "4 m" },
        new[] { "platform-win-build", "passed", "6 m" },
        new[] { "pack-exe", "running", "1 m" },
        new[] { "smoke-single-monitor", "queued", "-" },
        new[] { "smoke-dual-monitor", "queued", "-" },
        new[] { "docs-render", "queued", "-" },
      };
      foreach (var r in rows) lv.Items.Add(new ListViewItem(r));
      f.Controls.Add(lv);
    } else {
      f.BackColor = Color.FromArgb(24, 28, 44);
      f.Controls.Add(Line("app.rs", 18, 14, 300, Color.FromArgb(150, 160, 190), true, null));
      var code = new[] {
        new { Text = "fn redraw(&mut self) {", C = Color.FromArgb(198, 120, 221) },
        new { Text = "    let canvas = self.compose();", C = Color.FromArgb(224, 228, 240) },
        new { Text = "    if let Some(canvas) = canvas {", C = Color.FromArgb(198, 120, 221) },
        new { Text = "        self.present(&canvas);", C = Color.FromArgb(224, 228, 240) },
        new { Text = "    }", C = Color.FromArgb(224, 228, 240) },
        new { Text = "}", C = Color.FromArgb(198, 120, 221) },
        new { Text = "", C = Color.White },
        new { Text = "// paint every tick the timer runs,", C = Color.FromArgb(110, 122, 152) },
        new { Text = "// including the tick a card lands on", C = Color.FromArgb(110, 122, 152) },
      };
      for (int i = 0; i < code.Length; i++) {
        f.Controls.Add(Line("    " + code[i].Text, 18, 50 + i * 24, 520, code[i].C, false, "Consolas"));
      }
    }
    Application.Run(f);
  }
}
'@

Add-Type -OutputAssembly (Join-Path $demoDir "shaiwin-demo.exe") -OutputType WindowsApplication `
  -ReferencedAssemblies @("System", "System.Drawing", "System.Windows.Forms") -TypeDefinition $DemoSource

$shots = @(
  @{ Name = "board";  Title = "Sprint 14";   Index = 0 },
  @{ Name = "notes";  Title = "Weekly sync"; Index = 1 },
  @{ Name = "queue";  Title = "Build queue"; Index = 2 },
  @{ Name = "editor"; Title = "app.rs";      Index = 3 }
)
foreach ($s in $shots) {
  Copy-Item (Join-Path $demoDir "shaiwin-demo.exe") (Join-Path $demoDir "$($s.Name).exe") -Force
}

function Find-By-Pid([int]$processId) {
  $cb = [W+EnumProc]{
    param($h, $lp)
    $pidRef = [uint32]0
    [void][W]::GetWindowThreadProcessId($h, [ref]$pidRef)
    if ([int64]$pidRef -eq [int64]$lp -and [W]::IsWindowVisible($h)) {
      $sb = New-Object System.Text.StringBuilder 512
      [void][W]::GetWindowTextW($h, $sb, 512)
      if ($sb.Length -gt 0) { $script:byPid = $h; return $false }
    }
    return $true
  }
  $script:byPid = [IntPtr]::Zero
  [void][W]::EnumWindows($cb, [uint32]$processId)
  return $script:byPid
}

try {
  Write-Host "== 0. clean slate =="
  Stop-ShaiwinWithEverythingDown

  Write-Host "== 1. four demo windows =="
  $handles = @()
  foreach ($s in $shots) {
    $v = Get-VirtualBounds
    $x = $v.Left + 90 + $s.Index * 150
    $y = $v.Top + 260 + ($s.Index % 2) * 60
    # Start-Process joins -ArgumentList with spaces and quotes nothing itself,
    # so a title with a space has to arrive already quoted.
    $proc = Start-Process (Join-Path $demoDir "$($s.Name).exe") -ArgumentList @("`"$($s.Title)`"", $s.Name, $x, $y) -PassThru
    $script:fixturePids += @{ Title = $s.Title; Id = $proc.Id }
    if (-not (Wait-Until ({ (Find-By-Pid $proc.Id) -ne [IntPtr]::Zero }) 20000)) {
      Fail "demo window $($s.Title) never showed (pid $($proc.Id))"
    }
    $h = Find-By-Pid $proc.Id
    $script:fixtureHandles += $h
    $handles += $h
    Write-Host "  $($s.Title): hwnd $h"
  }
  Start-Sleep -Milliseconds 800

  Write-Host "== 2. start the app and hang them =="
  $app = Start-Shaiwin $Exe $state
  Start-Sleep -Milliseconds 2200
  $overlay = Get-Overlay
  if (-not (Test-Window $overlay)) { Fail "the overlay window was not created" }
  foreach ($h in $handles) {
    Invoke-Wm $overlay $KitHang ([IntPtr]::Zero) ([IntPtr]$h)
    Start-Sleep -Milliseconds 400
  }
  if (-not (Wait-Until ({ (Get-CardRects $shotDir $app.Id (Get-Overlay)).Count -eq $handles.Count }) 8000)) {
    Fail "not every demo window became a card"
  }
  # The flight and the spring settle in well under a second; the wait is so the
  # dump catches the cards at rest, which is what a README shows.
  Start-Sleep -Milliseconds 2500
  Invoke-Wm $overlay $KitDumpFrame ([IntPtr]::Zero) ([IntPtr]::Zero)
  if (-not (Wait-Until ({ (Get-ChildItem $shotDir -Filter "frame-*.png" -ErrorAction SilentlyContinue).Count -gt 0 }) 3000)) {
    Fail "no frame was dumped (the renderer never ran)"
  }
  $frame = Get-ChildItem $shotDir -Filter "frame-*.png" | Sort-Object Name | Select-Object -Last 1
  Move-Item $frame.FullName $Out -Force
  Write-Host "frame: $Out ($((Get-Item $Out).Length) bytes)"

  Write-Host "== 3. take everything down =="
  Invoke-Wm $overlay $KitTakeAll ([IntPtr]::Zero) ([IntPtr]::Zero)
  Start-Sleep -Milliseconds 1500
  Write-Host "PASS  next: node scripts/trim-frame.mjs `"$Out`" assets\usage.png --pad 200"
} finally {
  Stop-ShaiwinWithEverythingDown
  Remove-Fixtures
  Write-Host "left: $root (delete when done)"
}
