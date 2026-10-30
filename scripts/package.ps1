# Build and package the way the Release workflow does, on this machine.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1
#
# The steps, the flags and the checks are the ones in
# .github/workflows/release.yml, in that order, so what comes out of this script
# is the artifact that comes out of the runner. The one difference is the version:
# the workflow reads it from the tag it was triggered by, and here there is no
# tag, so it reads it from Cargo.toml.
#
# What makes it a release build rather than a `cargo build --release` is
# RUSTFLAGS=-C target-feature=+crt-static: without it the exe imports
# VCRUNTIME140.dll, which is not part of Windows, and the README's promise of
# "no runtime needed" is false. Because the flag changes every fingerprint, the
# first run rebuilds the whole workspace.
#
# The artifact is a folder with exactly three files - shaiwin.exe, LICENSE,
# README.md - zipped, plus the SHA256 of the zip. All three land in the repository
# root and all three are gitignored.
param(
  [string]$OutDir = (Join-Path $PSScriptRoot "..")
)

$ErrorActionPreference = "Stop"
$LASTEXITCODE = 0
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $repo

function Invoke-Step([string]$name, [scriptblock]$body) {
  Write-Host "== $name =="
  & $body
  if ($LASTEXITCODE -ne 0) { throw "FAIL: $name exited $LASTEXITCODE" }
}

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
foreach ($needed in @("LICENSE", "README.md", "assets\shaiwin.ico")) {
  # Checked before the build so a missing file costs seconds, not four minutes.
  if (-not (Test-Path $needed)) { throw "FAIL: $needed is missing, and a release cannot be made without it" }
}
Write-Host "version $version (Cargo.toml)"

# Everything after this line is the workflow, step by step.
$env:RUSTFLAGS = "-C target-feature=+crt-static"

Invoke-Step "toolchain" {
  rustc --version
  cargo --version
  node --version
}
Invoke-Step "test" { cargo test --workspace }
Invoke-Step "clippy" { cargo clippy --workspace --all-targets -- -D warnings }
Invoke-Step "build" { cargo build -p shaiwin --release }
Invoke-Step "the exe wears the icon" {
  node scripts/check-exe-icon.mjs target/release/shaiwin.exe assets/shaiwin.ico
}
Invoke-Step "the icon group table" {
  node scripts/check-exe-group.mjs target/release/shaiwin.exe assets/shaiwin.ico
}

Invoke-Step "the exe carries its own runtime" {
  $text = [System.Text.Encoding]::ASCII.GetString(
    [System.IO.File]::ReadAllBytes("target/release/shaiwin.exe"))
  if ($text.Contains("VCRUNTIME140.dll")) {
    throw "the exe imports VCRUNTIME140.dll, so RUSTFLAGS did not reach the build"
  }
  "no VC++ runtime import"
}

$stage = Join-Path $repo "shaiwin-$version-win64"
$zip = Join-Path $repo "shaiwin-$version-win64.zip"
$sums = Join-Path $repo "SHA256SUMS.txt"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
if (Test-Path $zip) { Remove-Item $zip -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

# Three files, and the check below is what keeps it that way: a fourth one
# creeping in is how an artifact starts to differ from the published one.
Copy-Item target/release/shaiwin.exe $stage/
Copy-Item LICENSE $stage/
Copy-Item README.md $stage/
$packed = @(Get-ChildItem $stage -File)
$want = @("README.md", "LICENSE", "shaiwin.exe")
$got = @($packed | ForEach-Object { $_.Name } | Sort-Object)
if (Compare-Object $want $got) {
  throw "FAIL: the artifact holds $($got -join ', ') instead of $($want -join ', ')"
}

Compress-Archive -Path "$stage/*" -DestinationPath $zip -CompressionLevel Optimal
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $(Split-Path $zip -Leaf)" | Out-File -FilePath $sums -Encoding utf8 -NoNewline

Write-Host "== packaged =="
Get-ChildItem $stage -File | Select-Object Name, Length | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
Get-Item $zip | Select-Object Name, Length | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
Write-Host (Get-Content $sums -Raw)
