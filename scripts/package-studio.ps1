# Packages Studio for Windows.
#   (no switch)  dist\AI Video Studio\   the app, bundled FFmpeg and licenses (a portable folder)
#   -Installer   dist\AI-Video-Studio-Setup-x64.exe   one file that installs all of that, with a
#                Start menu shortcut, an uninstaller and .aivp file association (needs Inno Setup 6:
#                winget install JRSoftware.InnoSetup)
#   -Zip         dist\AI-Video-Studio-windows-x64.zip   the portable folder, zipped
#   -SkipBuild   package what target\release already holds (the release workflow signs the exe
#                between building it and packaging it)
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package-studio.ps1 [-Installer] [-Zip]
param([switch]$Zip, [switch]$Installer, [switch]$SkipBuild)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not $SkipBuild) {
    cargo build -p studio --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

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
# The licenses of what is bundled travel with the app (the app's icon is inside the .exe).
foreach ($f in "LICENSE-Inter.txt", "LICENSE-Poppins.txt", "LICENSE-HindSiliguri.txt") {
    Copy-Item (Join-Path "studio\assets\fonts" $f) (Join-Path $out "licenses")
}
Copy-Item "studio\assets\icons\LICENSE-lucide.txt" (Join-Path $out "licenses")
Copy-Item "LICENSE-MIT", "LICENSE-APACHE" (Join-Path $out "licenses")
Write-Output "Packaged to $out"

if ($Zip) {
    $zipPath = Join-Path $root "dist\AI-Video-Studio-windows-x64.zip"
    if (Test-Path $zipPath) { Remove-Item -Force $zipPath }
    Compress-Archive -Path $out -DestinationPath $zipPath
    Write-Output "Zipped to $zipPath"
}

if ($Installer) {
    $iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
    if (-not $iscc) {
        $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    }
    if (-not $iscc) { throw "Inno Setup 6 was not found. Install it with: winget install JRSoftware.InnoSetup" }
    $version = (Select-String -Path "studio\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
    # Setup.exe carries a four-part number; a tag like 0.3.0-beta.1 keeps its digits only.
    $numeric = ($version -replace '[^0-9.].*$', '')
    # The installer's own icon: the same one the app draws for its window.
    $assets = Join-Path $root "dist\installer"
    # (A windowed app: PowerShell only waits for it, and learns its exit code, through Start-Process.)
    $icons = Start-Process -FilePath $exe -ArgumentList "--write-icons", "`"$assets`"" -Wait -PassThru
    if ($icons.ExitCode -ne 0 -or -not (Test-Path (Join-Path $assets "ai-video-studio.ico"))) { throw "Writing the icon failed" }
    # The script finds the app folder, the icon and the output folder relative to itself.
    & $iscc "/Qp" "/DAppVersion=$numeric" (Join-Path $root "scripts\installer.iss")
    if ($LASTEXITCODE -ne 0) { throw "Building the installer failed" }
    Remove-Item -Recurse -Force $assets
    Write-Output "Installer: $(Join-Path $root 'dist\AI-Video-Studio-Setup-x64.exe')"
}
