#!/bin/bash
# Runs inside an ubuntu:24.04 container (packaging/release.sh starts it): builds the release binary
# from the source tarball (glibc 2.39, so it runs on older distributions than Arch), then writes the
# binary tarball and the AppImage to /dist. Arguments: version, Rust toolchain, owner uid:gid, mtime.
set -euo pipefail
ver=$1 rust=$2 owner=$3 mtime=$4
appimagetool_url=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
appimagetool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
runtime_url=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
# What the AppImage bundles as a fallback (AppRun uses it only if the host lacks one), and the Ubuntu
# packages they come from (for their licences). Not libwayland: the host's Vulkan driver needs the
# host's own (Ubuntu's 1.22 lacks symbols Mesa on Arch calls).
libs=(libvulkan.so.1 libxkbcommon.so.0)
lib_packages=(libvulkan1 libxkbcommon0)

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends ca-certificates curl gcc libc6-dev pkg-config file \
    zstd "${lib_packages[@]}" >/dev/null
export CARGO_HOME=/cargo RUSTUP_HOME=/cargo/rustup PATH=/cargo/bin:$PATH
command -v cargo >/dev/null || curl -sSf https://sh.rustup.rs | sh -s -- -y -q --no-modify-path \
    --profile minimal --default-toolchain "$rust"
rustup toolchain install --profile minimal "$rust" >/dev/null

mkdir -p /build && cd /build
tar -xzf "/dist/neuralcanvas-$ver.tar.gz"
cd "neuralcanvas-$ver"
CARGO_TARGET_DIR=/target cargo +"$rust" build --release --locked -p neuralcanvas

# The installed tree, as the source PKGBUILD lays it out.
root=/build/stage/neuralcanvas-$ver
install -Dm755 /target/release/neuralcanvas -t "$root/usr/bin"
install -Dm644 packaging/sh.nodaysidle.NeuralCanvas.desktop -t "$root/usr/share/applications"
install -Dm644 packaging/sh.nodaysidle.NeuralCanvas.metainfo.xml -t "$root/usr/share/metainfo"
install -Dm644 packaging/icons/sh.nodaysidle.NeuralCanvas.svg -t "$root/usr/share/icons/hicolor/scalable/apps"
for size in 16 32 48 64 128 256 512; do
    install -Dm644 "packaging/icons/hicolor/${size}x${size}/apps/sh.nodaysidle.NeuralCanvas.png" \
        -t "$root/usr/share/icons/hicolor/${size}x${size}/apps"
done
install -Dm644 LICENSE crates/nc-render/fonts/{Manrope,DMMono}-OFL.txt -t "$root/usr/share/licenses/neuralcanvas"
(cd /build/stage && tar --sort=name --mtime="@$mtime" --owner=0 --group=0 --numeric-owner \
    -cf - "neuralcanvas-$ver" | zstd -19 -q -T0) > "/dist/neuralcanvas-$ver-x86_64.tar.zst"

# The AppImage: the same tree, plus the fallback libraries.
app=/build/NeuralCanvas.AppDir
cp -a "$root" "$app"
for lib in "${libs[@]}"; do
    install -Dm644 "$(readlink -f "/usr/lib/x86_64-linux-gnu/$lib")" "$app/usr/lib/fallback/$lib"
done
for pkg in "${lib_packages[@]}"; do
    install -Dm644 "/usr/share/doc/$pkg/copyright" "$app/usr/share/licenses/neuralcanvas/bundled/$pkg"
done
install -m755 packaging/appimage/AppRun "$app/AppRun"
cp packaging/sh.nodaysidle.NeuralCanvas.desktop "$app/"
cp packaging/icons/hicolor/256x256/apps/sh.nodaysidle.NeuralCanvas.png "$app/"
ln -s sh.nodaysidle.NeuralCanvas.png "$app/.DirIcon"

cd /build
curl -sSfLo appimagetool "$appimagetool_url"
curl -sSfLo runtime "$runtime_url"
echo "$appimagetool_sha  appimagetool" | sha256sum -c --quiet
echo "$runtime_sha  runtime" | sha256sum -c --quiet
chmod +x appimagetool
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 SOURCE_DATE_EPOCH=$mtime ./appimagetool --no-appstream \
    --runtime-file runtime "$app" "/dist/NeuralCanvas-$ver-x86_64.AppImage"
chown "$owner" /dist/*
