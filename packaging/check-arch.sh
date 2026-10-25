#!/bin/bash
# Checks the Arch packages in clean archlinux containers (docs/RELEASING.md step 4): builds both from
# dist/aur with makepkg, runs namcap on each PKGBUILD and package, then installs each package into a
# fresh container with pacman and runs `neuralcanvas` against the host's Wayland session for a few
# seconds. Run packaging/release.sh first. Logs and the built packages go to dist/check-arch/.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
dist=$root/dist
out=$dist/check-arch
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)
rm -rf "$out"
mkdir -p "$out"
docker pull -q archlinux:latest >/dev/null

echo "== makepkg and namcap"
docker run --rm -v "$dist:/dist:ro" -v "$out:/out" -v nc-check-cargo:/cargo archlinux:latest bash -euc "
    pacman -Syu --noconfirm --needed base-devel namcap cargo hicolor-icon-theme wayland libxkbcommon vulkan-icd-loader >/dev/null
    useradd -m builder
    mkdir -p /cargo && chown builder /cargo
    cp -r /dist/aur /home/builder/aur
    cp /dist/neuralcanvas-$ver.tar.gz /home/builder/aur/neuralcanvas/
    cp /dist/neuralcanvas-$ver-x86_64.tar.zst /home/builder/aur/neuralcanvas-bin/
    chown -R builder /home/builder/aur
    for pkg in neuralcanvas neuralcanvas-bin; do
        su builder -c \"cd ~/aur/\$pkg && CARGO_HOME=/cargo makepkg -f --noconfirm\" > /out/makepkg-\$pkg.log 2>&1
        { echo \"namcap PKGBUILD (\$pkg):\"; namcap /home/builder/aur/\$pkg/PKGBUILD
          echo \"namcap package (\$pkg):\"; namcap /home/builder/aur/\$pkg/*.pkg.tar.zst; } > /out/namcap-\$pkg.txt 2>&1
        cp /home/builder/aur/\$pkg/*.pkg.tar.zst /out/
    done
    chown -R $(id -u):$(id -g) /out
"
cat "$out"/namcap-*.txt

echo "== clean install and run"
for pkg in "$out"/neuralcanvas-"$ver"-*.pkg.tar.zst "$out"/neuralcanvas-bin-"$ver"-*.pkg.tar.zst; do
    name=$(basename "$pkg")
    docker run --rm -v "$out:/out" \
        --device /dev/dri \
        -v "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY:/run/wayland/$WAYLAND_DISPLAY" \
        -e WAYLAND_DISPLAY="$WAYLAND_DISPLAY" -e XDG_RUNTIME_DIR=/run/wayland \
        archlinux:latest bash -euc "
        pacman -Syu --noconfirm --needed vulkan-intel vulkan-radeon >/dev/null
        pacman -U --noconfirm /out/$name > /out/install-$name.log 2>&1
        useradd -m -u $(id -u) user
        chown user /run/wayland
        su user -c 'timeout -s TERM 6 neuralcanvas' > /out/run-$name.log 2>&1 || status=\$?
        echo \"exit status: \${status:-0} (124 = still running when stopped, as expected)\" >> /out/run-$name.log
    "
    echo "$name:"; cat "$out/run-$name.log"
done
