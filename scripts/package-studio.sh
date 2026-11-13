#!/usr/bin/env bash
# Builds the Linux packages for Studio:
#   dist/AI-Video-Studio-Linux.deb   the download for people: one file that installs the app and
#                                    FFmpeg on Ubuntu, Debian, Linux Mint, Pop!_OS... with a
#                                    double-click, adds it to the app menu and opens .aivp files
#   dist/ai-video-studio-linux-<arch>/ (and a .tar.gz)   the same files with an install.sh into
#                                    ~/.local, for other distros (FFmpeg from the system there)
# Usage: scripts/package-studio.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

cargo build -p studio --release

arch="$(uname -m)"
name="ai-video-studio-linux-$arch"
out="$root/dist/$name"
rm -rf "$out"
mkdir -p "$out/bin" "$out/share/applications" "$out/share/licenses" "$out/share/mime/packages"

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

# .aivp files are AI Video Studio projects, so they open in it with a double-click.
cat > "$out/share/mime/packages/ai-video-studio.xml" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/x-ai-video-project">
    <comment>AI Video Studio project</comment>
    <glob pattern="*.aivp"/>
    <icon name="ai-video-studio"/>
  </mime-type>
</mime-info>
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
mkdir -p "$prefix/share/mime/packages"
install -m 644 "$here/share/mime/packages/ai-video-studio.xml" "$prefix/share/mime/packages/"
command -v update-mime-database >/dev/null && update-mime-database "$prefix/share/mime" || true
command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
echo "Installed to $prefix. Find AI Video Studio in your app menu."
command -v ffmpeg >/dev/null || echo "Note: FFmpeg is not installed; run: sudo apt install ffmpeg"
EOF
chmod 755 "$out/install.sh"

tar -C "$root/dist" -czf "$root/dist/$name.tar.gz" "$name"
echo "Packaged to $out and dist/$name.tar.gz"

# The .deb: the same files under /usr. Installing it (a double-click, or
# `sudo apt install ./AI-Video-Studio-Linux.deb`) also installs FFmpeg and the libraries the app
# needs; the system's own triggers update the app menu, the icons and the file types.
if ! command -v dpkg-deb >/dev/null || ! command -v dpkg-shlibdeps >/dev/null; then
    echo "No dpkg-deb/dpkg-shlibdeps here (sudo apt install dpkg-dev): skipping the .deb"
    exit 0
fi
version="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' studio/Cargo.toml | head -1)"
deb="$tmp/deb"
mkdir -p "$deb/DEBIAN" "$deb/usr/bin" "$deb/usr/share/doc/ai-video-studio"
chmod 755 "$deb" "$deb/DEBIAN"
chmod 755 "$deb" "$deb/DEBIAN"
install -m 755 "$out/bin/ai-video-studio" "$deb/usr/bin/ai-video-studio"
cp -r "$out/share/applications" "$out/share/icons" "$out/share/mime" "$deb/usr/share/"
cp "$out/share/licenses/"* LICENSE-MIT LICENSE-APACHE "$deb/usr/share/doc/ai-video-studio/"
find "$deb/usr" -type d -exec chmod 755 {} +
find "$deb/usr/share" -type f -exec chmod 644 {} +

# The libraries the app links to, named the way this distro's packages provide them. Releases
# are built on the oldest supported Ubuntu, so the versions asked for suit newer ones too.
mkdir -p "$tmp/shlibs/debian"
printf 'Source: ai-video-studio\n\nPackage: ai-video-studio\nArchitecture: any\n' > "$tmp/shlibs/debian/control"
linked="$(cd "$tmp/shlibs" && dpkg-shlibdeps -O "$deb/usr/bin/ai-video-studio" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')"
# Loaded while running rather than linked: the window (X11) and the GPU (EGL; Vulkan if there).
runtime="libx11-6, libx11-xcb1, libxcursor1, libxi6, libxkbcommon-x11-0, libegl1"
cat > "$deb/DEBIAN/control" <<CONTROL
Package: ai-video-studio
Version: $version
Architecture: $(dpkg --print-architecture)
Maintainer: AI Video Studio <https://github.com/molla-rakib/ai-video-editor>
Installed-Size: $(du -sk "$deb/usr" | cut -f1)
Depends: ${linked:+$linked, }$runtime, ffmpeg
Recommends: libvulkan1, mesa-vulkan-drivers
Section: video
Priority: optional
Homepage: https://github.com/molla-rakib/ai-video-editor
Description: Motion graphics editor for product videos
 AI Video Studio makes product showcase videos (launch videos, app explainers) from
 screen and camera recordings, text and shapes. Everything works by hand; an optional
 AI assistant (your own Claude Code or Codex) can edit alongside you.
CONTROL
dpkg-deb --root-owner-group -Zxz --build "$deb" "$root/dist/AI-Video-Studio-Linux.deb" >/dev/null
echo "Packaged dist/AI-Video-Studio-Linux.deb (version $version)"
