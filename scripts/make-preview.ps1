# Builds assets/preview.png from a card sprite dumped by the app.
#
#   1. powershell -File scripts\smoke-test.ps1        # dumps a sprite
#   2. powershell -File scripts\make-preview.ps1
#
# The sprite comes from SHAIWIN_DUMP_SPRITES, so the preview always shows what
# the real renderer draws, not a mock-up.
param(
  [string]$Sprite = "$env:TEMP\shaiwin-sprites\sprite-2.png",
  [string]$Out = "assets\preview.png"
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

if (-not (Test-Path $Sprite)) { throw "no sprite at $Sprite; run scripts\smoke-test.ps1 first" }

$card = [System.Drawing.Bitmap]::FromFile((Resolve-Path $Sprite))
$w = 600
$h = 340
$canvas = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($canvas)
$g.SmoothingMode = 'AntiAlias'

# A calm desktop-ish backdrop, so the translucent card has something to sit on.
$rect = New-Object System.Drawing.Rectangle 0, 0, $w, $h
$brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, ([System.Drawing.Color]::FromArgb(255, 32, 38, 50)), ([System.Drawing.Color]::FromArgb(255, 58, 66, 84)), 90.0
$g.FillRectangle($brush, $rect)
$brush.Dispose()

# The clothesline, sagging slightly, as the app draws it.
$pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(210, 236, 236, 240)), 2.0
$path = New-Object System.Drawing.Drawing2D.GraphicsPath
$path.AddBezier(0, 36, 150, 60, 450, 60, 600, 36)
$g.DrawPath($pen, $path)
$pen.Dispose()
$path.Dispose()

# The card hangs from its pin: the sprite's pivot (22 px from its top) must sit
# on the rope, exactly like the app positions it.
$scale = 1.5
$dw = [int]($card.Width * $scale)
$dh = [int]($card.Height * $scale)
$g.InterpolationMode = 'HighQualityBicubic'
$g.PixelOffsetMode = 'HighQuality'
$g.DrawImage($card, [int](($w - $dw) / 2), [int](54 - 22 * $scale), $dw, $dh)

$g.Dispose()
$canvas.Save((Join-Path (Get-Location) $Out), [System.Drawing.Imaging.ImageFormat]::Png)
$canvas.Dispose()
$card.Dispose()
Write-Host "wrote $Out"
