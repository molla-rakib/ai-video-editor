# Builds the portable Windows folder for Studio: dist\AI Video Studio\ with the app
# (its icon is embedded in the .exe), bundled FFmpeg, the icon files and licenses.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package-studio.ps1 [-Zip]
param([switch]$Zip)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build -p studio --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$out = Join-Path $root "dist\AI Video Studio"
$running = Get-Process ai-video-studio -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$out*" }
if ($running) { throw "AI Video Studio is running from $out - close it first (your project saves automatically), then run this again." }
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force (Join-Path $out "ffmpeg") | Out-Null
New-Item -ItemType Directory -Force (Join-Path $out "licenses") | Out-Null

$exe = Join-Path $out "ai-video-studio.exe"
Copy-Item "target\release\ai-video-studio.exe" $exe
foreach ($f in "ffmpeg.exe", "ffprobe.exe", "LICENSE.txt") {
    $src = Join-Path "vendor\ffmpeg" $f
    if (-not (Test-Path $src)) { throw "Missing $src - see README (Setup)" }
    Copy-Item $src (Join-Path $out "ffmpeg")
}
# A "shared" FFmpeg build keeps its libraries in DLLs next to the exes; they must ship too.
Get-ChildItem "vendor\ffmpeg\*.dll" -ErrorAction SilentlyContinue | Copy-Item -Destination (Join-Path $out "ffmpeg")
foreach ($f in "LICENSE-Inter.txt", "LICENSE-Poppins.txt", "LICENSE-HindSiliguri.txt") {
    Copy-Item (Join-Path "studio\assets\fonts" $f) (Join-Path $out "licenses")
}
Copy-Item "studio\assets\icons\LICENSE-lucide.txt" (Join-Path $out "licenses")

# The icon as files too (.ico for shortcuts, PNGs for anything else).
& $exe --write-icons (Join-Path $out "icons")
if ($LASTEXITCODE -ne 0) { throw "Writing the icons failed" }

if ($Zip) {
    $zipPath = Join-Path $root "dist\AI-Video-Studio-windows-x64.zip"
    if (Test-Path $zipPath) { Remove-Item -Force $zipPath }
    Compress-Archive -Path $out -DestinationPath $zipPath
    Write-Output "Packaged to $out and $zipPath"
} else {
    Write-Output "Packaged to $out"
}
