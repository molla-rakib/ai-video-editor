# Quick local test build: cargo build --release, then just the exe at
# build\ai-video-studio-v<version>.exe (nothing else - no ffmpeg, no icons, no licenses).
# Not to be confused with Cargo's own target\release\ - that's its build cache, this is ours.
# For an actual distributable package (bundled FFmpeg, icons, licenses) use
# scripts\package-studio.ps1 instead, which builds dist\AI Video Studio\.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build -p studio --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$version = (Select-String -Path "studio\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$out = Join-Path $root "build"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null

$exe = Join-Path $out "ai-video-studio-v$version.exe"
Copy-Item "target\release\ai-video-studio.exe" $exe
Write-Output "Built $exe"
