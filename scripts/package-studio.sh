#!/usr/bin/env bash
# Builds the Linux package for Studio: dist/ai-video-studio-linux-<arch>/ (and a .tar.gz)
# with the app, its icon in every size, a desktop entry and an installer.
# FFmpeg comes from the system (sudo apt install ffmpeg).
# Usage: scripts/package-studio.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

cargo build -p studio --release

arch="$(uname -m)"
name="ai-video-studio-linux-$arch"
out="$root/dist/$name"
rm -rf "$out"
mkdir -p "$out/bin" "$out/share/applications" "$out/share/licenses"

install -m 755 target/release/ai-video-studio "$out/bin/ai-video-studio"
cp studio/assets/fonts/LICENSE-Inter.txt studio/assets/fonts/LICENSE-Poppins.txt \
   studio/assets/fonts/LICENSE-HindSiliguri.txt studio/assets/icons/LICENSE-lucide.txt \
   "$out/share/licenses/"

# The icon in the freedesktop hicolor theme layout.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
"$out/bin/ai-video-studio" --write-icons "$tmp"
for png in "$tmp"/ai-video-studio-*.png; do
    size="$(basename "$png" .png)"
    size="${size##*-}"
    mkdir -p "$out/share/icons/hicolor/${size}x${size}/apps"
    cp "$png" "$out/share/icons/hicolor/${size}x${size}/apps/ai-video-studio.png"
done

cat > "$out/share/applications/ai-video-studio.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=AI Video Studio
GenericName=Video Editor
Comment=Motion graphics editor for product videos, with an optional AI assistant
Exec=ai-video-studio %f
Icon=ai-video-studio
Terminal=false
Categories=AudioVideo;Video;AudioVideoEditing;
MimeType=application/x-ai-video-project;
StartupWMClass=ai-video-studio
EOF

cat > "$out/install.sh" <<'EOF'
#!/usr/bin/env bash
# Installs AI Video Studio for the current user (~/.local), or system-wide with
# PREFIX=/usr/local sudo -E ./install.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
mkdir -p "$prefix/bin" "$prefix/share/applications"
install -m 755 "$here/bin/ai-video-studio" "$prefix/bin/ai-video-studio"
cp -r "$here/share/icons" "$prefix/share/"
install -m 644 "$here/share/applications/ai-video-studio.desktop" "$prefix/share/applications/"
command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
echo "Installed to $prefix. Find AI Video Studio in your app menu."
command -v ffmpeg >/dev/null || echo "Note: FFmpeg is not installed; run: sudo apt install ffmpeg"
EOF
chmod 755 "$out/install.sh"

tar -C "$root/dist" -czf "$root/dist/$name.tar.gz" "$name"
echo "Packaged to $out and dist/$name.tar.gz"
